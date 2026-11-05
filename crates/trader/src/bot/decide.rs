//! What the fleet does with a completed bar for each account that reads it: the shared strategy gives
//! a target, and the account's controls (sizing, risk limits, margin, hours) turn it into an order or
//! into none, from a read of the account that is of the log. Also the daily loss limit, which closes
//! an account's positions and stops it for the day, and what the fleet keeps of each account in its
//! state file.

use std::collections::BTreeMap;

use engine_core::Fixed;
use engine_core::types::Millis;
use tracing::{debug, info, warn};
use trader_core::bars::Bar;
use trader_core::controls::{Account as Controlled, BarTarget, Decision, Refusal};
use trader_core::margin::Holding;
use trader_core::risk::DayStatus;
use trader_core::strategies::is_valid_close;
use trader_core::strategy::Target;

use crate::bot::day::{Saved, SavedPosition};
use crate::bot::fleet::{Assignment, Fleet, MarketFeed, Pending, READ_EVERY_MS, READ_RETRY_MS, REPLAY_LAG_MS, Wanted};
use crate::node::api::AccountView;

/// The most holdings one account has: one for each of the markets it may trade.
const MAX_HOLDINGS: usize = 16;

/// The account's open positions at their markets' marks, in `buf`; how many, or `None` when a market
/// the account holds has no mark yet, so the account cannot be valued.
fn holdings_of<'f>(
    assignments: &'f [Assignment],
    range: std::ops::Range<u32>,
    markets: &'f [MarketFeed],
    buf: &mut [Option<Holding<'f>>; MAX_HOLDINGS],
) -> Option<usize> {
    let mut n = 0;
    for asg in &assignments[range.start as usize..range.end as usize] {
        if asg.position.is_zero() {
            continue;
        }
        let feed = &markets[asg.market_slot as usize];
        buf[n] = Some(Holding {
            market: asg.market,
            qty: asg.position,
            entry: asg.entry,
            mark: feed.mark?,
            spec: &feed.spec,
        });
        n += 1;
    }
    Some(n)
}

/// `buf` as an array with no gaps, if it holds anything: the first holding fills what follows the
/// holdings, which the callers slice off.
fn filled<'f>(buf: &[Option<Holding<'f>>; MAX_HOLDINGS]) -> Option<[Holding<'f>; MAX_HOLDINGS]> {
    let first = buf[0]?;
    Some(std::array::from_fn(|i| buf[i].unwrap_or(first)))
}

impl Fleet {
    /// Hands a completed bar to the strategies that read it, and each account that trades on them.
    /// A bar the stream is replaying from history places no order: `lag` is past `REPLAY_LAG_MS`.
    pub(super) fn bar_closed(&mut self, series: u32, bar: Bar, ts: Millis, lag: Millis) {
        let state = &mut self.series[series as usize];
        let feed = &self.markets[state.market_slot as usize];
        let gap = state.gaps.on_bar(&feed.spec.schedule, bar.start, state.length);
        let valid = is_valid_close(bar.close);
        if !valid {
            warn!(market = feed.id, start = bar.start, close = %bar.close, "the bar's close is not a mark the strategies take: no data");
        }
        for shared in &mut state.strategies {
            shared.target = if valid { shared.strategy.on_close(bar.close) } else { None };
            debug!(market = feed.id, start = bar.start, close = %bar.close, target = ?shared.target, gap, lag, "bar");
        }
        for k in 0..state.assignments.len() {
            let a = self.series[series as usize].assignments[k];
            let target = self.series[series as usize].strategies[self.assignments[a as usize].strategy as usize].target;
            if let Some(target) = target {
                self.want(a, Wanted { target, bar_start: bar.start, ts, lag, gap });
            }
        }
    }

    /// Notes that assignment `a` wants to move toward `wanted.target`, and asks for a read of its
    /// account to decide it from, unless the position the log gives is already there.
    fn want(&mut self, a: u32, wanted: Wanted) {
        let asg = &mut self.assignments[a as usize];
        asg.wanted = None;
        let at_target = matches!(
            (wanted.target, asg.position.signum()),
            (Target::Long, 1) | (Target::Short, -1) | (Target::Flat, 0)
        );
        let account = asg.account;
        // A newer bar's target replaces an order queued for an older one, whether it asks for a move or not.
        if at_target {
            asg.pending = None;
        }
        if self.accounts[account as usize].failed.is_some() || at_target {
            return;
        }
        asg.wanted = Some(wanted);
        self.want_read(account);
    }

    /// Asks for a read of `account`; one already out is out of date for what is wanted now.
    pub fn want_read(&mut self, account: u32) {
        let entry = &mut self.accounts[account as usize];
        if entry.failed.is_some() {
            return;
        }
        entry.read_stale |= entry.reading;
        if !entry.read_wanted {
            entry.read_wanted = true;
            self.reads.push_back(account);
        }
    }

    /// Takes a read of `account` as the node answered it. A read that holds the positions the log
    /// gives is of the log: its balance and entries are used. One that does not is behind it (or
    /// ahead), and gives neither; nothing is valued or ordered from the account until a later read
    /// agrees. Returns whether it was of the log.
    pub fn apply_view(&mut self, account: u32, view: &AccountView) -> bool {
        let range = self.accounts[account as usize].assignments.clone();
        if let Some((market, _)) =
            view.positions.iter().find(|(id, p)| !p.qty.is_zero() && self.assignment_of(account, **id).is_none())
        {
            let reason =
                format!("the account holds a position in market {market}, which this bot does not trade or limit");
            self.fail(account, reason);
            return false;
        }
        let at_log = range.clone().all(|a| {
            let asg = &self.assignments[a as usize];
            view.positions.get(&asg.market).map_or(Fixed::ZERO, |p| p.qty) == asg.position
        });
        let log_ts = self.log_ts;
        let entry = &mut self.accounts[account as usize];
        if !at_log {
            entry.behind = true;
            entry.read_after = log_ts.saturating_add(READ_RETRY_MS);
            debug!(account = %entry.address, "the account read is not at the log");
            return false;
        }
        entry.behind = false;
        entry.read_after = log_ts.saturating_add(READ_EVERY_MS);
        entry.balance = view.balance;
        for a in range {
            let asg = &mut self.assignments[a as usize];
            asg.entry = view.positions.get(&asg.market).map_or(Fixed::ZERO, |p| p.entry);
        }
        self.touch_state(account);
        true
    }

    /// Marks that a read of `account` failed or was not of the log, to be tried again soon.
    pub fn read_failed(&mut self, account: u32) {
        let log_ts = self.log_ts;
        let entry = &mut self.accounts[account as usize];
        entry.behind = true;
        entry.read_after = log_ts.saturating_add(READ_RETRY_MS);
    }

    /// Decides each target `account` wants, from the read of it that has just been taken.
    pub fn decide_account(&mut self, account: u32) {
        for a in self.accounts[account as usize].assignments.clone() {
            if let Some(wanted) = self.assignments[a as usize].wanted.take() {
                self.decide_one(a, wanted);
            }
        }
    }

    /// Turns one target into an order, as the account's controls allow.
    fn decide_one(&mut self, a: u32, w: Wanted) {
        let asg = &self.assignments[a as usize];
        let (idx, market) = (asg.account, asg.market);
        let acc = &self.accounts[idx as usize];
        if acc.failed.is_some() {
            return;
        }
        // A target restored after a wait for a read can be past the lag orders may go at: not decided.
        if self.log_ts.saturating_sub(w.ts) > REPLAY_LAG_MS {
            debug!(account = %acc.address, market, "the target is history by now: no order");
            return;
        }
        // The account's limits are checked against the positions the log gives, so an order of another
        // of its markets that is queued, or may not be in the log yet, is waited for: the target is
        // decided again when that order is answered or its fill is read.
        if self.other_order_out(a, w.ts) {
            self.assignments[a as usize].wanted = Some(w);
            return;
        }
        if acc.behind || acc.state_unsaved {
            info!(account = %acc.address, market, target = ?w.target, behind = acc.behind, "the account read is not at the log, or the saved state is not written: no order");
            if acc.state_unsaved {
                // Written again now, as at any change, and the target is decided once it is.
                self.assignments[a as usize].wanted = Some(w);
                let acc = &mut self.accounts[idx as usize];
                acc.save_retry_at = None;
                if !acc.state_dirty {
                    acc.state_dirty = true;
                    self.saves.push(idx);
                }
            }
            return;
        }
        let feed = &self.markets[asg.market_slot as usize];
        let Some(mark) = feed.mark else { return };
        let mut buf = [None; MAX_HOLDINGS];
        let Some(n) = holdings_of(&self.assignments, acc.assignments.clone(), &self.markets, &mut buf) else {
            info!(account = %acc.address, market, "a holding has no mark yet, so the account cannot be valued: no order");
            return;
        };
        let held = filled(&buf);
        let holdings: &[Holding<'_>] = held.as_ref().map_or(&[], |held| &held[..n]);
        let bar = BarTarget {
            ts: w.ts,
            market,
            spec: &feed.spec,
            mark,
            target: w.target,
            gap: w.gap,
            close_first: asg.close_first,
            max_slippage_bps: asg.max_slippage_bps,
        };
        let acc = &mut self.accounts[idx as usize];
        let decision = acc.controls.decide(&Controlled { balance: acc.balance, holdings }, &bar);
        // The decision may have tripped the daily limit: that is kept before anything is done about it.
        self.touch_state(idx);
        self.track_cash_alert(idx, &decision);
        let address = self.accounts[idx as usize].address;
        match decision {
            Decision::AtTarget => {}
            Decision::Refused(refusal) => match refusal {
                Refusal::DailyLossStopped | Refusal::MarketClosed => {
                    debug!(%address, market, target = ?w.target, ?refusal, "no order")
                }
                _ => info!(%address, market, target = ?w.target, ?refusal, "no order"),
            },
            Decision::Order(_) | Decision::Flatten if w.lag > REPLAY_LAG_MS => {
                info!(%address, market, target = ?w.target, lag = w.lag, "the bar is history the stream is replaying: no order");
            }
            Decision::Flatten => self.request_flatten(idx),
            Decision::Order(intent) => {
                if let Some(reason) = self.order_in_flight(a, w.ts) {
                    info!(%address, market, target = ?w.target, "{reason}: no order");
                    return;
                }
                let epoch = self.accounts[idx as usize].epoch;
                let asg = &mut self.assignments[a as usize];
                asg.pending = Some(Pending { intent, epoch, wanted: w });
                self.decisions += 1;
                if !asg.queued {
                    asg.queued = true;
                    self.queue.push_back(a);
                }
            }
        }
    }

    /// Raises the cash alert on the first refusal for cash that is not positive, and resolves it on
    /// the first decision after cash is positive again.
    fn track_cash_alert(&mut self, account: u32, decision: &Decision) {
        let acc = &mut self.accounts[account as usize];
        let no_cash = matches!(decision, Decision::Refused(Refusal::NoCash));
        if no_cash && !acc.cash_alert {
            acc.cash_alert = true;
            warn!(alert = "cash_not_positive", account = %acc.address, balance = %acc.balance, "cash is not positive: no order adds to a position, and an account with negative cash at a MONKEY period's close earns no entitlement");
        } else if !no_cash && acc.cash_alert && acc.balance.is_positive() {
            acc.cash_alert = false;
            info!(alert_resolved = "cash_not_positive", account = %acc.address, balance = %acc.balance, "cash is positive again");
        }
    }

    /// Whether an order of another market of assignment `a`'s account is queued or may still enter the log.
    fn other_order_out(&self, a: u32, ts: Millis) -> bool {
        let account = &self.accounts[self.assignments[a as usize].account as usize];
        account.assignments.clone().filter(|&b| b != a).any(|b| {
            let other = &self.assignments[b as usize];
            other.pending.is_some() || other.expires_at.is_some_and(|expires_at| ts <= expires_at)
        })
    }

    /// Drops the order assignment `a` has queued, and asks for a read if a target of its account waits.
    /// With `again`, its own target waits too, to be decided on the account as it now is.
    pub fn drop_pending(&mut self, a: u32, again: bool) {
        let asg = &mut self.assignments[a as usize];
        if let (true, Some(pending), None) = (again, asg.pending, asg.wanted) {
            asg.wanted = Some(pending.wanted);
        }
        (asg.pending, asg.queued) = (None, false);
        let account = asg.account;
        self.read_if_waiting(account);
    }

    /// Asks for a read of `account` if one of its assignments has a target waiting to be decided.
    pub(super) fn read_if_waiting(&mut self, account: u32) {
        let range = self.accounts[account as usize].assignments.clone();
        if range.into_iter().any(|b| self.assignments[b as usize].wanted.is_some()) {
            self.want_read(account);
        }
    }

    /// Why no order is signed for assignment `a` at the command at `ts`, if an earlier one may still
    /// enter the log and move its position: the last this process signed for it, or one an earlier
    /// process signed before this start.
    pub(super) fn order_in_flight(&self, a: u32, ts: Millis) -> Option<&'static str> {
        if self.assignments[a as usize].expires_at.is_some_and(|expires_at| ts <= expires_at) {
            return Some("the last order may still enter the log");
        }
        (ts <= self.trading_from).then_some("an order signed before this start may still enter the log")
    }

    /// Reads the equity of each account that holds a position in market `slot`, as its mark moves, so
    /// a loss is caught when it happens and not at the next bar.
    pub(super) fn observe_market(&mut self, slot: usize, ts: Millis) {
        for k in 0..self.markets[slot].assignments.len() {
            let a = self.markets[slot].assignments[k];
            let asg = &self.assignments[a as usize];
            if !asg.position.is_zero() {
                self.observe_account(asg.account, ts);
            }
        }
    }

    /// Reads `account`'s equity at the sequenced time `ts`, while its balance and entries are those of
    /// the positions the log gives, and keeps the day's state.
    pub fn observe_account(&mut self, account: u32, ts: Millis) {
        let acc = &self.accounts[account as usize];
        if acc.behind || acc.failed.is_some() {
            return;
        }
        let mut buf = [None; MAX_HOLDINGS];
        let Some(n) = holdings_of(&self.assignments, acc.assignments.clone(), &self.markets, &mut buf) else { return };
        let held = filled(&buf);
        let holdings: &[Holding<'_>] = held.as_ref().map_or(&[], |held| &held[..n]);
        let acc = &mut self.accounts[account as usize];
        let Some(status) = acc.controls.observe(ts, &Controlled { balance: acc.balance, holdings }) else {
            return;
        };
        match status {
            DayStatus::Stopped { newly } => {
                if newly || !acc.loss_alert {
                    let state = acc.controls.day_state();
                    warn!(alert = "daily_loss_limit", account = %acc.address, ?state, "the daily loss limit is reached: flattening and stopping until the next UTC day");
                }
                acc.loss_alert = true;
                // The close is asked for by `retry_flattens`, behind the replay lag: on this command when a mark move
                // observed the loss, at the next when a read did.
                if !self.stopped.contains(&account) {
                    self.stopped.push(account);
                }
            }
            DayStatus::Trading if acc.loss_alert => {
                acc.loss_alert = false;
                info!(alert_resolved = "daily_loss_limit", account = %acc.address, "a new UTC day: trading again");
            }
            DayStatus::Trading => {}
        }
        self.touch_state(account);
    }

    /// Asks the runner to close `account`'s positions, once.
    fn request_flatten(&mut self, account: u32) {
        let acc = &mut self.accounts[account as usize];
        // Not again until the time after the last try, whatever asks.
        if !acc.flatten_queued && self.log_ts >= acc.flatten_after {
            acc.flatten_queued = true;
            self.flattens.push_back(account);
        }
    }

    /// While the daily loss limit has stopped an account and it still holds a position, asks for it
    /// to be closed again on any command at least `FLATTEN_RETRY_MS` of sequenced time after the last
    /// try: a close the engine declined does not wait for a move of the mark or the next bar.
    pub(super) fn retry_flattens(&mut self, lag: Millis) {
        let mut i = 0;
        while i < self.stopped.len() {
            let account = self.stopped[i];
            let acc = &mut self.accounts[account as usize];
            if !acc.controls.is_stopped() || acc.failed.is_some() {
                self.stopped.swap_remove(i);
                continue;
            }
            i += 1;
            let holds = self.assignments[acc.assignments.start as usize..acc.assignments.end as usize]
                .iter()
                .any(|asg| !asg.position.is_zero());
            if holds && lag <= REPLAY_LAG_MS && !acc.behind {
                self.request_flatten(account);
            }
        }
    }

    /// Asks for a read of each account that is due one.
    pub(super) fn scan(&mut self, ts: Millis) {
        for account in 0..self.accounts.len() {
            let acc = &self.accounts[account];
            if acc.failed.is_some() || acc.read_wanted || acc.reading || ts < acc.read_after {
                continue;
            }
            // Every account is read now and then, whether or not it holds a position: its balance
            // moves with deposits and withdrawals, and a position in a market the bot does not trade
            // is found.
            self.want_read(account as u32);
        }
    }

    /// What the account's controls and positions say should be in its state file.
    pub fn saved_for(&self, account: u32) -> Saved {
        let acc = &self.accounts[account as usize];
        let mut positions = BTreeMap::new();
        for asg in &self.assignments[acc.assignments.start as usize..acc.assignments.end as usize] {
            if !asg.position.is_zero() {
                positions.insert(asg.market, SavedPosition { side: asg.position.signum(), entry: asg.entry });
            }
        }
        let mut splits = acc.saved.splits.clone();
        for asg in &self.assignments[acc.assignments.start as usize..acc.assignments.end as usize] {
            if let Some(at) = self.markets[asg.market_slot as usize].split_at {
                splits.insert(asg.market, at);
            }
        }
        Saved {
            day: acc.controls.day_state(),
            opened: acc.controls.opened().clone(),
            positions,
            splits,
            ..Saved::default()
        }
    }

    /// Notes that `account`'s state file is out of date, if it is. Nothing is ordered for the account
    /// until it is written.
    pub fn touch_state(&mut self, account: u32) {
        let acc = &self.accounts[account as usize];
        let saved = &acc.saved;
        let mut same = acc.controls.day_state() == saved.day && acc.controls.opened() == &saved.opened;
        let mut open = 0;
        for asg in &self.assignments[acc.assignments.start as usize..acc.assignments.end as usize] {
            let now = (!asg.position.is_zero()).then_some((asg.position.signum(), asg.entry));
            open += usize::from(now.is_some());
            let kept = saved.positions.get(&asg.market).map(|p| (p.side, p.entry));
            same &= now == kept
                && self.markets[asg.market_slot as usize].split_at == saved.splits.get(&asg.market).copied();
        }
        same &= open == saved.positions.len();
        if !same && !acc.state_dirty {
            self.accounts[account as usize].state_dirty = true;
            self.saves.push(account);
        }
    }
}
