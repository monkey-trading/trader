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
use trader_core::controls::{Account, Controls};
use trader_core::hours::GapTracker;
use trader_core::marks::IndexMarks;
use trader_core::strategies::is_valid_close;

use crate::bot::day::{Saved, StateFile, holding};
use crate::bot::fills::other_market;
use crate::bot::{Bot, Halt, orders, supported};
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
pub(super) async fn start(cfg: &Config, key: AccountKey) -> anyhow::Result<(Bot, Stream)> {
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
    let (now, account_seq, view) = (view.time_ms, view.head_seq, view.body);
    if let Some(other) = other_market(&view, market_id) {
        bail!(
            "{account} holds a position in market {other}: close it first, as this bot trades one market per account"
        );
    }
    let held = view.positions.get(&market_id);
    let position = held.map_or(Fixed::ZERO, |p| p.qty);
    info!(%account, market = market_id, %position, balance = %view.balance, head_seq, "account read");

    let bar_ms = Millis::try_from(strategy_cfg.bar.as_millis()).context("bar length")?;
    let now = Millis::try_from(now).context("node time")?;
    let strategy = strategy_cfg.params.build()?;
    let state_file = StateFile::new(&cfg.account.state_dir, account);
    let saved = state_file.load().await?;
    let mut controls = Controls::new(cfg.risk.clone(), cfg.sizing.clone(), saved.day)?;
    controls.check_market(&market.spec).with_context(|| format!("sizing for {}", strategy_cfg.market))?;
    // A position keeps the opening time saved for it, if it is on the side saved. One with none, as when
    // a crash came between its fill and the save, is held from now: the hold is the conservative guess.
    let opened = (!position.is_zero()).then(|| match saved.opened.get(&market_id) {
        Some(at) if saved.side == position.signum() => *at,
        _ => now,
    });
    controls.restore_opened(opened.map(|at| (market_id, at)).into_iter().collect());
    // The day's opening equity is the account's now, unless a restart finds today's already kept; the
    // markets and the account must have been read at the same point of the log to be valued together.
    if let (Some(mark), true) = (market.mark, account_seq == head_seq) {
        let entry = held.map_or(Fixed::ZERO, |p| p.entry);
        let holding = held.and_then(|p| holding(&market.spec, market_id, p.qty, entry, mark));
        let holdings = holding.as_slice();
        controls.observe(now, &Account { balance: view.balance, holdings });
    }
    let saved_state = Saved {
        day: controls.day_state(),
        opened: controls.opened().clone(),
        side: position.signum(),
        split_at: saved.split_at,
    };
    state_file.save(saved_state.clone()).await.context("keeping the saved state")?;
    let bot = Bot {
        domain: eip712::domain(node_cfg.chain_id, node_cfg.vault),
        node,
        key,
        market: market_id,
        marks: IndexMarks::scheduled(market.spec.price_rules, market.spec.schedule.clone()),
        spec: market.spec,
        max_slippage_bps: strategy_cfg.max_slippage_bps,
        close_on_stop: cfg.account.close_on_stop,
        bar_ms,
        history_bars: i64::try_from(strategy_cfg.params.warm_up_bars()).context("warm-up bars")?,
        bars: None,
        warm_up_after: Millis::MIN,
        strategy,
        controls,
        gaps: GapTracker::new(),
        state_file,
        saved_state,
        state_unsaved: false,
        loss_alert: false,
        flatten_after: Millis::MIN,
        cash_alert: false,
        account_behind: false,
        read_retry_after: Millis::MIN,
        split_at: saved.split_at,
        close_first: false,
        signed_orders: VecDeque::with_capacity(SIGNED_ORDERS),
        position,
        entry: held.map_or(Fixed::ZERO, |p| p.entry),
        balance: view.balance,
        mark: market.mark,
        exact_mark: None,
        exact_from: None,
        pending_moves: Vec::new(),
        order_expires_at: None,
        // An order an earlier process signed before this start expires by now plus its window.
        trading_from: now.saturating_add(orders::EXPIRES_IN_MS),
        log_ts: Millis::MIN,
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
        // History from before a split the bot halted at is in the units before it.
        let history_from = (live_from - bar_ms * self.history_bars).max(self.split_at.unwrap_or(Millis::MIN));
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
                self.gaps.on_bar(&self.spec.schedule, bar.start, bar_ms);
                if is_valid_close(bar.close) {
                    self.strategy.on_close(bar.close);
                }
                bars += 1;
            }
        }
        info!(bars, seeded = seeded.len(), resolution, live_from, "warmed up from the marks history");
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
        Ok(())
    }
}
