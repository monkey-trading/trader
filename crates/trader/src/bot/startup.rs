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

use crate::bot::{Bot, Halt, Link, orders, supported};
use crate::config::Config;
use crate::keys::AccountKey;
use crate::node::api::MarkBucket;
use crate::node::{CallError, Node, Tls};
use crate::stream::Stream;

/// The marks history's resolutions, widest first, with their bucket widths.
const RESOLUTIONS: [(&str, Millis); 3] = [("1d", 86_400_000), ("1h", 3_600_000), ("1m", 60_000)];
/// How many signed orders the bot remembers to tell its fills apart: more than can be in flight.
pub(super) const SIGNED_ORDERS: usize = 64;
/// How long after a failed warm-up, in sequenced time, the bot tries again.
const WARM_UP_RETRY_MS: Millis = 5_000;
/// How long after its index is exact the bot reads the marks history: past the node's own lag in
/// writing marks to it, so the history holds every move before the index was exact.
const HISTORY_LAG_MS: Millis = 5_000;
/// The most moves kept between the index being exact and the warm-up; a mark moves at most once
/// per command, and `HISTORY_LAG_MS` holds far fewer commands.
const PENDING_MOVES: usize = 4096;
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
/// head before the reads.
/// Error cases:
/// - the node cannot be reached, or an answer does not verify, or is refused
/// - the market is not listed, or is one the bot does not trade
/// - the account has never been credited
pub(super) async fn start(cfg: &Config, key: AccountKey, link: Link) -> anyhow::Result<(Bot, Stream)> {
    let node_cfg = &cfg.node;
    let tls = Tls::new(node_cfg.url.clone(), node_cfg.tls_key_sha256)?;
    let deployment = Deployment { chain_id: node_cfg.chain_id, vault: node_cfg.vault.0 };
    let node = Node::new(tls.clone(), node_cfg.signer.0, deployment, node_cfg.request_timeout);
    let strategy_cfg = &cfg.strategy;
    let account = key.address();

    // The log head as it stood before the reads below: their views hold at least that much of the
    // log, so the stream, which starts after it, misses nothing they do not show. What it repeats
    // applies again to the same effect: a fill's position is absolute, and a listing's rules are
    // the same rules.
    let head_seq = node.nonce(account).await?.head_seq;
    let markets = node.markets().await?;
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
    let params = link.params();
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
        history_bars: i64::try_from(params.slow).context("slow period")?,
        bars: None,
        warm_up_after: Millis::MIN,
        strategy: SmaCrossover::new(params.fast, params.slow)?,
        close_first: false,
        signed_orders: VecDeque::with_capacity(SIGNED_ORDERS),
        position,
        balance: view.balance,
        mark: market.mark,
        exact_mark: None,
        exact_from: None,
        pending_moves: Vec::new(),
        order_expires_at: None,
        // An order an earlier process signed before this start expires by now plus its window.
        trading_from: now.saturating_add(orders::EXPIRES_IN_MS),
        log_ts: Millis::MIN,
        link,
    };
    let stream = Stream::new(tls, node_cfg.signer.0, deployment, node_cfg.stream_timeout, head_seq + 1);
    Ok((bot, stream))
}

impl Bot {
    /// Before the bars start: from the first command at which the bot's index is exact, keeps its
    /// moves, and `HISTORY_LAG_MS` later warms the strategy up from the node's marks history,
    /// through `node_time`. Bars before the one the index became exact in come from the history;
    /// that one starts from the history's bucket in its window, which holds the moves before the
    /// index was exact, and the kept moves follow, which may close it and place an order. Only
    /// buckets the log derived count; a bucket seeded from a venue's candles is not the engine's
    /// index. A failed read is tried again `WARM_UP_RETRY_MS` later.
    /// Error cases:
    /// - an answer signed by another key than the signer
    pub(super) async fn warm_up(&mut self, ts: Millis, moved: Option<Fixed>, node_time: Millis) -> Result<(), Halt> {
        if !self.marks.is_complete(ts) {
            return Ok(());
        }
        let exact_from = *self.exact_from.get_or_insert(ts);
        if let Some(mark) = moved {
            if self.pending_moves.len() == PENDING_MOVES {
                warn!("too many moves before the warm-up; starting it again");
                self.pending_moves.clear();
                self.exact_from = Some(ts);
            }
            self.pending_moves.push((ts, mark));
        }
        if ts < exact_from.saturating_add(HISTORY_LAG_MS) || ts < self.warm_up_after {
            return Ok(());
        }
        let bar_ms = self.bar_ms;
        let live_from = BarBuilder::new(bar_ms, Millis::MIN).window_start(exact_from);
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
                return Ok(());
            }
        };
        let (logged, seeded): (Vec<&MarkBucket>, Vec<&MarkBucket>) =
            buckets.iter().partition(|bucket| bucket.provenance == LOG_DERIVED);
        let mut history = BarBuilder::new(bar_ms, Millis::MIN);
        let mut bars = 0;
        let past = logged.iter().filter(|bucket| bucket.ts < live_from).map(|bucket| (bucket.ts, Some(bucket.close)));
        for (at, close) in past.chain([(live_from, None)]) {
            if let Some(bar) = history.on_command(at, close) {
                self.strategy.on_close(bar.close);
                bars += 1;
            }
        }
        let (fast, slow) = self.strategy.averages();
        info!(bars, seeded = seeded.len(), resolution, live_from, ?fast, ?slow, "warmed up from the marks history");
        // With no exact value yet, moves start from the node's last mark in the history.
        if self.exact_mark.is_none() {
            self.exact_mark = logged.last().map(|bucket| bucket.close);
        }
        let mut live = BarBuilder::new(bar_ms, live_from);
        for bucket in logged.iter().filter(|bucket| bucket.ts < live_from + bar_ms) {
            live.on_command(bucket.ts, Some(bucket.close));
        }
        for (at, mark) in std::mem::take(&mut self.pending_moves) {
            if let Some(bar) = live.on_command(at, Some(mark)) {
                self.on_bar(bar, at, node_time.saturating_sub(at)).await?;
            }
        }
        if let Some(bar) = live.on_command(ts, None) {
            self.on_bar(bar, ts, node_time.saturating_sub(ts)).await?;
        }
        self.bars = Some(live);
        self.publish();
        Ok(())
    }
}
