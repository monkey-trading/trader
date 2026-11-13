//! A target that waits for a read of its account: it is decided when the read arrives, however much
//! log time has passed since its bar.

#[path = "support/log.rs"]
mod log;

use trader::bot::fleet::{Fleet, REPLAY_LAG_MS};

use log::*;

/// A target decided from a read that comes more than the replay lag of log time after its bar is still
/// decided and queued: how late an order may go is held at the send, on the node's time, and not at the
/// read, so what is decided does not depend on how fast a replayed log is fed.
#[test]
fn a_target_that_waits_is_decided_whatever_log_time_passes() {
    let mut fleet = Fleet::new(&markets(1), vec![account(0, vec![strategy(1, MINUTE)])], T0).unwrap();
    let seq = warm(&mut fleet, &[1], "100");
    // Two bars close and give a long target, which waits for a read.
    for (i, (minute, mid)) in [(1, "101"), (2, "102")].into_iter().enumerate() {
        let (ts, seq) = (T0 + minute * MINUTE, seq + i as u64);
        fleet.apply(&prices(seq, ts, 1, mid), at(seq), u64::try_from(ts + 50).unwrap()).unwrap();
    }
    let wanted = fleet.assignments[0].wanted.expect("the target waits");
    assert_eq!(fleet.reads.iter().copied().collect::<Vec<_>>(), [0]);
    // The log moves on past the replay lag before the read that ends the wait.
    fleet.log_ts = wanted.ts + 2 * REPLAY_LAG_MS;
    settle(&mut fleet);
    assert!(fleet.assignments[0].pending.is_some(), "the target is decided");
    assert_eq!(fleet.queue.iter().copied().collect::<Vec<_>>(), [0], "and its order queued");
}
