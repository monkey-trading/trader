//! The hot path: once warm, reading a log message allocates a fixed few times, however many accounts
//! read it. A counting allocator (the `allocation-counter` crate's, per thread) checks a fleet
//! reading ten minutes of a synthetic log. What allocates is `serde_json` reading a message's
//! envelope and its command's type: it allocates once for any input that nests objects or arrays (a
//! command inside a message, quotes inside a command), even when every field is borrowed or skipped.
//! Everything else the fleet does with a message allocates nothing: quotes go into the index one by
//! one, events are borrowed slices, and decisions go onto a queue sized at startup. The decision
//! itself, taken when an account's read arrives and not when a message does, is counted apart: the
//! account controls collect the holdings an order would leave, once for each decision to trade.

#[path = "support/log.rs"]
mod log;

use trader::bot::fleet::Fleet;

use log::*;

/// The allocations a message may cost: the envelope and the command's type.
const PER_MESSAGE: u64 = 2;
/// The allocations taking an account's read and deciding what it wants may cost for each decision to trade.
const PER_DECISION: u64 = 1;

/// After a warm-up of two minutes of log, applying any message allocates at most `PER_MESSAGE` times,
/// though bars complete, decisions queue and are taken off the queue, and strangers' orders fill.
#[test]
fn reading_the_log_allocates_a_fixed_few_times_once_warm() {
    let mut fleet = Fleet::new(&markets(37), mixed(300), T0).unwrap();
    let seq = warm(&mut fleet, &(1..=37).collect::<Vec<_>>(), "1000.00");
    let log = hour(37, 12 * 60, seq);
    let split = log.iter().position(|e| e.ts >= T0 + 10_000 + 2 * MINUTE).unwrap();
    let (warm_up, measured) = log.split_at(split);
    let drain = |fleet: &mut Fleet| -> u64 {
        // The runner reads the accounts a bar wants decided and takes the decisions.
        let (_, allocations) = settle_counting(fleet);
        while let Some(a) = fleet.queue.pop_front() {
            let asg = &mut fleet.assignments[a as usize];
            (asg.pending, asg.queued) = (None, false);
        }
        allocations
    };
    for e in warm_up {
        fleet.apply(&e.msg, at(e.seq), u64::try_from(e.ts + 20).unwrap()).unwrap();
        drain(&mut fleet);
    }
    let decisions = fleet.decisions;
    let (mut total, mut worst, mut worst_at, mut deciding) = (0, 0, 0, 0);
    for e in measured {
        let info = allocation_counter::measure(|| {
            fleet.apply(&e.msg, at(e.seq), u64::try_from(e.ts + 20).unwrap()).unwrap();
        });
        deciding += drain(&mut fleet);
        total += info.count_total;
        if info.count_total > worst {
            (worst, worst_at) = (info.count_total, e.seq);
        }
    }
    let taken = fleet.decisions - decisions;
    assert!(taken > 0, "the measured minutes include decisions to trade");
    println!(
        "{total} allocations over {} messages and {taken} decisions; at most {worst}, at seq {worst_at}; \
         {deciding} allocations taking the reads and deciding",
        measured.len()
    );
    assert!(worst <= PER_MESSAGE, "a message at seq {worst_at} allocated {worst} times, over {PER_MESSAGE}");
    assert!(deciding <= PER_DECISION * taken, "{taken} decisions allocated {deciding} times, over {PER_DECISION} each");
}
