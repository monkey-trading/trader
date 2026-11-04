//! What the bot reads before it follows the log: the market, the account, and the market's recent
//! marks, each a verified answer; and the stop signals it listens for.

use std::collections::VecDeque;

use anyhow::{Context, bail};
use engine_core::Fixed;
use engine_core::eip712;
use engine_core::types::Millis;
use engine_wire::Deployment;
use tokio::signal::unix::{SignalKind, signal};
use tracing::{info, warn};
use trader_core::bars::BarBuilder;
use trader_core::marks::IndexMarks;
use trader_core::strategy::SmaCrossover;

use crate::bot::{Bot, Halt, orders, supported};
use crate::config::Config;
use crate::keys::AccountKey;
use crate::node::api::MarkBucket;
use crate::node::{CallError, Node, Tls};
use crate::stream::Stream;

/// The marks history's resolutions, widest first, with their bucket widths.
const RESOLUTIONS: [(&str, Millis); 3] = [("1d", 86_400_000), ("1h", 3_600_000), ("1m", 60_000)];
/// How many signed nonces the bot remembers to tell its fills apart: more than can be in flight.
pub(super) const SIGNED_NONCES: usize = 64;
/// How long after a failed warm-up, in sequenced time, the bot tries again.
const WARM_UP_RETRY_MS: Millis = 5_000;
/// The provenance of a marks bucket the log itself derived, the only kind the bot warms up from.
const LOG_DERIVED: &str = "log_derived";

/// A future that completes on SIGINT or SIGTERM.
/// Error cases:
/// - a signal handler cannot be installed
pub fn stop_signal() -> anyhow::Result<impl Future<Output = ()>> {
    let mut interrupt = signal(SignalKind::interrupt()).context("SIGINT handler")?;
    let mut terminate = signal(SignalKind::terminate()).context("SIGTERM handler")?;
    Ok(async move {
        tokio::select! {
            _ = interrupt.recv() => {}
            _ = terminate.recv() => {}
        }
    })
}

/// The bot for `cfg`'s account, with its position and balance, and the log from the seq after the
/// market read.
/// Error cases:
/// - the node cannot be reached, or an answer does not verify, or is refused
/// - the market is not listed, or is one the bot does not trade
/// - the account has never been credited
pub(super) async fn start(cfg: &Config, key: AccountKey) -> anyhow::Result<(Bot, Stream)> {
    let node_cfg = &cfg.node;
    let tls = Tls::new(node_cfg.url.clone(), node_cfg.tls_key_sha256)?;
    let deployment = Deployment { chain_id: node_cfg.chain_id, vault: node_cfg.vault.0 };
    let node = Node::new(tls.clone(), node_cfg.signer.0, deployment, node_cfg.request_timeout);
    let strategy_cfg = &cfg.strategy;
    let account = key.address();

    let markets = node.markets().await?;
    let head_seq = markets.head_seq;
    let market = markets
        .body
        .into_iter()
        .find(|m| m.spec.symbol == strategy_cfg.market)
        .with_context(|| format!("the node lists no market {}", strategy_cfg.market))?;
    supported(&market.spec)?;
    let market_id = market.spec.id;

    let view = match node.account(account).await {
        Ok(view) => view,
        Err(CallError::Refused { status: 404, .. }) => {
            bail!("the node has never credited {account}: fund and declare it first")
        }
        Err(e) => return Err(e.into()),
    };
    let (now, view) = (view.time_ms, view.body);
    let position = view.positions.get(&market_id).map_or(Fixed::ZERO, |p| p.qty);
    info!(%account, market = market_id, %position, balance = %view.balance, head_seq, "account read");

    let bar_ms = Millis::try_from(strategy_cfg.bar.as_millis()).context("bar length")?;
    let now = Millis::try_from(now).context("node time")?;
    let bot = Bot {
        domain: eip712::domain(node_cfg.chain_id, node_cfg.vault),
        node,
        key,
        market: market_id,
        qty: strategy_cfg.qty,
        max_slippage_bps: strategy_cfg.max_slippage_bps,
        close_on_stop: cfg.account.close_on_stop,
        marks: IndexMarks::new(market.spec.price_rules),
        bar_ms,
        history_bars: i64::try_from(strategy_cfg.slow).context("slow period")?,
        bars: None,
        warm_up_after: Millis::MIN,
        strategy: SmaCrossover::new(strategy_cfg.fast, strategy_cfg.slow)?,
        close_first: false,
        signed_nonces: VecDeque::with_capacity(SIGNED_NONCES),
        position,
        balance: view.balance,
        mark: market.mark,
        order_expires_at: None,
        // An order an earlier process signed before this start expires by now plus its window.
        trading_from: now.saturating_add(orders::EXPIRES_IN_MS),
        log_ts: Millis::MIN,
    };
    let stream = Stream::new(tls, node_cfg.signer.0, deployment, node_cfg.stream_timeout, head_seq + 1);
    Ok((bot, stream))
}

impl Bot {
    /// Once the index the bot derives is exact at `ts`, warms the strategy up from the node's marks
    /// history through `node_time` and starts the bars from it: bars before the current one from
    /// the history, and the current one from the history's buckets in its window. Reading the
    /// history only then means no move the bot's index missed is in a bar. Only buckets the log
    /// derived count; a bucket seeded from a venue's candles is not the engine's index. Returns
    /// whether the bars have started; a failed read is tried again `WARM_UP_RETRY_MS` later.
    /// Error cases:
    /// - an answer signed by another key than the signer
    pub(super) async fn warm_up(&mut self, ts: Millis, node_time: Millis) -> Result<bool, Halt> {
        if !self.marks.is_complete(ts) || ts < self.warm_up_after {
            return Ok(false);
        }
        let bar_ms = self.bar_ms;
        // Bars that end by `live_from` come from the history.
        let live_from = BarBuilder::new(bar_ms, Millis::MIN).window_start(node_time.max(ts));
        let history_from = live_from - bar_ms * self.history_bars;
        // The widest buckets that tile a bar, which the node keeps longest; every bar is whole minutes.
        let (resolution, _) =
            RESOLUTIONS.into_iter().find(|(_, width)| bar_ms % width == 0).expect("bars are whole minutes");
        let buckets = match self.node.marks(self.market, resolution, history_from, node_time.max(ts)).await {
            Ok(buckets) => buckets,
            Err(CallError::WrongSigner { .. }) => return Err(Halt::WrongSigner),
            Err(e) => {
                warn!("the marks history was not read; trying again: {e}");
                self.warm_up_after = ts.saturating_add(WARM_UP_RETRY_MS);
                return Ok(false);
            }
        };
        let (logged, seeded): (Vec<&MarkBucket>, Vec<&MarkBucket>) =
            buckets.iter().partition(|bucket| bucket.provenance == LOG_DERIVED);
        let (past, current): (Vec<&MarkBucket>, Vec<&MarkBucket>) =
            logged.into_iter().partition(|bucket| bucket.ts < live_from);
        let mut history = BarBuilder::new(bar_ms, Millis::MIN);
        let mut bars = 0;
        let closes = past.iter().map(|bucket| (bucket.ts, Some(bucket.close))).chain([(live_from, None)]);
        for (at, close) in closes {
            if let Some(bar) = history.on_command(at, close) {
                self.strategy.on_close(bar.close);
                bars += 1;
            }
        }
        let mut live = BarBuilder::new(bar_ms, live_from);
        current.iter().for_each(|bucket| _ = live.on_command(bucket.ts, Some(bucket.close)));
        self.bars = Some(live);
        let (fast, slow) = self.strategy.averages();
        info!(bars, seeded = seeded.len(), resolution, live_from, ?fast, ?slow, "warmed up from the marks history");
        Ok(true)
    }
}
