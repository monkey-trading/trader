//! Changing the parameters of one account's strategy while the bot runs. The new parameters are a
//! strategy of their own in the series, shared with any other assignment that already runs them; the
//! series then warms up again from the node's marks history, as at the start, so every strategy in
//! it is built from the same bars, and it decides nothing until it has.

use std::collections::BTreeSet;

use trader_core::hours::GapTracker;
use trader_core::strategies::StrategyParams;
use trader_core::strategy::ParamsError;

use crate::bot::fleet::{Fleet, Shared, Warm};

impl Fleet {
    /// Runs assignment `a` on `params`, and starts its series' warm-up again. No order of the
    /// assignment is sent until the series has warmed up; an order decided from the old parameters is
    /// dropped.
    /// Error cases:
    /// - the strategy refuses the parameters (see `ParamsError`)
    pub fn set_params(&mut self, a: u32, params: StrategyParams) -> Result<(), ParamsError> {
        let series = self.assignments[a as usize].series;
        let state = &mut self.series[series as usize];
        if state.strategies[self.assignments[a as usize].strategy as usize].params == params {
            return Ok(());
        }
        let shared = match state.strategies.iter().position(|shared| shared.params == params) {
            Some(shared) => shared,
            None => {
                let strategy = params.build()?;
                state.strategies.push(Shared { params, strategy, target: None, changed: false });
                state.strategies.len() - 1
            }
        };
        self.assignments[a as usize].strategy = u32::try_from(shared).unwrap_or(u32::MAX);
        self.drop_unused(series);
        self.rewarm(series);
        Ok(())
    }

    /// Removes the strategies of `series` that no assignment runs any more.
    fn drop_unused(&mut self, series: u32) {
        let state = &mut self.series[series as usize];
        let used: BTreeSet<usize> =
            state.assignments.iter().map(|&b| self.assignments[b as usize].strategy as usize).collect();
        if used.len() == state.strategies.len() {
            return;
        }
        let mut place = vec![0; state.strategies.len()];
        let mut kept = Vec::with_capacity(used.len());
        for (old, shared) in std::mem::take(&mut state.strategies).into_iter().enumerate() {
            if used.contains(&old) {
                place[old] = u32::try_from(kept.len()).unwrap_or(u32::MAX);
                kept.push(shared);
            }
        }
        state.strategies = kept;
        for &b in &state.assignments {
            let asg = &mut self.assignments[b as usize];
            asg.strategy = place[asg.strategy as usize];
        }
    }

    /// Starts `series` over, as at the start: every strategy in it empty, its bars to be built again
    /// from the marks history, and the moves of its market kept from the next exact index on. The
    /// market's other series that are not warm yet start over with it, for they share those moves.
    fn rewarm(&mut self, series: u32) {
        let state = &mut self.series[series as usize];
        for shared in &mut state.strategies {
            // The same parameters built before.
            if let Ok(strategy) = shared.params.build() {
                shared.strategy = strategy;
            }
            (shared.target, shared.changed) = (None, false);
        }
        state.history_bars = state.strategies.iter().map(|shared| shared.params.warm_up_bars()).max().unwrap_or(0);
        state.live = None;
        state.gaps = GapTracker::new();
        let slot = state.market_slot as usize;
        let was_warm = state.warm == Warm::Done;
        state.warm = Warm::Waiting;
        for k in 0..self.series[series as usize].assignments.len() {
            let b = self.series[series as usize].assignments[k];
            let asg = &mut self.assignments[b as usize];
            (asg.wanted, asg.pending) = (None, None);
            // Every account that reads the series stops deciding until it has warmed up.
            let account = asg.account;
            self.mark_changed(account);
        }
        let feed = &mut self.markets[slot];
        if was_warm {
            feed.unwarmed += 1;
        }
        (feed.exact_from, feed.first_exact) = (None, None);
        feed.pending_moves.clear();
        for k in 0..self.markets[slot].series.len() {
            let other = self.markets[slot].series[k];
            if self.series[other as usize].warm == Warm::Requested {
                self.series[other as usize].warm = Warm::Waiting;
            }
        }
    }
}
