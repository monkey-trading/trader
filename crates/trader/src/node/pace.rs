//! Keeping the bot's requests under the node's limits, and its idea of the node's time.
//!
//! The pacer hands out the instant each request may go, so a burst of orders queues in the order it
//! was asked for instead of failing with 429: one bucket for every request (the node's `per_ip`) and
//! one per account for the requests that act for it (`per_account`). Each is a token bucket in
//! virtual-scheduling form, holding one request of its burst in reserve to absorb timing differences.
//! A retry-later answer from the node holds every request back for the delay it names.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use engine_core::types::AccountId;
use tokio::time::Instant;

use crate::config::{LimitsSection, RateSection};

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
    blocked_until: Instant,
}

/// The request pacer of one node.
pub struct Pacer {
    ip: Shape,
    account: Shape,
    state: Mutex<State>,
}

impl Shape {
    /// The bucket `rate` names, with one request of its burst kept in reserve.
    fn of(rate: RateSection) -> Self {
        let cost = Duration::from_secs(1) / rate.per_second.max(1);
        Self { cost, allowance: cost * rate.burst.saturating_sub(1).max(1) }
    }

    /// Takes one request from `bucket` at `now`; the instant it may go.
    fn take(self, bucket: &mut Bucket, now: Instant) -> Instant {
        let tat = bucket.tat.max(now) + self.cost;
        bucket.tat = tat;
        tat.checked_sub(self.allowance).map_or(now, |at| at.max(now))
    }
}

impl Pacer {
    /// A pacer for `limits`, with full buckets at `now`.
    pub fn new(limits: &LimitsSection, now: Instant) -> Self {
        Self {
            ip: Shape::of(limits.per_ip),
            account: Shape::of(limits.per_account),
            state: Mutex::new(State { ip: Bucket { tat: now }, accounts: HashMap::new(), blocked_until: now }),
        }
    }

    /// Reserves the next request, for `account` when it acts for one, and returns the instant it may
    /// be sent: no earlier than the end of any retry-later hold.
    pub fn reserve(&self, account: Option<AccountId>, now: Instant) -> Instant {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let mut at = self.ip.take(&mut state.ip, now);
        if let Some(account) = account {
            if state.accounts.len() >= FORGET_ABOVE {
                state.accounts.retain(|_, bucket| bucket.tat > now);
            }
            let bucket = state.accounts.entry(account).or_insert(Bucket { tat: now });
            at = at.max(self.account.take(bucket, now));
        }
        at.max(state.blocked_until)
    }

    /// Holds every later reservation back for `wait`, at most `MAX_BACK_OFF`.
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
    /// moves to `sent`, at the later of that time and what the old reference counts on to; an answer
    /// to an earlier request than the reference's changes nothing.
    pub fn observe(&self, time_ms: u64, sent: Instant) {
        let mut reference = self.reference.lock().unwrap_or_else(PoisonError::into_inner);
        let time = i128::from(time_ms);
        *reference = match *reference {
            Some(held) if held.1 > sent => Some(held),
            Some(held) => Some((time.max(estimate(held, sent)), sent)),
            None => Some((time, sent)),
        };
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
        LimitsSection { per_ip: rate, per_account: rate, connections: 4 }
    }

    /// A burst goes at once up to its allowance less the reserve, then at the refill rate, in the
    /// order it was asked for.
    #[test]
    fn requests_queue_at_the_rate_after_the_burst() {
        let now = Instant::now();
        let pacer = Pacer::new(&limits(10, 5), now);
        let at: Vec<_> = (0..8).map(|_| pacer.reserve(None, now)).collect();
        let ms = |i: usize| at[i].duration_since(now).as_millis();
        // Burst 5 keeps one in reserve: the fifth request waits for the refill of one.
        assert_eq!((0..8).map(ms).collect::<Vec<_>>(), [0, 0, 0, 0, 100, 200, 300, 400]);
        assert!(at.windows(2).all(|w| w[0] <= w[1]));
    }

    /// One account's bucket holds its requests back without delaying another account's.
    #[test]
    fn accounts_have_their_own_bucket() {
        let now = Instant::now();
        let pacer =
            Pacer::new(&LimitsSection { per_ip: RateSection { per_second: 1000, burst: 1000 }, ..limits(2, 2) }, now);
        let (a, b) = (AccountId([1; 20]), AccountId([2; 20]));
        let first: Vec<_> = (0..3).map(|_| pacer.reserve(Some(a), now)).collect();
        assert_eq!(first[0], now);
        assert!(first[2] > now + Duration::from_millis(400));
        assert_eq!(pacer.reserve(Some(b), now), now);
    }

    /// A retry-later answer holds back every later request, and never for more than the cap.
    #[test]
    fn a_retry_later_holds_every_request() {
        let now = Instant::now();
        let pacer = Pacer::new(&limits(1000, 1000), now);
        pacer.back_off(Duration::from_secs(3), now);
        assert_eq!(pacer.reserve(None, now), now + Duration::from_secs(3));
        pacer.back_off(Duration::from_secs(3600), now);
        assert_eq!(pacer.reserve(Some(AccountId([1; 20])), now), now + MAX_BACK_OFF);
    }

    /// The clock counts on from the latest answer and keeps the later estimate, and answers only
    /// from a request sent late enough.
    #[test]
    fn the_clock_counts_on_from_verified_answers() {
        let t0 = Instant::now();
        let clock = NodeClock::default();
        assert_eq!(clock.now_ms(t0, t0), None);
        clock.observe(10_000, t0);
        let later = t0 + Duration::from_secs(2);
        assert_eq!(clock.now_ms(later, t0), Some(12_000));
        // An answer to an older request, with an earlier estimate, changes nothing.
        clock.observe(9_000, t0);
        assert_eq!(clock.now_ms(later, t0), Some(12_000));
        // The answer to a request sent before the event the caller waits for is no use to it.
        assert_eq!(clock.now_ms(later, t0 + Duration::from_millis(1)), None);
        clock.observe(13_000, later);
        assert_eq!(clock.now_ms(later, later), Some(13_000));
    }
}
