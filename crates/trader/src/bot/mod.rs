//! One account trading one market with an SMA crossover. At startup the bot reads the market, the
//! account, and the market's recent marks from the node; from then on it follows the public log:
//! each `Prices` command moves the index mark, each completed bar goes to the strategy, and the
//! account's fills move its position. A target the position is not at sends a signed market order.

mod orders;
mod startup;
pub use startup::stop_signal;

use engine_core::Fixed;
use engine_core::command::{AdminCommand, Command};
use engine_core::eip712::Eip712Domain;
use engine_core::event::Event;
use engine_core::market::{MarketSpec, QuoteCurrency};
use engine_core::session::Schedule;
use engine_core::types::MarketId;
use tracing::{debug, error, info};
use trader_core::bars::{Bar, BarBuilder};
use trader_core::marks::IndexMarks;
use trader_core::strategy::{SmaCrossover, order_for};

use crate::config::Config;
use crate::keys::AccountKey;
use crate::node::Node;
use crate::stream::message::{CommandMessage, Head, Quotes};
use crate::stream::{Frame, Stream};

/// Why the bot stopped on its own.
#[derive(Debug, thiserror::Error)]
pub enum Halt {
    /// An answer or frame from the node is not the configured signer's: nothing it says is trusted,
    /// and nothing more is sent to it.
    #[error("the node's answers are not signed by the configured signer")]
    WrongSigner,
    /// The log holds what the bot cannot follow; it stops before acting on a view it cannot confirm.
    #[error("{0:#}")]
    Unfollowable(anyhow::Error),
}

/// The bot's state between messages.
pub struct Bot {
    node: Node,
    key: AccountKey,
    domain: Eip712Domain,
    market: MarketId,
    qty: Fixed,
    max_slippage_bps: i64,
    close_on_stop: bool,
    marks: IndexMarks,
    bars: BarBuilder,
    strategy: SmaCrossover,
    /// The account's signed position in the market.
    position: Fixed,
    /// The account's balance from the startup read, moved by each fill's realised PnL and fee.
    balance: Fixed,
    /// The last index mark: the node's at startup, then the log's.
    mark: Option<Fixed>,
}

/// Runs the bot until `stop` completes, or until it halts on its own; then cancels the account's
/// resting orders and, when configured, closes its position, unless the node signs with another key.
/// Error cases:
/// - the key or the startup reads fail (see `startup::start`)
/// - the bot halts (see `Halt`)
pub async fn run(
    cfg: Config,
    env: impl Fn(&str) -> Option<String>,
    stop: impl Future<Output = ()>,
) -> anyhow::Result<()> {
    let key = AccountKey::load(&cfg.account.key, env).await?;
    info!(account = %key.address(), "account key unlocked");
    let (mut bot, mut stream) = startup::start(&cfg, key).await?;
    let halt = match bot.follow(&mut stream, stop).await {
        Err(Halt::WrongSigner) => Err(Halt::WrongSigner),
        followed => {
            if let Err(halt) = &followed {
                error!(alert = "bot_halted", "{halt}");
            }
            bot.stop().await.and(followed)
        }
    };
    if let Err(Halt::WrongSigner) = &halt {
        error!(alert = "node_signer_mismatch", "{}", Halt::WrongSigner);
    }
    Ok(halt?)
}

impl Bot {
    /// Applies the log's messages until `stop` completes or the bot halts.
    /// Error cases:
    /// - see `Halt`
    async fn follow(&mut self, stream: &mut Stream, stop: impl Future<Output = ()>) -> Result<(), Halt> {
        tokio::pin!(stop);
        loop {
            tokio::select! {
                biased;
                () = &mut stop => {
                    info!("stop signal");
                    return Ok(());
                }
                frame = stream.next() => {
                    let frame = frame.map_err(|_| Halt::WrongSigner)?;
                    self.apply(&frame).await?;
                }
            }
        }
    }

    /// Applies one message of the log: a `Prices` command for the market stores its quotes, a new
    /// listing of the market changes its rules, the account's fills move its position, and the
    /// command's time may complete a bar, which may place an order.
    /// Error cases:
    /// - a command or its events the pinned engine cannot read
    /// - a new listing makes the market one the bot does not trade
    /// - an order call answered by another key than the signer
    async fn apply(&mut self, frame: &Frame) -> Result<(), Halt> {
        let unreadable =
            |e: anyhow::Error| Halt::Unfollowable(e.context(format!("the log message at {}", frame.offset)));
        let head: Head<'_> = serde_json::from_str(&frame.msg).map_err(|e| unreadable(e.into()))?;
        if head.kind != "command" {
            return Ok(());
        }
        let message: CommandMessage<'_> = serde_json::from_str(&frame.msg).map_err(|e| unreadable(e.into()))?;
        let command = message.head().map_err(unreadable)?;
        let events = message.events().map_err(unreadable)?;
        match command.kind {
            "prices" if command.market == Some(self.market) => {
                let quotes: Quotes = serde_json::from_str(message.cmd.get()).map_err(|e| unreadable(e.into()))?;
                self.marks.store(message.ts, &quotes.quotes);
            }
            "admin" | "signed_admin" => self.relist(&message.command().map_err(unreadable)?, &events)?,
            _ => {}
        }
        events.iter().for_each(|event| self.on_event(message.seq, event));
        let mark = self.marks.mark(message.ts);
        if mark.is_some() {
            self.mark = mark;
        }
        if let Some(bar) = self.bars.on_command(message.ts, mark) {
            self.on_bar(bar).await?;
        }
        Ok(())
    }

    /// Takes the price rules of a listing of the market that an applied admin command sets.
    /// Error cases:
    /// - the new listing is one the bot does not trade
    fn relist(&mut self, command: &Command, events: &[Event]) -> Result<(), Halt> {
        let admin = match command {
            Command::Admin(admin) => admin,
            Command::SignedAdmin(request) => &request.command,
            _ => return Ok(()),
        };
        let AdminCommand::UpsertMarket(spec) = admin else { return Ok(()) };
        if spec.id != self.market || !events.contains(&Event::AdminApplied) {
            return Ok(());
        }
        supported(spec).map_err(Halt::Unfollowable)?;
        info!(market = self.market, rules = ?spec.price_rules, "the market's listing changed");
        self.marks.set_rules(spec.price_rules);
        Ok(())
    }

    /// Follows the account's fills: the position in the market, and the balance.
    fn on_event(&mut self, seq: u64, event: &Event) {
        let Event::Fill(fill) = event else { return };
        if fill.account != self.key.address() {
            return;
        }
        self.balance += fill.realised_pnl - fill.profit_fee;
        if fill.market == self.market {
            self.position = fill.position;
        }
        info!(
            seq,
            order_id = fill.order_id,
            market = fill.market,
            side = ?fill.side,
            qty = %fill.qty,
            price = %fill.price,
            realised_pnl = %fill.realised_pnl,
            profit_fee = %fill.profit_fee,
            liquidation = fill.liquidation,
            position = %self.position,
            balance = %self.balance,
            "fill"
        );
    }

    /// Hands a completed bar to the strategy, and orders toward its target.
    /// Error cases:
    /// - an order call answered by another key than the signer
    async fn on_bar(&mut self, bar: Bar) -> Result<(), Halt> {
        let target = self.strategy.on_close(bar.close);
        let (fast, slow) = self.strategy.averages();
        debug!(start = bar.start, close = %bar.close, ?fast, ?slow, ?target, "bar");
        let Some(target) = target else { return Ok(()) };
        let Some(intent) = order_for(target, self.qty, self.position) else { return Ok(()) };
        info!(?target, position = %self.position, "moving to the target");
        self.place(intent, bar).await
    }
}

/// Checks the bot can follow `spec`'s index from the log: a market quoted in USD, priced from spot
/// quotes, and never closed.
/// Error cases:
/// - another quote currency, futures contracts, or trading hours
pub(crate) fn supported(spec: &MarketSpec) -> anyhow::Result<()> {
    anyhow::ensure!(
        spec.quote_currency == QuoteCurrency::Usd && spec.contracts.is_empty() && spec.schedule == Schedule::AlwaysOpen,
        "{} is not a USD market priced from spot quotes that never closes, the only markets the bot trades",
        spec.symbol
    );
    Ok(())
}
