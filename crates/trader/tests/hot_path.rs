//! The cost of many accounts: an hour of a synthetic public log (every launch market priced twice a
//! second, strangers' orders and ticks) read through 1,000 accounts across the markets the bot trades,
//! timing every message and measuring what each account holds in memory. CI holds both to budgets set
//! from the first measurement, with room for a slower runner; `--nocapture` prints them.

#[path = "support/log.rs"]
mod log;

use std::time::{Duration, Instant};

use trader::bot::fleet::Fleet;

use log::*;

/// The accounts, and the markets the log prices: the bot trades the first three.
const ACCOUNTS: u64 = 1_000;
const LAUNCH_MARKETS: u32 = 37;
/// A message's budget on average, and at the 99.9th percentile (a bar completing for every
/// account that reads it), and an account's memory.
const MEAN_BUDGET: Duration = Duration::from_micros(BUDGET_MEAN_US);
const P999_BUDGET: Duration = Duration::from_micros(BUDGET_P999_US);
const BYTES_PER_ACCOUNT_BUDGET: i64 = BUDGET_BYTES;
// The first measurement, on a development machine: 0.8 µs a message on average, 4.5 µs at the
// 99.9th percentile, and 920 bytes an account. The budgets leave a slower runner room, and still
// fail a message that costs a thousand accounts' worth of work.
const BUDGET_MEAN_US: u64 = 5;
const BUDGET_P999_US: u64 = 100;
const BUDGET_BYTES: i64 = 1_500;

#[test]
fn an_hour_through_a_thousand_accounts_is_within_budget() {
    let mut slot = None;
    let built = allocation_counter::measure(|| {
        let mut fleet = Fleet::new(&markets(3), mixed(ACCOUNTS), T0).unwrap();
        let seq = warm(&mut fleet, &[1, 2, 3], "1000.00");
        slot = Some((fleet, seq));
    });
    let (mut fleet, seq) = slot.unwrap();
    let per_account = built.bytes_current / i64::try_from(ACCOUNTS).unwrap();
    let log = hour(LAUNCH_MARKETS, 3_600, seq);
    let mut times = Vec::with_capacity(log.len());
    let mut orders = 0u64;
    let started = Instant::now();
    for e in &log {
        let begun = Instant::now();
        fleet.apply(&e.msg, at(e.seq), u64::try_from(e.ts + 20).unwrap()).unwrap();
        times.push(begun.elapsed());
        // The dispatcher takes each decision off the queue.
        while let Some(a) = fleet.queue.pop_front() {
            let asg = &mut fleet.assignments[a as usize];
            (asg.pending, asg.queued) = (None, false);
            orders += 1;
        }
    }
    let total = started.elapsed();
    times.sort_unstable();
    let mean = total / u32::try_from(log.len()).unwrap();
    let p999 = times[times.len() * 999 / 1000];
    println!(
        "{} messages through {ACCOUNTS} accounts, {orders} decisions: {mean:?} a message on average, {p999:?} at the 99.9th percentile, \
         {:?} at most; {per_account} bytes an account ({} series)",
        log.len(),
        times.last().unwrap(),
        fleet.series.len()
    );
    assert!(orders > 0);
    assert!(mean <= MEAN_BUDGET, "{mean:?} a message on average is over {MEAN_BUDGET:?}");
    assert!(p999 <= P999_BUDGET, "{p999:?} at the 99.9th percentile is over {P999_BUDGET:?}");
    assert!(
        per_account <= BYTES_PER_ACCOUNT_BUDGET,
        "{per_account} bytes an account is over {BYTES_PER_ACCOUNT_BUDGET}"
    );
}
