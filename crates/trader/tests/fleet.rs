//! Many accounts sharing one log: the fleet driven message by message, with no node. What is
//! shared (the index, the bars, the averages), what is each account's own (position, fence, order
//! signatures), and that one account's failure leaves the others trading.

#[path = "support/log.rs"]
mod log;

use engine_core::eip712::Signature;
use trader::bot::fleet::{AccountIn, StrategyIn};
use trader::bot::fleet::{Fleet, Warm};

use log::*;

/// Applies `msg` as the node sent it 50 ms after its command.
fn apply(fleet: &mut Fleet, seq: u64, ts: i64, msg: &str) {
    fleet.apply(msg, at(seq), u64::try_from(ts + 50).unwrap()).unwrap();
}

/// A fleet of `n` accounts on BTC-USD one-minute bars, warmed up on a mark of 100 in minute 0.
fn warmed(n: u64) -> (Fleet, u64) {
    let accounts = (0..n).map(|i| account(i, vec![strategy(1, MINUTE)])).collect();
    let mut fleet = Fleet::new(&markets(1), accounts, T0).unwrap();
    let seq = warm(&mut fleet, &[1], "100");
    (fleet, seq)
}

/// Prices 101 in minute 1 and 102 in minute 2 complete bars closing at 100 and 101: the fast
/// average (one bar) is above the slow (two), so the target is long. Returns the next seq.
fn go_long(fleet: &mut Fleet, seq: u64) -> u64 {
    apply(fleet, seq, T0 + MINUTE, &prices(seq, T0 + MINUTE, 1, "101"));
    apply(fleet, seq + 1, T0 + 2 * MINUTE, &prices(seq + 1, T0 + 2 * MINUTE, 1, "102"));
    seq + 2
}

/// The signature `n` bytes of `byte`.
fn signature(byte: &str) -> String {
    format!("0x{}", byte.repeat(65))
}

/// Accounts that trade the same market at the same bar length share one series, whatever their
/// periods, and a series holds one average for each distinct period.
#[test]
fn strategies_on_one_market_and_bar_length_share_a_series() {
    let with = |fast, slow| StrategyIn { fast, slow, ..strategy(1, MINUTE) };
    let mut accounts: Vec<AccountIn> = (0..100).map(|i| account(i, vec![strategy(1, MINUTE)])).collect();
    accounts.extend((100..200).map(|i| account(i, vec![with(2, 5), strategy(2, 5 * MINUTE)])));
    let fleet = Fleet::new(&markets(2), accounts, T0).unwrap();
    assert_eq!(fleet.markets.len(), 2);
    assert_eq!(fleet.series.len(), 2, "market 1 at 1m (read by 200 strategies), market 2 at 5m");
    // (1,1m) holds the periods 1, 2, 5 once each, and all 200 accounts' first strategies.
    let shared = &fleet.series[0];
    assert_eq!([1, 2, 3, 5].map(|p| shared.series.slot(p).is_some()), [true, true, false, true]);
    assert_eq!(shared.series.longest(), 5);
    assert_eq!(shared.assignments.len(), 200);
    assert_eq!(fleet.assignments.len(), 300);
}

/// A bar that completes queues every account whose position is not at the target, in the order
/// of the accounts, and not one already there.
#[test]
fn a_completed_bar_queues_the_accounts_that_are_not_at_their_target() {
    let (mut fleet, seq) = warmed(4);
    fleet.assignments[1].position = "1".parse().unwrap();
    go_long(&mut fleet, seq);
    assert_eq!(fleet.queue.iter().copied().collect::<Vec<_>>(), [0, 2, 3]);
    assert!(fleet.assignments[1].pending.is_none());
    assert!(fleet.assignments.iter().filter(|a| a.queued).count() == 3);
}

/// Decisions queue in the order their bars completed, across markets and bar lengths: a market's
/// one-minute bar completes two minutes before another's two-minute bar.
#[test]
fn decisions_queue_in_bar_order_across_markets() {
    let two = vec![strategy(2, 2 * MINUTE), strategy(1, MINUTE)];
    let mut fleet = Fleet::new(&markets(2), vec![account(0, two.clone()), account(1, two)], T0).unwrap();
    let mut seq = warm(&mut fleet, &[1, 2], "100");
    for minute in 1..=4 {
        for market in [2, 1] {
            let (ts, mid) = (T0 + minute * MINUTE, (100 + minute).to_string());
            apply(&mut fleet, seq, ts, &prices(seq, ts, market, &mid));
            seq += 1;
            if minute == 2 && market == 1 {
                let queued: Vec<_> = fleet.queue.iter().map(|&a| fleet.assignments[a as usize].market).collect();
                assert_eq!(queued, [1, 1], "only the one-minute bars have completed");
            }
        }
    }
    let queued: Vec<_> = fleet
        .queue
        .iter()
        .map(|&a| (fleet.assignments[a as usize].account, fleet.assignments[a as usize].market))
        .collect();
    assert_eq!(queued, [(0, 1), (1, 1), (0, 2), (1, 2)]);
}

/// A decision taken while an earlier one for the same assignment waits replaces it, keeping its place.
#[test]
fn a_newer_decision_replaces_a_queued_one_in_place() {
    let (mut fleet, seq) = warmed(2);
    let seq = go_long(&mut fleet, seq);
    let before: Vec<_> = fleet.queue.iter().copied().collect();
    // The price falls inside minute 2; minute 3 completes a bar at 50, under the average: short.
    apply(&mut fleet, seq, T0 + 2 * MINUTE + 10_000, &prices(seq, T0 + 2 * MINUTE + 10_000, 1, "50"));
    apply(&mut fleet, seq + 1, T0 + 3 * MINUTE, &tick(seq + 1, T0 + 3 * MINUTE));
    assert_eq!(fleet.queue.iter().copied().collect::<Vec<_>>(), before);
    assert!(fleet.assignments.iter().all(|a| a.pending.is_some_and(|p| p.bar_start == T0 + 2 * MINUTE)));
    assert_eq!(format!("{:?}", fleet.assignments[0].pending.unwrap().target), "Short");
}

/// A bar the stream replays from history, one inside an order's fence, and one before an earlier
/// process's orders can have expired queue nothing.
#[test]
fn bars_that_may_not_order_queue_nothing() {
    // Replayed: the frame was sent minutes after the command.
    let (mut fleet, seq) = warmed(1);
    apply(&mut fleet, seq, T0 + MINUTE, &prices(seq, T0 + MINUTE, 1, "101"));
    let ts = T0 + 2 * MINUTE;
    fleet.apply(&prices(seq + 1, ts, 1, "102"), at(seq + 1), u64::try_from(ts + 40_000).unwrap()).unwrap();
    assert!(fleet.queue.is_empty());
    // Fenced: the last order signed here may still enter the log.
    let (mut fleet, seq) = warmed(1);
    fleet.assignments[0].expires_at = Some(T0 + 2 * MINUTE);
    go_long(&mut fleet, seq);
    assert!(fleet.queue.is_empty());
    // An earlier process's order may still enter the log.
    let accounts = vec![account(0, vec![strategy(1, MINUTE)])];
    let mut fleet = Fleet::new(&markets(1), accounts, T0 + 2 * MINUTE).unwrap();
    let seq = warm(&mut fleet, &[1], "100");
    go_long(&mut fleet, seq);
    assert!(fleet.queue.is_empty());
}

/// An account that has failed is not decided; the others are.
#[test]
fn a_failed_account_is_not_queued() {
    let (mut fleet, seq) = warmed(3);
    fleet.fail(1, "test".into());
    go_long(&mut fleet, seq);
    assert_eq!(fleet.queue.iter().copied().collect::<Vec<_>>(), [0, 2]);
}

/// A fill of one account that no order of this process signed stops that account and no other.
#[test]
fn a_foreign_fill_stops_only_its_account() {
    let (mut fleet, seq) = warmed(3);
    let address = fleet.accounts[0].address.to_string();
    let ts = T0 + MINUTE;
    apply(&mut fleet, seq, ts, &order_fill(seq, ts, &address, &signature("11"), 1, "1"));
    assert!(fleet.accounts[0].failed.as_deref().is_some_and(|r| r.contains("did not sign")));
    assert!(fleet.accounts[1].failed.is_none() && fleet.accounts[2].failed.is_none());
    assert_eq!(fleet.assignments[0].position, "1".parse().unwrap(), "the position still follows the log");
    go_long(&mut fleet, seq + 1);
    assert_eq!(fleet.queue.iter().copied().collect::<Vec<_>>(), [1, 2]);
}

/// A signature is one account's: a fill of another account on a request carrying it is not that
/// account's own order, and a nonce is no identity.
#[test]
fn an_order_is_an_accounts_own_by_its_signature_alone() {
    let (mut fleet, seq) = warmed(2);
    let signed: Signature = signature("22").parse().unwrap();
    fleet.accounts[0].signed.push_back(signed);
    let (a0, a1) = (fleet.accounts[0].address.to_string(), fleet.accounts[1].address.to_string());
    let ts = T0 + MINUTE;
    apply(&mut fleet, seq, ts, &order_fill(seq, ts, &a0, &signature("22"), 1, "1"));
    assert!(fleet.accounts[0].failed.is_none(), "its own order");
    apply(&mut fleet, seq + 1, ts, &order_fill(seq + 1, ts, &a1, &signature("22"), 1, "1"));
    assert!(fleet.accounts[1].failed.is_some(), "the other account's fill on the first account's signature");
    assert!(fleet.accounts[0].failed.is_none());
}

/// Fills move their own account's position only, and mark its balance for one read however many
/// fills there are.
#[test]
fn fills_move_their_own_accounts_and_mark_one_balance_read() {
    let (mut fleet, seq) = warmed(3);
    fleet.accounts[2].signed.push_back(signature("33").parse().unwrap());
    let address = fleet.accounts[2].address.to_string();
    let ts = T0 + MINUTE;
    apply(&mut fleet, seq, ts, &order_fill(seq, ts, &address, &signature("33"), 1, "1"));
    apply(&mut fleet, seq + 1, ts, &order_fill(seq + 1, ts, &address, &signature("33"), 1, "2"));
    let positions: Vec<_> = fleet.assignments.iter().map(|a| a.position.to_string()).collect();
    assert_eq!(positions, ["0", "0", "2"]);
    assert_eq!(fleet.stale_balances, [2]);
}

/// A series is warm only once it has read its history; until then it decides nothing.
#[test]
fn a_series_decides_nothing_before_it_has_warmed_up() {
    let accounts = vec![account(0, vec![strategy(1, MINUTE)])];
    let mut fleet = Fleet::new(&markets(1), accounts, T0).unwrap();
    for (seq, ts) in [(10, T0), (11, T0 + 1_001), (12, T0 + 6_002)] {
        apply(&mut fleet, seq, ts, &prices(seq, ts, 1, "100"));
    }
    assert_eq!(fleet.series[0].warm, Warm::Requested);
    assert_eq!(fleet.warm_requests.len(), 1);
    go_long(&mut fleet, 20);
    assert!(fleet.queue.is_empty());
    let request = fleet.warm_requests.pop().unwrap();
    assert_eq!((request.resolution, request.market), ("1m", 1));
    // A failed read is tried again five seconds of log later, not at once.
    fleet.warmed(request.series, None, T0);
    assert!(matches!(fleet.series[0].warm, Warm::RetryAt(_)));
}
