//! Warming each series up from the node's marks history. Once a market's index is exact, and the
//! node's own lag in writing marks to its history has passed, the fleet asks for the history of each
//! bar length some account trades, and builds that series' bars from it: the bars before the one
//! the index became exact in come from the history, that one starts from the history's bucket in its
//! window, and the moves the fleet kept meanwhile follow. Only buckets the log derived count; a
//! bucket seeded from a venue's candles is not the engine's index.

use engine_core::Fixed;
use engine_core::types::{MarketId, Millis};
use tracing::{info, warn};
use trader_core::bars::BarBuilder;
use trader_core::strategies::{after_unit_break, is_valid_close};

use crate::bot::fleet::{Fleet, Warm};
use crate::node::api::MarkBucket;

/// The marks history's resolutions, widest first, with their bucket widths.
const RESOLUTIONS: [(&str, Millis); 3] = [("1d", 86_400_000), ("1h", 3_600_000), ("1m", 60_000)];
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

/// A read of a market's marks history that a series wants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WarmRequest {
    pub series: u32,
    pub generation: u32,
    pub market: MarketId,
    pub resolution: &'static str,
    pub from: Millis,
    pub to: Millis,
}

impl Fleet {
    /// Before a market's series have warmed up: from the first command at which its index is exact,
    /// keeps its moves, and `HISTORY_LAG_MS` later asks for the history each series needs.
    pub(super) fn note_for_warm_up(&mut self, slot: usize, ts: Millis, moved: Option<Fixed>, node_time: Millis) {
        let feed = &mut self.markets[slot];
        if !feed.marks.is_complete(ts) {
            return;
        }
        let exact_from = *feed.exact_from.get_or_insert(ts);
        if let Some(mark) = moved {
            if feed.pending_moves.len() == PENDING_MOVES {
                warn!(market = feed.id, "too many moves before the warm-up; starting it again");
                feed.pending_moves.clear();
                feed.exact_from = Some(ts);
                for &series in &feed.series {
                    let state = &mut self.series[series as usize];
                    if state.warm == Warm::Requested {
                        state.warm = Warm::Waiting;
                    }
                }
                self.markets[slot].pending_moves.push((ts, mark));
                return;
            }
            feed.pending_moves.push((ts, mark));
        }
        if ts < exact_from.saturating_add(HISTORY_LAG_MS) {
            return;
        }
        for k in 0..self.markets[slot].series.len() {
            let series = self.markets[slot].series[k];
            let state = &mut self.series[series as usize];
            let ready = match state.warm {
                Warm::Waiting => true,
                Warm::RetryAt(at) => ts >= at,
                Warm::Requested | Warm::Done => false,
            };
            if !ready {
                continue;
            }
            state.warm = Warm::Requested;
            state.generation += 1;
            let generation = state.generation;
            let length = state.length;
            let live_from = exact_from - exact_from.rem_euclid(length);
            // The widest buckets that tile a bar, which the node keeps longest; every bar is whole minutes.
            let (resolution, _) =
                RESOLUTIONS.into_iter().find(|(_, width)| length % width == 0).expect("bars are whole minutes");
            let history_bars = Millis::try_from(state.history_bars).unwrap_or(Millis::MAX);
            self.warm_requests.push(WarmRequest {
                series,
                generation,
                market: self.markets[slot].id,
                resolution,
                // History from before a split is in the units before it.
                from: live_from
                    .saturating_sub(length.saturating_mul(history_bars))
                    .max(self.markets[slot].split_at.unwrap_or(Millis::MIN)),
                to: node_time.max(ts),
            });
        }
    }

    /// Takes the history a warm-up read gave for `series`, read through `node_time`, and starts its
    /// bars: the strategy's averages from the history's bars, then the live bar, which the kept
    /// moves may close and place an order from. A failed read (`None`) is tried again later.
    pub fn warmed(&mut self, series: u32, generation: u32, buckets: Option<&[MarkBucket]>, node_time: Millis) {
        let state = &mut self.series[series as usize];
        if state.warm != Warm::Requested || state.generation != generation {
            return;
        }
        let Some(buckets) = buckets else {
            state.warm = Warm::RetryAt(self.log_ts.saturating_add(WARM_UP_RETRY_MS));
            return;
        };
        let (slot, length) = (state.market_slot as usize, state.length);
        let Some(exact_from) = self.markets[slot].exact_from else { return };
        let live_from = exact_from - exact_from.rem_euclid(length);
        let (logged, seeded): (Vec<&MarkBucket>, Vec<&MarkBucket>) =
            buckets.iter().partition(|bucket| bucket.provenance == LOG_DERIVED);
        let mut history = BarBuilder::new(length, Millis::MIN);
        let mut bars = 0;
        // Only the history since the last split-sized step counts: before it, prices are in other units.
        let before: Vec<&&MarkBucket> = logged.iter().filter(|bucket| bucket.ts < live_from).collect();
        let closes: Vec<Fixed> = before.iter().map(|bucket| bucket.close).collect();
        let past = before[after_unit_break(&closes)..].iter().map(|bucket| (bucket.ts, Some(bucket.close)));
        let schedule = self.markets[slot].spec.schedule.clone();
        for (at, close) in past.chain([(live_from, None)]) {
            if let Some(bar) = history.on_command(at, close) {
                let state = &mut self.series[series as usize];
                state.gaps.on_bar(&schedule, bar.start, length);
                if is_valid_close(bar.close) {
                    for shared in &mut state.strategies {
                        shared.strategy.on_close(bar.close);
                    }
                }
                bars += 1;
            }
        }
        info!(
            market = self.markets[slot].id,
            length,
            bars,
            seeded = seeded.len(),
            live_from,
            "warmed up from the marks history"
        );
        // With no exact value yet, moves start from the node's last mark in the history.
        if self.markets[slot].exact_mark.is_none() {
            self.markets[slot].exact_mark = logged.last().map(|bucket| bucket.close);
        }
        let mut live = BarBuilder::new(length, live_from);
        for bucket in logged.iter().filter(|bucket| bucket.ts < live_from + length) {
            live.on_command(bucket.ts, Some(bucket.close));
        }
        // The first exact value is a move when the history does not end there.
        let mut moves: Vec<(Millis, Fixed)> = self.markets[slot]
            .first_exact
            .filter(|(at, mark)| {
                // What the history held when the index first became exact, not what came after.
                let before = logged.iter().rev().find(|bucket| bucket.ts <= *at);
                before.is_none_or(|bucket| bucket.close != *mark)
            })
            .into_iter()
            .collect();
        moves.extend(self.markets[slot].pending_moves.iter().copied());
        for (at, mark) in moves {
            if let Some(bar) = live.on_command(at, Some(mark)) {
                self.bar_closed(series, bar, at, node_time.saturating_sub(at));
            }
        }
        // A bar that time alone completed was completed by some command since its window ended, which
        // was not kept: it counts as completed when its window ended, the earliest it can have been,
        // so it is never fresher than it was.
        let ts = self.log_ts;
        if let Some(bar) = live.on_command(ts, None) {
            let completed = bar.start.saturating_add(length);
            self.bar_closed(series, bar, completed, node_time.saturating_sub(completed));
        }
        let state = &mut self.series[series as usize];
        state.live = Some(live);
        state.warm = Warm::Done;
        for k in 0..self.series[series as usize].assignments.len() {
            let a = self.series[series as usize].assignments[k];
            self.mark_changed(self.assignments[a as usize].account);
        }
        let feed = &mut self.markets[slot];
        feed.unwarmed -= 1;
        if feed.unwarmed == 0 {
            feed.pending_moves = Vec::new();
            feed.first_exact = None;
        }
    }
}
