//! The bot against the stand-in: it reads the market and the account, follows the log's prices into
//! bars, orders toward the crossover's target with requests the engine's own hashes verify, takes
//! its position from its fills in the log, retries a declined order on the next bar, holds orders
//! while the stream replays history, and on stopping cancels resting orders and closes its position.

mod common;

use std::time::Duration;

use common::{
    MINUTE, StandIn, T0, account, account_view, btc_usd, command, config, domain, posted, prices, resting_order, run,
};
use engine_core::command::{CancelAllRequest, CloseAllRequest, OrderRequest};
use engine_core::eip712::{cancel_all_hash, close_all_hash, order_hash, recover};
use serde_json::{Value, json};
use tokio::sync::oneshot;

#[tokio::test]
async fn trades_toward_the_crossover_and_stops_clean() {
    let stand_in = StandIn::start().await;
    let account = account();
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99"))]);
        i.accounts.insert(account.to_string(), account_view("1000", json!({}), json!({})));
        i.head_seq = 10;
        i.nonce = 3;
        // The first order is declined for slippage; the rest fill.
        i.replies.push_back(json!([{ "type": "order_rejected", "account": account, "client_id": T0 + MINUTE,
            "reason": { "kind": "slippage", "quoted": "102", "live": "103", "slippage_bps": 98 } }]));
    });
    let dir = tempfile::tempdir().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let bot = tokio::spawn(run(config(&stand_in, dir.path(), "1m"), async {
        _ = stopped.await;
    }));

    // Minute 0 closes at 100 and minute 1 at 101: the one-bar average is above the two-bar one, so
    // the target is long, and the bot buys 1 quoted at the mark of the command that closed the bar.
    stand_in.append((11, 0), command(11, T0, prices(1, "100", T0), json!([])));
    stand_in.append((12, 0), command(12, T0 + 1_001, prices(1, "100", T0 + 1_001), json!([])));
    stand_in.append((13, 0), command(13, T0 + MINUTE, prices(1, "101", T0 + MINUTE), json!([])));
    stand_in.append((14, 0), command(14, T0 + 2 * MINUTE, prices(1, "102", T0 + 2 * MINUTE), json!([])));
    check_order(&order(&stand_in, 1).await, T0 + MINUTE, "buy", "1", "102", false, 3);

    // The declined order is retried at the next bar, still long.
    stand_in.append((15, 0), command(15, T0 + 3 * MINUTE, prices(1, "103", T0 + 3 * MINUTE), json!([])));
    check_order(&order(&stand_in, 2).await, T0 + 2 * MINUTE, "buy", "1", "103", false, 4);

    // Its fill in the log makes the position long 1, and the bot reads the balance again; at
    // minute 4, 99 falls below the average of 103 and 99, so the target is short and it sells 2.
    let reads = stand_in.with(|i| i.account_reads);
    stand_in.append((16, 0), command(16, T0 + 3 * MINUTE + 1, json!({ "type": "tick" }), json!([fill(1, "1")])));
    stand_in.append((17, 0), command(17, T0 + 4 * MINUTE, prices(1, "99", T0 + 4 * MINUTE), json!([])));
    stand_in.append((18, 0), command(18, T0 + 5 * MINUTE, prices(1, "98", T0 + 5 * MINUTE), json!([])));
    check_order(&order(&stand_in, 3).await, T0 + 4 * MINUTE, "sell", "2", "98", false, 5);
    assert_eq!(stand_in.with(|i| i.account_reads), reads + 1, "one balance read for the one fill");

    // Stopping, once the node's time is past the last order's expiry, reads the account: an order
    // rests and a position is held, so it cancels, and closes quoted at the node's mark as it stands.
    stand_in.with(|i| {
        i.accounts.insert(
            account.to_string(),
            account_view("1000", json!({ "1": { "qty": "-1", "entry": "98" } }), json!({ "9": resting_order(9) })),
        );
        i.markets = json!([btc_usd(Some("97.5"))]);
        i.time_ms = (T0 + 10 * MINUTE) as u64;
    });
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap();
    let posts = stand_in.with(|i| i.posts.clone());
    assert_eq!(posts.len(), 5, "{posts:?}");
    let (path, body) = &posts[3];
    assert_eq!(path, "/v1/orders/cancel-all");
    let cancel: CancelAllRequest = serde_json::from_value(body.clone()).unwrap();
    let auth = cancel.auth.clone().unwrap();
    assert_eq!(auth.nonce, 6);
    assert_eq!(
        recover(&cancel_all_hash(&domain(), account, auth.nonce, auth.expires_at), &auth.signature),
        Some(account)
    );
    let (path, body) = &posts[4];
    assert_eq!(path, "/v1/positions/close-all");
    assert_eq!(body["closes"], json!([{ "market": 1, "quoted": "97.5" }]));
    assert_eq!(body["max_slippage_bps"], 25);
    let close: CloseAllRequest = serde_json::from_value(body.clone()).unwrap();
    let auth = close.auth.clone().unwrap();
    assert_eq!(
        recover(&close_all_hash(&domain(), &close, auth.nonce, auth.expires_at), &auth.signature),
        Some(account)
    );
}

/// With nothing resting and no position, stopping sends nothing.
#[tokio::test]
async fn stopping_flat_sends_nothing() {
    let stand_in = StandIn::start().await;
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("100"))]);
        i.accounts.insert(account().to_string(), account_view("5", json!({}), json!({})));
    });
    let dir = tempfile::tempdir().unwrap();
    run(config(&stand_in, dir.path(), "1m"), async {}).await.unwrap();
    assert!(stand_in.with(|i| i.posts.is_empty()));
}

/// Startup refuses an account the node never credited, a market it does not list, and a market the
/// bot cannot follow; and stops at once on a node that signs with another key, sending nothing.
#[tokio::test]
async fn startup_refusals() {
    let stand_in = StandIn::start().await;
    let dir = tempfile::tempdir().unwrap();
    stand_in.with(|i| i.markets = json!([btc_usd(Some("100"))]));
    let err = run(config(&stand_in, dir.path(), "1m"), async {}).await.unwrap_err();
    assert!(err.to_string().contains("never credited"), "{err:#}");

    stand_in.with(|i| {
        i.accounts.insert(account().to_string(), account_view("5", json!({}), json!({})));
        let mut weekly = btc_usd(Some("100"));
        weekly["schedule"] = json!({ "kind": "weekly", "tz": "America/New_York", "windows": [] });
        i.markets = json!([weekly]);
    });
    let err = run(config(&stand_in, dir.path(), "1m"), async {}).await.unwrap_err();
    assert!(err.to_string().contains("never closes"), "{err:#}");

    stand_in.with(|i| i.markets = json!([]));
    let err = run(config(&stand_in, dir.path(), "1m"), async {}).await.unwrap_err();
    assert!(err.to_string().contains("no market BTC-USD"), "{err:#}");

    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("100"))]);
        i.answers = Some(common::Answers::OtherSigner);
    });
    let err = run(config(&stand_in, dir.path(), "1m"), async {}).await.unwrap_err();
    assert!(format!("{err:#}").contains("not signed by the configured signer"), "{err:#}");
    assert!(stand_in.with(|i| i.posts.is_empty()));
}

/// The bars before the current one come from the marks history: with the slow average full from
/// the history, the first bar the log completes already orders.
#[tokio::test]
async fn warms_up_from_the_marks_history() {
    let stand_in = StandIn::start().await;
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("100"))]);
        i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({})));
        i.head_seq = 10;
        // Three one-minute buckets: the two-minute bars before T0 close at 105 and at 100.
        let bucket = |ts: i64, close: &str| {
            json!({ "ts": ts, "open": close, "high": close, "low": close,
            "close": close, "provenance": "log_derived" })
        };
        i.marks = vec![bucket(T0 - 4 * MINUTE, "105"), bucket(T0 - 2 * MINUTE, "101"), bucket(T0 - MINUTE, "100")];
    });
    let dir = tempfile::tempdir().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let bot = tokio::spawn(run(config(&stand_in, dir.path(), "2m"), async {
        _ = stopped.await;
    }));
    // Bars of two minutes: history gives 105 then 100; the log's first bar closes at 99, below the
    // average of 100 and 99, so the target is short at once.
    stand_in.append((11, 0), command(11, T0, prices(1, "99", T0), json!([])));
    stand_in.append((12, 0), command(12, T0 + 1_001, prices(1, "99", T0 + 1_001), json!([])));
    stand_in.append((13, 0), command(13, T0 + 2 * MINUTE, prices(1, "98", T0 + 2 * MINUTE), json!([])));
    check_order(&order(&stand_in, 1).await, T0, "sell", "1", "98", false, 0);
    stand_in.with(|i| i.time_ms = (T0 + 10 * MINUTE) as u64);
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap();
}

/// Bars the stream replays from history, more than half a minute behind the node's time, update
/// the strategy but place no order; the first bar closed live does.
#[tokio::test]
async fn replayed_bars_place_no_order() {
    let stand_in = StandIn::start().await;
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99"))]);
        i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({})));
        i.head_seq = 10;
    });
    let dir = tempfile::tempdir().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let bot = tokio::spawn(run(config(&stand_in, dir.path(), "1m"), async {
        _ = stopped.await;
    }));
    // Once the bot has read the account and opened its stream, the node's time is ten minutes on:
    // minute 1's close at 101 above minute 0's targets long, but the command that closes it is
    // eight minutes old, so nothing is ordered.
    while stand_in.with(|i| i.connections.is_empty()) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    stand_in.with(|i| i.time_ms = (T0 + 10 * MINUTE) as u64);
    stand_in.append((11, 0), command(11, T0, prices(1, "100", T0), json!([])));
    stand_in.append((12, 0), command(12, T0 + 1_001, prices(1, "100", T0 + 1_001), json!([])));
    stand_in.append((13, 0), command(13, T0 + MINUTE, prices(1, "101", T0 + MINUTE), json!([])));
    stand_in.append((14, 0), command(14, T0 + 2 * MINUTE, prices(1, "102", T0 + 2 * MINUTE), json!([])));
    // Minute 2's bar closes live, and is still long: the bot orders now, quoted at the live mark.
    stand_in.append((15, 0), command(15, T0 + 10 * MINUTE, prices(1, "103", T0 + 10 * MINUTE), json!([])));
    let first = order(&stand_in, 1).await;
    assert_eq!(first["client_id"], T0 + 2 * MINUTE, "{first}");
    assert_eq!(first["kind"]["quoted"], "103");
    stand_in.with(|i| i.time_ms = (T0 + 20 * MINUTE) as u64);
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap();
    assert_eq!(stand_in.with(|i| i.posts.len()), 1);
}

/// A window in which the mark does not move makes no bar, as the node's marks history keeps none
/// for it: after a long at 101, a quiet minute is not a second 101 that would make the target flat
/// and close the position; the move down to 100 makes it short.
#[tokio::test]
async fn only_moves_of_the_mark_make_bars() {
    let stand_in = StandIn::start().await;
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99"))]);
        i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({})));
        i.head_seq = 10;
    });
    let dir = tempfile::tempdir().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let bot = tokio::spawn(run(config(&stand_in, dir.path(), "1m"), async {
        _ = stopped.await;
    }));
    stand_in.append((11, 0), command(11, T0, prices(1, "100", T0), json!([])));
    stand_in.append((12, 0), command(12, T0 + 1_001, prices(1, "100", T0 + 1_001), json!([])));
    stand_in.append((13, 0), command(13, T0 + MINUTE, prices(1, "101", T0 + MINUTE), json!([])));
    stand_in.append((14, 0), command(14, T0 + 2 * MINUTE, prices(1, "101", T0 + 2 * MINUTE), json!([])));
    check_order(&order(&stand_in, 1).await, T0 + MINUTE, "buy", "1", "101", false, 0);
    stand_in.append((15, 0), command(15, T0 + 2 * MINUTE + 1, json!({ "type": "tick" }), json!([fill(1, "1")])));
    stand_in.append((16, 0), command(16, T0 + 3 * MINUTE, prices(1, "101", T0 + 3 * MINUTE), json!([])));
    stand_in.append((17, 0), command(17, T0 + 4 * MINUTE, prices(1, "100", T0 + 4 * MINUTE), json!([])));
    stand_in.append((18, 0), command(18, T0 + 5 * MINUTE, prices(1, "100", T0 + 5 * MINUTE), json!([])));
    check_order(&order(&stand_in, 2).await, T0 + 4 * MINUTE, "sell", "2", "100", false, 1);
    stand_in.with(|i| i.time_ms = (T0 + 10 * MINUTE) as u64);
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap();
}

/// While the log has not passed the last order's expiry, that order's fill may still be to come,
/// so a bar closing before then places no order and the next bar decides again. A bar half a
/// minute behind the node's time is still live.
#[tokio::test]
async fn an_order_holds_the_next_until_it_expires() {
    let stand_in = StandIn::start().await;
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99"))]);
        i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({})));
        i.head_seq = 10;
    });
    let dir = tempfile::tempdir().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let bot = tokio::spawn(run(config(&stand_in, dir.path(), "1m"), async {
        _ = stopped.await;
    }));
    while stand_in.with(|i| i.connections.is_empty()) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // The node's time is half a minute past minute 2, so the order minute 1's close sends expires
    // at minute 3, and the bar minute 3's command closes is held.
    stand_in.with(|i| i.time_ms = (T0 + 2 * MINUTE + 30_000) as u64);
    for (seq, ts, mid) in
        [(11, T0, "100"), (12, T0 + 1_001, "100"), (13, T0 + MINUTE, "101"), (14, T0 + 2 * MINUTE, "102")]
    {
        stand_in.append((seq, 0), command(seq, ts, prices(1, mid, ts), json!([])));
    }
    assert_eq!(order(&stand_in, 1).await["client_id"], T0 + MINUTE);
    stand_in.append((15, 0), command(15, T0 + 3 * MINUTE, prices(1, "103", T0 + 3 * MINUTE), json!([])));
    stand_in.append((16, 0), command(16, T0 + 4 * MINUTE, prices(1, "104", T0 + 4 * MINUTE), json!([])));
    assert_eq!(order(&stand_in, 2).await["client_id"], T0 + 3 * MINUTE);
    stand_in.with(|i| i.time_ms = (T0 + 10 * MINUTE) as u64);
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap();
}

// ── Helpers ──

/// The `n`th POST, which must be an order.
async fn order(stand_in: &StandIn, n: usize) -> Value {
    let (path, body) = posted(stand_in, n).await;
    assert_eq!(path, "/v1/orders", "{body}");
    body
}

/// A fill of the test account in BTC-USD that leaves `position`.
fn fill(order_id: u64, position: &str) -> Value {
    json!({ "type": "fill", "account": account(), "order_id": order_id, "market": 1, "side": "buy", "qty": "1",
        "price": "100", "index": "100", "spread_bps": 0, "realised_pnl": "0", "profit_fee": "0",
        "position": position, "liquidation": false })
}

/// Checks a posted order's fields and that its signature is the account's over the engine's hash.
fn check_order(body: &Value, client_id: i64, side: &str, qty: &str, quoted: &str, reduce_only: bool, nonce: u64) {
    assert_eq!(body["client_id"], client_id, "{body}");
    assert_eq!(
        (body["market"].as_u64(), body["side"].as_str(), body["qty"].as_str()),
        (Some(1), Some(side), Some(qty))
    );
    assert_eq!(body["kind"], json!({ "kind": "market", "quoted": quoted, "max_slippage_bps": 25 }));
    assert_eq!(body["reduce_only"], reduce_only);
    let order: OrderRequest = serde_json::from_value(body.clone()).unwrap();
    let auth = order.auth.clone().unwrap();
    assert_eq!(auth.nonce, nonce);
    assert_eq!(auth.expires_at, T0 + 30_000, "the expiry counts from the node's signed time");
    let hash = order_hash(&domain(), &order, auth.nonce, auth.expires_at);
    assert_eq!(recover(&hash, &auth.signature), Some(account()));
}
