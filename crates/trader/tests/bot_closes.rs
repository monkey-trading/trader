//! The close of an account the daily loss limit stopped, against the stand-in: it waits while the account
//! is not at the log, and a request the engine rejects without a fill frees the markets it named.

mod common;

use std::time::Duration;

use common::{
    MINUTE, StandIn, T0, account, account_view, btc_usd, command, eth_usd, next_seq, own_close, posted,
    posts_after_settling, price_at, prices, start, warm,
};
use serde_json::json;

const LIMIT: (&str, &str) = ("daily_loss = { usd = \"100000\" }", "daily_loss = { usd = \"5\" }");

/// A close the limit asks for while the node's clock is past the replay lag waits, and an account that
/// stops being at the log meanwhile (a cash event the view does not show) signs nothing until a read of it
/// agrees with the log: then the close goes.
#[tokio::test]
async fn a_queued_close_waits_while_its_account_is_behind_the_log() {
    let stand_in = StandIn::start().await;
    let dir = tempfile::tempdir().unwrap();
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99"))]);
        let position = json!({ "1": { "qty": "1", "entry": "99" } });
        i.accounts.insert(account().to_string(), account_view("1000", position, json!({})));
        i.head_seq = 10;
        i.nonce = 3;
        i.settle = false;
    });
    let (stop, bot) = start(&stand_in, dir.path(), &[LIMIT]);
    warm(&stand_in).await;

    // A cash event makes the bot read the account, and the answer puts the node's time a minute past the
    // log, though its frames say it is current: the loss then trips the limit and asks for a close, which
    // waits for the log to catch up.
    let event = json!({ "type": "corporate_dividend", "id": "d", "market": 1, "account": account(),
        "quantity": "1", "amount": "5", "fx_mark": null });
    stand_in.with(|i| i.time_ms = (T0 + 2 * MINUTE) as u64);
    let seq = next_seq(&stand_in);
    stand_in.append((seq, 0), command(seq, T0 + 30_000, json!({ "type": "tick" }), json!([event])));
    assert_eq!(posts_after_settling(&stand_in).await, 0);
    stand_in.with(|i| i.frame_time = Some((T0 + MINUTE + 100) as u64));
    price_at(&stand_in, T0 + MINUTE, "94");
    assert_eq!(posts_after_settling(&stand_in).await, 0, "the close waits for the log");

    // A cash event the account's view does not hold makes the account's balance not the log's. The node's
    // time is current again, so only the account's state holds the close back.
    let held = json!({ "1": { "qty": "2", "entry": "99" } });
    stand_in.with(|i| {
        i.time_ms = (T0 + MINUTE + 1_100) as u64;
        i.frame_time = Some((T0 + MINUTE + 1_100) as u64);
        i.accounts.insert(account().to_string(), account_view("1000", held, json!({})));
    });
    let event = json!({ "type": "corporate_dividend", "id": "e", "market": 1, "account": account(),
        "quantity": "1", "amount": "5", "fx_mark": null });
    let seq = next_seq(&stand_in);
    stand_in.append((seq, 0), command(seq, T0 + MINUTE + 1_000, json!({ "type": "tick" }), json!([event])));
    assert_eq!(posts_after_settling(&stand_in).await, 0, "no close is signed on an account not at the log");

    // A read that agrees with the log ends it, and the close is signed.
    let position = json!({ "1": { "qty": "1", "entry": "99" } });
    stand_in.with(|i| i.accounts.insert(account().to_string(), account_view("1000", position, json!({}))));
    let seq = next_seq(&stand_in);
    stand_in.append((seq, 0), command(seq, T0 + MINUTE + 10_000, json!({ "type": "tick" }), json!([])));
    let (path, body) = posted(&stand_in, 1).await;
    assert_eq!(path, "/v1/positions/close-all");
    assert_eq!(body["closes"], json!([{ "market": 1, "quoted": "94" }]));
    stand_in.with(|i| i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({}))));
    let seq = next_seq(&stand_in);
    stand_in.append((seq, 0), own_close(seq, T0 + MINUTE + 10_100, &body));
    // The stop waits for the log to pass the close's expiry.
    stand_in.append((seq + 1, 0), command(seq + 1, T0 + 10 * MINUTE, json!({ "type": "tick" }), json!([])));
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap();
}

/// A close of two markets the engine rejects with no fill, or the node refuses for its nonce, moved
/// nothing: the next try closes both again, and does not wait for the first's request to expire.
#[tokio::test]
async fn a_rejected_close_of_two_markets_is_tried_again_for_both() {
    for nonce_refused in [false, true] {
        a_close_of_two_markets_is_tried_again_after(nonce_refused).await;
    }
}

/// That test with the first close rejected by the engine, or refused for its nonce.
async fn a_close_of_two_markets_is_tried_again_after(nonce_refused: bool) {
    let stand_in = StandIn::start().await;
    let dir = tempfile::tempdir().unwrap();
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99")), eth_usd(Some("99"))]);
        let positions = json!({ "1": { "qty": "1", "entry": "99" }, "2": { "qty": "1", "entry": "99" } });
        i.accounts.insert(account().to_string(), account_view("1000", positions, json!({})));
        i.head_seq = 10;
        i.nonce = 3;
        i.settle = false;
        if nonce_refused {
            i.statuses.push_back(409);
        } else {
            i.replies.push_back(json!([{ "type": "order_rejected", "account": account(), "client_id": 0,
                "reason": { "kind": "market_closed" } }]));
        }
    });
    let eth = "\n[[account.strategy]]\nmarket = \"ETH-USD\"\nbar = \"1m\"\nkind = \"sma_crossover\"\nfast = 1\n\
               slow = 2\nmax_slippage_bps = 25\n\n[sizing]";
    let edits = [LIMIT, ("\n[sizing]", eth), ("max_open_positions = 1", "max_open_positions = 2")];
    let (stop, bot) = start(&stand_in, dir.path(), &edits);
    warm(&stand_in).await;
    // The second market is priced as the first is, and its index is exact.
    for (seq, ts) in [(14, T0 + 7_000), (15, T0 + 8_001)] {
        stand_in.append((seq, 0), command(seq, ts, prices(2, "99", ts), json!([])));
    }
    // A read gives the node's time as a minute on, so the close's request, signed then, runs 30 s past it.
    let event = json!({ "type": "corporate_dividend", "id": "d", "market": 1, "account": account(),
        "quantity": "1", "amount": "5", "fx_mark": null });
    stand_in.with(|i| i.time_ms = (T0 + MINUTE) as u64);
    let seq = next_seq(&stand_in);
    stand_in.append((seq, 0), command(seq, T0 + 30_000, json!({ "type": "tick" }), json!([event])));
    assert_eq!(posts_after_settling(&stand_in).await, 0);
    price_at(&stand_in, T0 + MINUTE, "94");
    let (path, first) = posted(&stand_in, 1).await;
    assert_eq!(path, "/v1/positions/close-all");
    assert_eq!(first["closes"].as_array().unwrap().len(), 2, "both markets in one request");
    // The next try, past the retry time, names both again.
    let seq = next_seq(&stand_in);
    stand_in.append((seq, 0), command(seq, T0 + MINUTE + 6_000, json!({ "type": "tick" }), json!([])));
    let (path, second) = posted(&stand_in, 2).await;
    assert_eq!(path, "/v1/positions/close-all");
    assert_eq!(second["closes"].as_array().unwrap().len(), 2, "and both again");
    stand_in.with(|i| i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({}))));
    // The stop waits for the log to pass the requests' expiry.
    let seq = next_seq(&stand_in);
    stand_in.append((seq, 0), command(seq, T0 + 10 * MINUTE, json!({ "type": "tick" }), json!([])));
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap();
}
