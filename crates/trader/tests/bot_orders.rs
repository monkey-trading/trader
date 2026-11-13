//! The bot's orders against the stand-in when an order is refused, goes unanswered, or is a
//! reversal the engine refuses whole, and when someone else trades the account.

mod common;

use std::time::Duration;

use common::{
    MINUTE, StandIn, T0, account, account_view, btc_usd, bucket, command, config, order, own_fill, prices, run, warm,
};
use serde_json::json;
use tokio::sync::oneshot;

/// An order the node refuses for its nonce never entered the log, as when another request of the
/// account took the nonce: its target is decided again on the node's next nonce, once for its bar, and
/// a second refusal leaves it to the next bar.
#[tokio::test]
async fn an_order_refused_for_its_nonce_is_decided_again_once_per_bar() {
    let stand_in = StandIn::start().await;
    flat_account(&stand_in);
    stand_in.with(|i| i.statuses.extend([409, 409]));
    let dir = tempfile::tempdir().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let bot = tokio::spawn(run(config(&stand_in, dir.path(), "1m"), async {
        _ = stopped.await;
    }));
    rising(&stand_in, 11, &["100", "100", "101", "102"]);
    assert_eq!(order(&stand_in, 1).await["client_id"], T0 + MINUTE);
    // No command comes between: the bar's order is sent again from the refusal alone.
    assert_eq!(order(&stand_in, 2).await["client_id"], T0 + MINUTE, "decided again for its bar");
    stand_in.append((15, 0), command(15, T0 + 3 * MINUTE, prices(1, "103", T0 + 3 * MINUTE), json!([])));
    assert_eq!(order(&stand_in, 3).await["client_id"], T0 + 2 * MINUTE, "the second refusal waits for the next bar");
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap();
}

/// An order whose answer never comes may still be in the log, so the bar that closes before it
/// expires places no order, as after an answered one.
#[tokio::test]
async fn an_unanswered_order_holds_the_next_until_it_expires() {
    let stand_in = StandIn::start().await;
    flat_account(&stand_in);
    let dir = tempfile::tempdir().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let bot = tokio::spawn(run(config(&stand_in, dir.path(), "1m"), async {
        _ = stopped.await;
    }));
    warm(&stand_in).await;
    // Half a minute past minute 2: the order minute 1's close sends expires at minute 3. Its answer
    // takes longer than the bot waits.
    stand_in.with(|i| {
        i.time_ms = (T0 + 2 * MINUTE + 30_000) as u64;
        i.post_delay = Some(Duration::from_secs(3));
    });
    // Minute 2's first command is ten seconds in, so the bar is twenty seconds behind the node's time.
    for (seq, ts, mid) in [(14, T0 + MINUTE, "101"), (15, T0 + 2 * MINUTE + 10_000, "102")] {
        stand_in.append((seq, 0), command(seq, ts, prices(1, mid, ts), json!([])));
    }
    assert_eq!(order(&stand_in, 1).await["client_id"], T0 + MINUTE);
    stand_in.with(|i| i.post_delay = None);
    stand_in.append((16, 0), command(16, T0 + 3 * MINUTE, prices(1, "103", T0 + 3 * MINUTE), json!([])));
    stand_in.append((17, 0), command(17, T0 + 4 * MINUTE, prices(1, "104", T0 + 4 * MINUTE), json!([])));
    assert_eq!(order(&stand_in, 2).await["client_id"], T0 + 3 * MINUTE);
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap();
}

/// A reversal the engine refuses for its close-spread ceiling is retried as the close alone,
/// reduce-only, which the ceiling never refuses.
#[tokio::test]
async fn a_reversal_refused_whole_closes_first() {
    let stand_in = StandIn::start().await;
    flat_account(&stand_in);
    let dir = tempfile::tempdir().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let bot = tokio::spawn(run(config(&stand_in, dir.path(), "1m"), async {
        _ = stopped.await;
    }));
    rising(&stand_in, 11, &["100", "100", "101", "102"]);
    let first = order(&stand_in, 1).await;
    stand_in.with(|i| {
        let position = json!({ "1": { "qty": "1", "entry": "101" } });
        i.accounts.insert(account().to_string(), account_view("1000", position, json!({})));
    });
    stand_in.append((15, 0), own_fill(15, T0 + 2 * MINUTE + 1, &first, "1"));
    stand_in.with(|i| {
        i.replies.push_back(json!([{ "type": "order_rejected", "account": account(), "client_id": T0 + 3 * MINUTE,
            "reason": { "kind": "close_spread_ceiling", "ceiling_bps": 30 } }]));
    });
    // Minute 3 closes below minute 2: short, a sell of 2 from long 1, refused whole.
    stand_in.append((16, 0), command(16, T0 + 3 * MINUTE, prices(1, "101", T0 + 3 * MINUTE), json!([])));
    stand_in.append((17, 0), command(17, T0 + 4 * MINUTE, prices(1, "100", T0 + 4 * MINUTE), json!([])));
    let reversal = order(&stand_in, 2).await;
    assert_eq!(
        (reversal["side"].as_str(), reversal["qty"].as_str(), &reversal["reduce_only"]),
        (Some("sell"), Some("2"), &json!(false))
    );
    stand_in.append((18, 0), command(18, T0 + 5 * MINUTE, prices(1, "99", T0 + 5 * MINUTE), json!([])));
    let close = order(&stand_in, 3).await;
    assert_eq!(
        (close["side"].as_str(), close["qty"].as_str(), &close["reduce_only"]),
        (Some("sell"), Some("1"), &json!(true))
    );
    stand_in.with(|i| i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({}))));
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap();
}

/// A fill from an order this process did not sign is an earlier process's while that one's orders
/// can still enter the log; after that, someone else trades the account, and the bot stops, leaving
/// the account as it is. An order is the bot's by its signature, not its nonce: another request on
/// a nonce the bot also signed is not the bot's.
#[tokio::test]
async fn a_fill_from_another_signer_stops_the_bot() {
    let stand_in = StandIn::start().await;
    flat_account(&stand_in);
    let dir = tempfile::tempdir().unwrap();
    let bot = tokio::spawn(run(config(&stand_in, dir.path(), "1m"), std::future::pending()));
    let theirs = |nonce: u64| {
        let auth = json!({ "nonce": nonce, "expires_at": T0, "signature": format!("0x{}", "11".repeat(65)) });
        json!({ "account": account(), "client_id": 7, "market": 1, "side": "buy", "qty": "1",
            "kind": { "kind": "market", "quoted": "100" }, "auth": auth })
    };
    // Within the half minute after the start: an earlier process's order, which leaves it flat.
    stand_in.append((11, 0), command(11, T0, prices(1, "100", T0), json!([])));
    stand_in.append((12, 0), own_fill(12, T0 + 500, &theirs(40), "0"));
    for (seq, ts, mid) in [(13, T0 + 1_001, "100"), (14, T0 + MINUTE, "101"), (15, T0 + 2 * MINUTE, "102")] {
        stand_in.append((seq, 0), command(seq, ts, prices(1, mid, ts), json!([])));
    }
    // The bot's own order, on nonce 0; then another order on nonce 0 fills.
    let first = order(&stand_in, 1).await;
    assert_eq!(first["auth"]["nonce"], 0);
    stand_in.append((16, 0), own_fill(16, T0 + 2 * MINUTE + 1, &theirs(0), "1"));
    let err = tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap_err();
    assert!(err.to_string().contains("a fill at seq 16 is from an order this process did not sign"), "{err:#}");
    assert_eq!(stand_in.with(|i| i.posts.len()), 1, "a halt leaves the account as it is");
}

// ── Helpers ──

/// BTC-USD at 99 and a flat, funded account, with the log after seq 10.
fn flat_account(stand_in: &StandIn) {
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99"))]);
        i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({})));
        i.head_seq = 10;
        i.marks = vec![bucket(T0, "100")];
    });
}

/// Appends prices from `seq` at T0, T0 + 1.001 s, then each minute: with `mids` 100, 100, 101 and
/// 102, minute 1's close above minute 0's targets long as minute 2 starts.
fn rising(stand_in: &StandIn, seq: u64, mids: &[&str]) {
    let times = [T0, T0 + 1_001, T0 + MINUTE, T0 + 2 * MINUTE];
    for (n, (ts, mid)) in times.iter().zip(mids).enumerate() {
        let seq = seq + n as u64;
        stand_in.append((seq, 0), command(seq, *ts, prices(1, mid, *ts), json!([])));
    }
}
