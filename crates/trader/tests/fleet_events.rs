//! What the log's events and commands change in the fleet besides orders and fills: a rollover's
//! financing, the commands that list markets, and what the bot cannot follow of a listing.

#[path = "support/log.rs"]
mod log;

use engine_core::command::{AdminCommand, Command};
use engine_core::governance::{PassedProposal, VotedChange};
use engine_core::market::{MarketSpec, Tier};
use trader::bot::fleet::Fleet;

use log::*;

/// Applies `msg` as the node sent it 50 ms after its command.
fn apply(fleet: &mut Fleet, seq: u64, ts: i64, msg: &str) {
    fleet.apply(msg, at(seq), u64::try_from(ts + 50).unwrap()).unwrap();
}

/// The signature `n` bytes of `byte`.
fn signature(byte: &str) -> String {
    format!("0x{}", byte.repeat(65))
}

/// `spec(1)` with margin of 10% in one band.
fn stricter() -> MarketSpec {
    MarketSpec { margin_tiers: vec![Tier { up_to: None, bps: 1000 }], ..spec(1) }
}

/// A rollover's financing names no account, so each account holding a position may have been charged:
/// those are behind until a read, and an account holding nothing is not.
#[test]
fn a_rollover_marks_the_accounts_holding_positions_behind() {
    let accounts = (0..3).map(|i| account(i, vec![strategy(1, MINUTE)])).collect();
    let mut fleet = Fleet::new(&markets(1), accounts, T0).unwrap();
    let seq = warm(&mut fleet, &[1], "100");
    settle(&mut fleet);
    fleet.accounts[1].signed.push_back(signature("66").parse().unwrap());
    let address = fleet.accounts[1].address.to_string();
    let ts = T0 + MINUTE;
    apply(&mut fleet, seq, ts, &order_fill(seq, ts, &address, &signature("66"), 1, "1"));
    settle(&mut fleet);
    assert!(fleet.accounts.iter().all(|a| !a.behind));
    let root = format!("0x{}", "00".repeat(32));
    let event = format!(
        r#"{{"type":"financing_applied","at":{ts},"span":86400000,"positions":1,"accounts":1,"net":"0.5","root":"{root}"}}"#
    );
    apply(&mut fleet, seq + 1, ts, &command(seq + 1, ts, r#"{"type":"tick"}"#, &format!("[{event}]")));
    assert_eq!(fleet.accounts.iter().map(|a| a.behind).collect::<Vec<_>>(), [false, true, false]);
    assert_eq!(fleet.accounts.iter().map(|a| a.epoch).collect::<Vec<_>>(), [0, 2, 0], "its orders are decided again");
    assert_eq!(fleet.reads.iter().copied().collect::<Vec<_>>(), [1]);
    settle(&mut fleet);
    assert!(!fleet.accounts[1].behind, "a read of the log ends it");
}

/// Each command that lists markets takes the new rules into the fleet, however it reaches the log:
/// one listing or a batch, signed by the operator or applied after a vote.
#[test]
fn every_listing_command_changes_the_market() {
    let upsert = || AdminCommand::UpsertMarket(stricter());
    let batch = || AdminCommand::RelistMarkets { specs: vec![stricter()] };
    let voted = |command| Command::ApplyProposal(Box::new(VotedChange { proposal: 3, command }));
    let listings =
        [("a batch", Command::Admin(batch())), ("a voted listing", voted(upsert())), ("a voted batch", voted(batch()))];
    for (name, listing) in listings {
        let mut fleet = Fleet::new(&markets(1), vec![account(0, vec![strategy(1, MINUTE)])], T0).unwrap();
        let seq = warm(&mut fleet, &[1], "100");
        let cmd = serde_json::to_string(&listing).unwrap();
        let ts = T0 + 1_000;
        apply(&mut fleet, seq, ts, &command(seq, ts, &cmd, r#"[{"type":"admin_applied"}]"#));
        assert_eq!(fleet.markets[0].spec.margin_tiers[0].bps, 1000, "{name}: the market takes its new listing");
        assert_eq!(fleet.accounts[0].epoch, 1, "{name}: an order decided under the old listing is not sent");
    }
}

/// A listing that asks a weekend margin makes the market one the accounts that trade it cannot follow:
/// their margin checks read the market's bands at every time, and the engine's requirement rises over
/// the weekend gap. The account that trades another market goes on.
#[test]
fn a_listing_with_a_weekend_margin_stops_the_accounts_that_trade_the_market() {
    let accounts = vec![account(0, vec![strategy(1, MINUTE)]), account(1, vec![strategy(2, MINUTE)])];
    let mut fleet = Fleet::new(&markets(2), accounts, T0).unwrap();
    let seq = warm(&mut fleet, &[1, 2], "100");
    let listing = MarketSpec { weekend_margin_bps: 500, ..spec(1) };
    let cmd = serde_json::to_string(&Command::Admin(AdminCommand::UpsertMarket(listing))).unwrap();
    let ts = T0 + 1_000;
    apply(&mut fleet, seq, ts, &command(seq, ts, &cmd, r#"[{"type":"admin_applied"}]"#));
    let reason = fleet.accounts[0].failed.as_deref().expect("the account that trades the market stops");
    assert!(reason.contains("weekend margin"), "{reason}");
    assert!(fleet.accounts[1].failed.is_none(), "the other account goes on");
}

/// A fast-path vote puts stricter margin bands into a market's listing with no `admin_applied`, so the
/// bands the fleet checks fall below the ones in force: the accounts that trade the market stop, and a
/// vote that leaves the bands as they are stops none.
#[test]
fn a_fast_vote_that_raises_the_margin_bands_stops_the_accounts_that_trade_the_market() {
    let restriction = |spec: MarketSpec| {
        let passed = PassedProposal {
            proposal: 4,
            fast: true,
            command_hash: engine_core::eip712::B256::ZERO,
            command: Some(AdminCommand::UpsertMarket(spec)),
            block: 9,
            chain_ref: "0x01:0".into(),
        };
        serde_json::to_string(&Command::PassProposal(Box::new(passed))).unwrap()
    };
    let events = r#"[{"type":"restriction_applied","proposal":4,"expires_at":99}]"#;
    let accounts = vec![account(0, vec![strategy(1, MINUTE)]), account(1, vec![strategy(2, MINUTE)])];
    let mut fleet = Fleet::new(&markets(2), accounts, T0).unwrap();
    let seq = warm(&mut fleet, &[1, 2], "100");
    let ts = T0 + 1_000;
    apply(&mut fleet, seq, ts, &command(seq, ts, &restriction(spec(1)), events));
    assert!(fleet.accounts.iter().all(|a| a.failed.is_none()), "bands as they are stop nobody");
    apply(&mut fleet, seq + 1, ts + 1, &command(seq + 1, ts + 1, &restriction(stricter()), events));
    let reason = fleet.accounts[0].failed.as_deref().expect("the account that trades the market stops");
    assert!(reason.contains("stricter margin bands"), "{reason}");
    assert!(fleet.accounts[1].failed.is_none(), "the other account goes on");
}
