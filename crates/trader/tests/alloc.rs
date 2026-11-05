//! The hot path: once warm, reading a log message allocates a fixed few times, however many accounts
//! read it. A counting allocator (the `allocation-counter` crate's, per thread) checks a fleet
//! reading ten minutes of a synthetic log, decisions to trade included. What allocates is
//! `serde_json` reading a message's envelope and its command's type: it allocates once for any
//! input that nests objects or arrays (a command inside a message, quotes inside a command), even
//! when every field is borrowed or skipped. Everything the bot does after those two reads allocates
//! nothing: quotes go into the index one by one, events are borrowed slices, and decisions go onto
//! a queue sized at startup.

#[path = "support/log.rs"]
mod log;

use trader::bot::fleet::Fleet;

use log::*;

/// The allocations a message may cost: the envelope and the command's type.
const PER_MESSAGE: u64 = 2;

/// After a warm-up of two minutes of log, applying a message allocates at most `PER_MESSAGE` times,
/// though bars complete, decisions queue and are taken off the queue, and strangers' orders fill.
#[test]
fn reading_the_log_allocates_a_fixed_few_times_once_warm() {
    let mut fleet = Fleet::new(&markets(37), mixed(300), T0).unwrap();
    let seq = warm(&mut fleet, &(1..=37).collect::<Vec<_>>(), "1000.00");
    let log = hour(37, 12 * 60, seq);
    let split = log.iter().position(|e| e.ts >= T0 + 10_000 + 2 * MINUTE).unwrap();
    let (warm_up, measured) = log.split_at(split);
    let drain = |fleet: &mut Fleet| {
        while let Some(a) = fleet.queue.pop_front() {
            let asg = &mut fleet.assignments[a as usize];
            (asg.pending, asg.queued) = (None, false);
        }
    };
    for e in warm_up {
        fleet.apply(&e.msg, at(e.seq), u64::try_from(e.ts + 20).unwrap()).unwrap();
        drain(&mut fleet);
    }
    let decisions = fleet.decisions;
    let info = allocation_counter::measure(|| {
        for e in measured {
            fleet.apply(&e.msg, at(e.seq), u64::try_from(e.ts + 20).unwrap()).unwrap();
            drain(&mut fleet);
        }
    });
    let taken = fleet.decisions - decisions;
    assert!(taken > 0, "the measured minutes include decisions to trade");
    let budget = PER_MESSAGE * measured.len() as u64;
    println!("{} allocations over {} messages and {taken} decisions", info.count_total, measured.len());
    assert!(
        info.count_total <= budget,
        "{} allocations over {} messages is over {PER_MESSAGE} each",
        info.count_total,
        measured.len()
    );
}
