//! Keeping the bot's requests under the node's limits, and its idea of the node's time.
//!
//! The pacer lets a request go only when the node's limits allow it: one bucket for every request
//! (the node's `per_ip`), one per account for the requests that act for it (`per_account`), and one
//! for declarations (the node's fresh-account budget for this client, `fresh_accounts`), each a
//! token bucket in virtual-scheduling form with one request of its burst kept in reserve to absorb
//! timing differences. A request that may not go yet is told how long to wait and asks again, so a
//! hold the node asks for (a retry-later answer) reaches every waiting request, and a burst of orders
//! is signed one by one at the rate the node allows, in the order it is asked for, instead of failing.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use engine_core::types::AccountId;
use tokio::time::Instant;

use crate::config::{LimitsSection, MinuteRateSection, RateSection};

/// The longest a retry-later answer may hold the bot back, whatever the node says.
const MAX_BACK_OFF: Duration = Duration::from_secs(60);
/// The most accounts the pacer keeps a bucket for before it forgets the refilled ones.
const FORGET_ABOVE: usize = 4096;

/// One token bucket as the instant it is next full of its allowance: `tat`, the theoretical arrival
/// time of the virtual scheduling algorithm.
#[derive(Clone, Copy)]
struct Bucket {
    tat: Instant,
}

/// A bucket's shape: the time one request costs, and how much earlier than `tat` a request may go.
#[derive(Clone, Copy)]
struct Shape {
    cost: Duration,
    allowance: Duration,
}

struct State {
    ip: Bucket,
    accounts: HashMap<AccountId, Bucket>,
    setups: Bucket,
    blocked_until: Instant,
}

/// The request pacer of one node.
pub struct Pacer {
    ip: Shape,
    account: Shape,
    setup: Shape,
    state: Mutex<State>,
}

impl Shape {
    /// The bucket `rate` names, with one request of its burst kept in reserve.
    fn of(rate: RateSection) -> Self {
        Self::with(Duration::from_secs(1) / rate.per_second.max(1), rate.burst)
    }

    /// The bucket a per-minute `rate` names, likewise.
    fn per_minute(rate: MinuteRateSection) -> Self {
        Self::with(Duration::from_secs(60) / rate.per_minute.max(1), rate.burst)
    }

    /// A bucket of requests that cost `cost` each, and `burst` at once with one kept in reserve.
    fn with(cost: Duration, burst: u32) -> Self {
        Self { cost, allowance: cost * burst.saturating_sub(1).max(1) }
    }

    /// What taking one request from `bucket` at `now` would leave: its new `tat`, and the instant
    /// the request may go.
    fn preview(self, bucket: Bucket, now: Instant) -> (Instant, Instant) {
        let tat = bucket.tat.max(now) + self.cost;
        (tat, tat.checked_sub(self.allowance).map_or(now, |at| at.max(now)))
    }
}

impl Pacer {
    /// A pacer for `limits`, with full buckets at `now`.
    pub fn new(limits: &LimitsSection, now: Instant) -> Self {
        Self {
            ip: Shape::of(limits.per_ip),
            account: Shape::of(limits.per_account),
            setup: Shape::per_minute(limits.fresh_accounts),
            state: Mutex::new(State {
                ip: Bucket { tat: now },
                accounts: HashMap::new(),
                setups: Bucket { tat: now },
                blocked_until: now,
            }),
        }
    }

    /// Lets a request go now, for `account` when it acts for one, taking it from the buckets.
    /// Error cases:
    /// - the buckets are empty or a retry-later hold is in force: the time to wait, after which to
    ///   ask again; nothing is taken
    pub fn try_reserve(&self, account: Option<AccountId>, now: Instant) -> Result<(), Duration> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if now < state.blocked_until {
            return Err(state.blocked_until - now);
        }
        let (ip_tat, ip_at) = self.ip.preview(state.ip, now);
        let mut at = ip_at;
        let mut account_tat = None;
        if let Some(account) = account {
            if state.accounts.len() >= FORGET_ABOVE {
                state.accounts.retain(|_, bucket| bucket.tat > now);
            }
            let bucket = state.accounts.get(&account).copied().unwrap_or(Bucket { tat: now });
            let (tat, account_at) = self.account.preview(bucket, now);
            (at, account_tat) = (at.max(account_at), Some((account, tat)));
        }
        if at > now {
            return Err(at - now);
        }
        state.ip.tat = ip_tat;
        if let Some((account, tat)) = account_tat {
            state.accounts.insert(account, Bucket { tat });
        }
        Ok(())
    }

    /// Lets a declaration go now under the fresh-account budget, taking it from that bucket; its
    /// request then waits its turn as any other.
    /// Error cases:
    /// - the bucket is empty: the time to wait, after which to ask again; nothing is taken
    pub fn try_reserve_setup(&self, now: Instant) -> Result<(), Duration> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let (tat, at) = self.setup.preview(state.setups, now);
        if at > now {
            return Err(at - now);
        }
        state.setups.tat = tat;
        Ok(())
    }

    /// Holds every request back for `wait`, at most `MAX_BACK_OFF`.
    pub fn back_off(&self, wait: Duration, now: Instant) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.blocked_until = state.blocked_until.max(now + wait.min(MAX_BACK_OFF));
    }
}

/// The node's time as the bot last verified it, counted on forward on the monotonic clock. A
/// verified answer carries the node's time, signed under the challenge sent with the request, so it
/// was true no earlier than the request was sent: the estimate never runs behind the node's time by
/// more than the answer's latency, and an answer to a request sent after some event shows the node's
/// time at least as late as the event.
#[derive(Default)]
pub struct NodeClock {
    reference: Mutex<Option<(i128, Instant)>>,
}

impl NodeClock {
    /// Takes the node's time `time_ms` from an answer to a request sent at `sent`. The reference
    /// moves to it unless it answers an earlier request than the reference's: the node's time only
    /// runs forward, so a later request's answer is never further behind the node than an earlier one
    /// counted on, and a lead the estimate had over the node does not carry over.
    pub fn observe(&self, time_ms: u64, sent: Instant) {
        let mut reference = self.reference.lock().unwrap_or_else(PoisonError::into_inner);
        if reference.is_none_or(|held| held.1 <= sent) {
            *reference = Some((i128::from(time_ms), sent));
        }
    }

    /// The node's time now in milliseconds, from an answer to a request sent at or after
    /// `not_before`, or `None` when the latest answer is to an earlier request. A caller that needs
    /// the time to be at least as late as some event passes the instant of the event.
    pub fn now_ms(&self, now: Instant, not_before: Instant) -> Option<u64> {
        let held = (*self.reference.lock().unwrap_or_else(PoisonError::into_inner))?;
        (held.1 >= not_before).then(|| u64::try_from(estimate(held, now)).unwrap_or(u64::MAX))
    }

    /// The node's time now from the latest answer, whenever its request was sent.
    pub fn latest_ms(&self, now: Instant) -> Option<u64> {
        let held = (*self.reference.lock().unwrap_or_else(PoisonError::into_inner))?;
        Some(u64::try_from(estimate(held, now)).unwrap_or(u64::MAX))
    }
}

/// The node's time, in milliseconds, at `at`, counting on from `held`.
fn estimate(held: (i128, Instant), at: Instant) -> i128 {
    held.0 + i128::try_from(at.saturating_duration_since(held.1).as_millis()).unwrap_or(i128::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(per_second: u32, burst: u32) -> LimitsSection {
        let rate = RateSection { per_second, burst };
        LimitsSection { per_ip: rate, per_account: rate, ..LimitsSection::default() }
    }

    /// A burst goes at once up to its allowance less the reserve, then at the refill rate.
    #[test]
    fn requests_go_at_the_rate_after_the_burst() {
        let now = Instant::now();
        let pacer = Pacer::new(&limits(10, 5), now);
        // Burst 5 keeps one in reserve: four go at once, and the fifth waits for the refill of one.
        assert!((0..4).all(|_| pacer.try_reserve(None, now).is_ok()));
        assert_eq!(pacer.try_reserve(None, now), Err(Duration::from_millis(100)));
        assert_eq!(pacer.try_reserve(None, now), Err(Duration::from_millis(100)), "nothing was taken");
        let later = now + Duration::from_millis(100);
        assert!(pacer.try_reserve(None, later).is_ok());
        assert_eq!(pacer.try_reserve(None, later), Err(Duration::from_millis(100)));
    }

    /// One account's bucket holds its requests back without delaying another account's.
    #[test]
    fn accounts_have_their_own_bucket() {
        let now = Instant::now();
        let pacer =
            Pacer::new(&LimitsSection { per_ip: RateSection { per_second: 1000, burst: 1000 }, ..limits(2, 2) }, now);
        let (a, b) = (AccountId([1; 20]), AccountId([2; 20]));
        assert!(pacer.try_reserve(Some(a), now).is_ok());
        assert!(pacer.try_reserve(Some(a), now).is_err(), "the account's bucket holds one");
        assert!(pacer.try_reserve(Some(b), now).is_ok());
    }

    /// Declarations go at the per-minute rate after their burst, apart from the other buckets.
    #[test]
    fn declarations_keep_to_the_fresh_account_budget() {
        let now = Instant::now();
        let fresh_accounts = MinuteRateSection { per_minute: 12, burst: 3 };
        let pacer = Pacer::new(&LimitsSection { fresh_accounts, ..limits(1000, 1000) }, now);
        // Burst 3 keeps one in reserve: two go at once, and the third waits for the refill of one.
        assert!(pacer.try_reserve_setup(now).is_ok() && pacer.try_reserve_setup(now).is_ok());
        assert_eq!(pacer.try_reserve_setup(now), Err(Duration::from_secs(5)));
        assert!(pacer.try_reserve(Some(AccountId([1; 20])), now).is_ok(), "the other buckets are full");
        assert!(pacer.try_reserve_setup(now + Duration::from_secs(5)).is_ok());
    }

    /// A retry-later hold stops every request, requests that ask again during it are told how long is
    /// left, the buckets refill meanwhile, and the hold never lasts past the cap.
    #[test]
    fn a_retry_later_holds_every_request() {
        let now = Instant::now();
        let pacer = Pacer::new(&limits(1000, 1000), now);
        pacer.back_off(Duration::from_secs(3), now);
        let account = Some(AccountId([1; 20]));
        assert_eq!(pacer.try_reserve(None, now), Err(Duration::from_secs(3)));
        assert_eq!(pacer.try_reserve(account, now + Duration::from_secs(1)), Err(Duration::from_secs(2)));
        assert!(pacer.try_reserve(account, now + Duration::from_secs(3)).is_ok());
        pacer.back_off(Duration::from_secs(3600), now + Duration::from_secs(3));
        assert_eq!(pacer.try_reserve(None, now + Duration::from_secs(3)), Err(MAX_BACK_OFF));
    }

    /// The clock counts on from the latest answer, answers only from a request sent late enough, and
    /// a later request's answer replaces an earlier estimate even when it is lower.
    #[test]
    fn the_clock_counts_on_from_verified_answers() {
        let t0 = Instant::now();
        let clock = NodeClock::default();
        assert_eq!(clock.now_ms(t0, t0), None);
        clock.observe(10_000, t0);
        let later = t0 + Duration::from_secs(2);
        assert_eq!(clock.now_ms(later, t0), Some(12_000));
        // The answer to a request sent before the event the caller waits for is no use to it.
        assert_eq!(clock.now_ms(later, t0 + Duration::from_millis(1)), None);
        // A later request answers with an earlier time than the estimate led to: the estimate comes down.
        clock.observe(11_000, later);
        assert_eq!(clock.now_ms(later, later), Some(11_000));
        // An answer to an earlier request than the reference's changes nothing.
        clock.observe(50_000, t0);
        assert_eq!(clock.now_ms(later, later), Some(11_000));
    }
}
