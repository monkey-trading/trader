//! One account trading one market with a strategy. At startup the bot reads the market, the
//! account, and the market's recent marks from the node; from then on it follows the public log:
//! each `Prices` command moves the index mark, each completed bar goes to the strategy, and the
//! account's fills move its position. A target the position is not at sends a signed market order,
//! unless the account's controls (sizing, risk limits, margin, hours) refuse it, the bar is history
//! the stream is replaying, or the last order could still enter the log.

mod day;
mod fills;
mod orders;
mod startup;
pub use startup::stop_signal;
mod stop;

use std::collections::VecDeque;

use anyhow::anyhow;
use engine_core::Fixed;
use engine_core::command::{AdminCommand, Command};
use engine_core::eip712::{Eip712Domain, Signature};
use engine_core::event::Event;
use engine_core::market::{MarketSpec, QuoteCurrency};
use engine_core::types::{MarketId, Millis};
use tracing::{debug, error, info, warn};
use trader_core::bars::{Bar, BarBuilder};
use trader_core::controls::{Account, BarTarget, Controls, Decision, Refusal};
use trader_core::hours::GapTracker;
use trader_core::marks::IndexMarks;
use trader_core::strategies::{Strategy, is_valid_close};

use crate::bot::day::{Saved, StateFile, holding};
use crate::config::Config;
use crate::keys::AccountKey;
use crate::node::{CallError, Node};
use crate::stream::message::{CommandMessage, Head, Quotes};
use crate::stream::{Frame, Stream, StreamError};

/// How far behind the node's time a bar may close and still be acted on; one further behind is
/// history the stream is replaying, quoted at marks long gone.
pub(super) const REPLAY_LAG_MS: Millis = 30_000;

/// How long, in sequenced time, between reads of the account while the last one was not at the log.
const READ_RETRY_MS: Millis = 5_000;

/// How long, in sequenced time, between reads of the account that was at the log.
const READ_EVERY_MS: Millis = 60_000;

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
    /// Stopping left resting orders, or a position it was to close, or could not confirm it did not.
    #[error("the stop is incomplete: {0}")]
    StopIncomplete(String),
}

/// The bot's state between messages.
pub struct Bot {
    node: Node,
    key: AccountKey,
    domain: Eip712Domain,
    market: MarketId,
    /// The market's listing, for its margin tiers and hours.
    spec: MarketSpec,
    max_slippage_bps: i64,
    close_on_stop: bool,
    marks: IndexMarks,
    /// The bar length, and how many bars of history warm the strategy up.
    bar_ms: Millis,
    history_bars: i64,
    /// Bars from the log, once the index is exact and the history has warmed the strategy up.
    bars: Option<BarBuilder>,
    /// The earliest sequenced time a failed warm-up is tried again.
    warm_up_after: Millis,
    strategy: Strategy,
    /// Sizing, risk limits and the minimum hold, applied to each target.
    controls: Controls,
    /// The first bar after a closure of the market.
    gaps: GapTracker,
    /// Where the day's loss and the positions' opening times are kept, what was last written, and
    /// whether the last write failed.
    state_file: StateFile,
    saved_state: Saved,
    state_unsaved: bool,
    /// Whether the daily loss alert is raised.
    loss_alert: bool,
    /// The earliest sequenced time the daily loss limit tries to close the position again.
    flatten_after: Millis,
    /// Whether the account is in the cash alert: its cash is not positive and an order was refused.
    cash_alert: bool,
    /// Whether the last read of the account was not at the log, or failed, so its balance and entry
    /// are not the position's: nothing is valued or ordered until a read agrees.
    account_behind: bool,
    /// The earliest sequenced time the account is read again: soon while it is behind the log, and
    /// otherwise after `READ_EVERY_MS`, so a balance moved by a withdrawal or financing is found.
    read_retry_after: Millis,
    /// When the market split, if the bot halted at one.
    split_at: Option<Millis>,
    /// Whether the next reversal is only a close: the engine refused the last one whole.
    close_first: bool,
    /// The signatures of the last orders this process signed, to tell its fills from others': a
    /// signature is over the whole request, so no other request carries one of them.
    signed_orders: VecDeque<Signature>,
    /// The account's signed position in the market, and its entry price as the last read gave it.
    position: Fixed,
    entry: Fixed,
    /// The account's balance, as the last verified read of the account gave it.
    balance: Fixed,
    /// The last index mark: the node's at startup, then the log's, which orders are quoted at.
    mark: Option<Fixed>,
    /// The last exact index mark the bot derived, which a move is a change of.
    exact_mark: Option<Fixed>,
    /// The time of the first command at which the index was exact.
    exact_from: Option<Millis>,
    /// The moves of the index from `exact_from` until the warm-up, which go into the bars then.
    pending_moves: Vec<(Millis, Fixed)>,
    /// The expiry of the last order signed. Until the log passes it, that order may still enter the
    /// log, so its fill may still be to come, and no other order is placed.
    order_expires_at: Option<Millis>,
    /// The latest expiry an order signed before this start can carry. Until the log passes it, an
    /// earlier process's order may still enter the log, and no order is placed.
    trading_from: Millis,
    /// The time of the last command read from the log.
    log_ts: Millis,
}

/// Runs the bot until `stop` completes, then cancels the account's resting orders and, when
/// configured, closes its position; or until it halts on its own, leaving the account as it is.
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
    let (mut bot, mut stream) = match startup::start(&cfg, key).await {
        Ok(started) => started,
        Err(e) => {
            if let Some(CallError::WrongSigner { .. }) = e.downcast_ref::<CallError>() {
                error!(alert = "node_signer_mismatch", "{e:#}");
            }
            return Err(e);
        }
    };
    let result = match bot.follow(&mut stream, stop).await {
        Ok(()) => bot.stop(&mut stream).await,
        // What the bot saw is not what it trades on: it changes nothing, and leaves the account to
        // the user.
        Err(Halt::WrongSigner) => Err(Halt::WrongSigner),
        Err(halt) => {
            error!(alert = "bot_halted", "{halt}; the account is left as it is");
            Err(halt)
        }
    };
    if let Err(Halt::WrongSigner) = &result {
        error!(alert = "node_signer_mismatch", "{}", Halt::WrongSigner);
    }
    Ok(result?)
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
                    let frame = frame.map_err(|e| match e {
                        StreamError::WrongSigner => Halt::WrongSigner,
                        StreamError::Unreadable(e) => Halt::Unfollowable(e),
                    })?;
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
    /// - an answer signed by another key than the signer
    async fn apply(&mut self, frame: &Frame) -> Result<(), Halt> {
        let unreadable =
            |e: anyhow::Error| Halt::Unfollowable(e.context(format!("the log message at {}", frame.offset)));
        let head: Head<'_> = serde_json::from_str(&frame.msg).map_err(|e| unreadable(e.into()))?;
        if head.kind != "command" {
            return Ok(());
        }
        let message: CommandMessage<'_> = serde_json::from_str(&frame.msg).map_err(|e| unreadable(e.into()))?;
        self.log_ts = message.ts;
        self.marks.advance(message.ts);
        let command = message.head().map_err(unreadable)?;
        let events = message.events().map_err(unreadable)?;
        match command.kind {
            "prices" if command.market == Some(self.market) => {
                let quotes: Quotes = serde_json::from_str(message.cmd.get()).map_err(|e| unreadable(e.into()))?;
                self.marks.store(message.ts, &quotes.quotes);
            }
            "admin" | "signed_admin" => self.relist(message.ts, &message.command().map_err(unreadable)?, &events)?,
            _ => {}
        }
        let mut filled = false;
        for event in &events {
            match event {
                Event::CorporateActionFailed { market, reason, .. } if *market == self.market => {
                    let reason =
                        format!("a corporate action failed for the market, which stops dealing in it: {reason}");
                    return Err(Halt::Unfollowable(anyhow!(reason)));
                }
                // A split changes the units of every price the strategy has seen.
                Event::CorporateSplit { market, .. } if *market == self.market => {
                    self.split_at = Some(message.ts);
                    self.save_state().await;
                    let reason = "the market split, which changes its units: the bot does not follow a split";
                    return Err(Halt::Unfollowable(anyhow!(reason)));
                }
                _ => {}
            }
            filled |= self.on_event(message.seq, message.ts, event);
        }
        if filled {
            self.check_own_fills(&message, &events)?;
            self.refresh_balance().await?;
        }
        // Only a move of the exact index goes into the bars, as the node's marks history records only
        // moves, so bars built live and bars warmed up from the history are built alike. The first
        // exact value is where moves start from; the history holds what came before it.
        let mark = self.marks.mark(message.ts);
        let moved = mark.filter(|mark| self.exact_mark.is_some_and(|last| last != *mark));
        if mark.is_some() {
            (self.mark, self.exact_mark) = (mark, mark);
        }
        let node_time = Millis::try_from(frame.time).unwrap_or(Millis::MAX);
        let lag = node_time.saturating_sub(message.ts);
        let read = message.ts >= self.read_retry_after;
        if read {
            self.refresh_balance().await?;
            let wait = if self.account_behind { READ_RETRY_MS } else { READ_EVERY_MS };
            self.read_retry_after = message.ts.saturating_add(wait);
        }
        if moved.is_some() || filled || read {
            self.check_day(message.ts).await?;
        }
        self.retry_flatten(message.ts, lag).await?;
        if self.bars.is_none() {
            return self.warm_up(message.ts, moved, node_time).await;
        }
        if let Some(bar) = self.bars.as_mut().and_then(|bars| bars.on_command(message.ts, moved)) {
            self.on_bar(bar, message.ts, node_time.saturating_sub(message.ts)).await?;
        }
        Ok(())
    }

    /// Takes the price rules of a listing of the market that an applied admin command at `ts` sets.
    /// Error cases:
    /// - the new listing is one the bot does not trade
    fn relist(&mut self, ts: Millis, command: &Command, events: &[Event]) -> Result<(), Halt> {
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
        self.controls.check_market(spec).map_err(|e| Halt::Unfollowable(e.into()))?;
        info!(market = self.market, rules = ?spec.price_rules, "the market's listing changed");
        self.marks.set_rules(ts, spec.price_rules);
        self.marks.set_schedule(ts, spec.schedule.clone());
        self.spec = spec.clone();
        Ok(())
    }

    /// Hands a bar to the strategy, completed by a command at `ts` that the node sent `lag` ms after
    /// it, and orders toward its target as the controls allow. A bar the stream is replaying from
    /// history, or one that closes before the log has passed the last order's expiry, places no
    /// order: that order's fill may not be in the position yet. The next bar decides again.
    /// Error cases:
    /// - an order call answered by another key than the signer
    async fn on_bar(&mut self, bar: Bar, ts: Millis, lag: Millis) -> Result<(), Halt> {
        self.refresh_balance().await?;
        let gap = self.gaps.on_bar(&self.spec.schedule, bar.start, self.bar_ms);
        let target = if is_valid_close(bar.close) {
            self.strategy.on_close(bar.close)
        } else {
            warn!(start = bar.start, close = %bar.close, "the bar's close is not a mark the strategies take: no data");
            None
        };
        debug!(start = bar.start, close = %bar.close, ?target, gap, lag, "bar");
        let (Some(target), Some(mark)) = (target, self.mark) else { return Ok(()) };
        if self.account_behind || self.state_unsaved {
            info!(
                ?target,
                behind = self.account_behind,
                "the account read is not at the log, or the saved state is not written: no order"
            );
            return Ok(());
        }
        let holding = holding(&self.spec, self.market, self.position, self.entry, mark);
        let holdings = holding.as_slice();
        let account = Account { balance: self.balance, holdings };
        let wanted = BarTarget {
            ts,
            market: self.market,
            spec: &self.spec,
            mark,
            target,
            gap,
            close_first: self.close_first,
            max_slippage_bps: self.max_slippage_bps,
        };
        let decision = self.controls.decide(&account, &wanted);
        self.track_cash_alert(&decision);
        let intent = match decision {
            Decision::AtTarget => return Ok(()),
            Decision::Refused(refusal) => {
                match refusal {
                    Refusal::DailyLossStopped | Refusal::MarketClosed => debug!(?target, ?refusal, "no order"),
                    _ => info!(?target, ?refusal, "no order"),
                }
                return Ok(());
            }
            Decision::Order(_) | Decision::Flatten if lag > REPLAY_LAG_MS => {
                info!(?target, lag, "the bar is history the stream is replaying: no order");
                return Ok(());
            }
            Decision::Flatten => return self.flatten(ts).await,
            Decision::Order(intent) => intent,
        };
        if let Some(reason) = self.order_in_flight(ts) {
            info!(?target, "{reason}: no order");
            return Ok(());
        }
        info!(?target, position = %self.position, "moving to the target");
        self.place(intent, bar, ts).await
    }

    /// Raises the cash alert on the first refusal for cash that is not positive, and resolves it on
    /// the first decision after cash is positive again.
    fn track_cash_alert(&mut self, decision: &Decision) {
        let no_cash = matches!(decision, Decision::Refused(Refusal::NoCash));
        if no_cash && !self.cash_alert {
            self.cash_alert = true;
            warn!(alert = "cash_not_positive", balance = %self.balance, "cash is not positive: no order adds to a position, and an account with negative cash at a MONKEY period's cutoff earns no weight for it");
        } else if !no_cash && self.cash_alert && self.balance.is_positive() {
            self.cash_alert = false;
            info!(alert_resolved = "cash_not_positive", balance = %self.balance, "cash is positive again");
        }
    }
}

/// Checks the bot can follow `spec`'s index from the log: a market quoted in USD and priced from spot
/// quotes, which may keep trading hours.
/// Error cases:
/// - another quote currency, or futures contracts
pub(crate) fn supported(spec: &MarketSpec) -> anyhow::Result<()> {
    anyhow::ensure!(
        spec.quote_currency == QuoteCurrency::Usd && spec.contracts.is_empty(),
        "{} is not a USD market priced from spot quotes, the only markets the bot trades",
        spec.symbol
    );
    Ok(())
}
