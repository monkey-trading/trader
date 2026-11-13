//! Several accounts in one process against the stand-in: one stream connection for all of them,
//! each account on its own nonce, orders paced under the node's limit in the order bars completed,
//! and one account's failure leaving the others trading.

mod common;

use std::time::Duration;

use common::{
    MINUTE, StandIn, T0, account_view, address, btc_usd, bucket, command, config_many, domain, eth_usd, own_fill,
    prices, resting_order, run,
};
use engine_core::command::OrderRequest;
use engine_core::eip712::{order_hash, recover};
use serde_json::{Value, json};
use tokio::sync::oneshot;

/// A stand-in with BTC-USD, a funded account for each of `accounts`, and the marks history.
async fn node(accounts: &[u16]) -> std::sync::Arc<StandIn> {
    let stand_in = StandIn::start().await;
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99"))]);
        for &n in accounts {
            i.accounts.insert(address(n).to_string(), account_view("1000", json!({}), json!({})));
        }
        i.head_seq = 10;
        i.marks = vec![bucket(T0, "100")];
    });
    stand_in
}

/// Waits until the bot has read the marks history, once for every series it warms up.
async fn read_history(stand_in: &StandIn, reads: usize) {
    for _ in 0..400 {
        if stand_in.with(|i| i.marks_reads) >= reads {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the bot did not read the marks history");
}

/// Prices 100 at T0 and T0 + 1.001 s make the index exact, a tick five seconds later has the bot read
/// the history, and 101 in minute 1 and 102 in minute 2 complete bars closing at 100 and 101: long.
async fn go_long(stand_in: &StandIn) {
    stand_in.append((11, 0), command(11, T0, prices(1, "100", T0), json!([])));
    stand_in.append((12, 0), command(12, T0 + 1_001, prices(1, "100", T0 + 1_001), json!([])));
    stand_in.append((13, 0), command(13, T0 + 6_001, json!({ "type": "tick" }), json!([])));
    read_history(stand_in, 1).await;
    stand_in.append((14, 0), command(14, T0 + MINUTE, prices(1, "101", T0 + MINUTE), json!([])));
    stand_in.append((15, 0), command(15, T0 + 2 * MINUTE, prices(1, "102", T0 + 2 * MINUTE), json!([])));
}

/// The orders posted so far, as (account, nonce, signature recovers to the account).
fn orders(stand_in: &StandIn) -> Vec<(String, u64, bool)> {
    stand_in
        .with(|i| i.posts.clone())
        .into_iter()
        .filter(|(path, _)| path == "/v1/orders")
        .map(|(_, body)| {
            let order: OrderRequest = serde_json::from_value(body).unwrap();
            let auth = order.auth.clone().unwrap();
            let signer = recover(&order_hash(&domain(), &order, auth.nonce, auth.expires_at), &auth.signature);
            (order.account.to_string(), auth.nonce, signer == Some(order.account))
        })
        .collect()
}

/// Waits for `n` orders.
async fn wait_for_orders(stand_in: &StandIn, n: usize) {
    for _ in 0..400 {
        if orders(stand_in).len() >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("{} orders, wanted {n}: {:?}", orders(stand_in).len(), orders(stand_in));
}

/// Three accounts read the one stream connection, one series of bars, and each orders on its own
/// nonce with its own signature, over the few connections the pool holds.
#[tokio::test]
async fn accounts_share_a_stream_and_order_on_their_own_nonces() {
    let stand_in = node(&[0, 1, 2]).await;
    stand_in.with(|i| i.nonces.insert(address(1).to_string(), 5));
    let dir = tempfile::tempdir().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let bot = tokio::spawn(run(config_many(&stand_in, dir.path(), "1m", &[0, 1, 2], ""), async {
        _ = stopped.await;
    }));
    go_long(&stand_in).await;
    wait_for_orders(&stand_in, 3).await;
    let placed = orders(&stand_in);
    let by_account = |n: u16| placed.iter().find(|(account, ..)| *account == address(n).to_string()).unwrap();
    assert_eq!([0, 1, 2].map(|n| (by_account(n).1, by_account(n).2)), [(0, true), (5, true), (0, true)]);
    assert_eq!(
        placed.iter().map(|(account, ..)| account.clone()).collect::<Vec<_>>(),
        [0, 1, 2].map(|n| address(n).to_string()),
        "in the accounts' order"
    );
    assert_eq!(stand_in.with(|i| i.marks_reads), 1, "one history read for the one series");
    assert_eq!(stand_in.with(|i| i.connections.len()), 1, "one stream connection");
    // Each account's second order uses its own next nonce without asking the node, even though the
    // node's count has moved on.
    stand_in.with(|i| i.nonces.iter_mut().for_each(|(_, nonce)| *nonce = 100));
    stand_in.with(|i| i.time_ms = (T0 + 3 * MINUTE + 1_000) as u64);
    stand_in.append((16, 0), command(16, T0 + 3 * MINUTE, prices(1, "103", T0 + 3 * MINUTE), json!([])));
    wait_for_orders(&stand_in, 6).await;
    let again: Vec<_> = orders(&stand_in)[3..].iter().map(|(account, nonce, _)| (account.clone(), *nonce)).collect();
    assert_eq!(again, vec![(address(0).to_string(), 1), (address(1).to_string(), 6), (address(2).to_string(), 1),]);
    stand_in.with(|i| {
        i.accounts.values_mut().for_each(|view| view["positions"] = json!({}));
        i.time_ms = (T0 + 10 * MINUTE) as u64;
    });
    stand_in.tick(T0 + 10 * MINUTE);
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap();
    assert!(stand_in.with(|i| i.tls_connections) <= 5, "the stream and at most four pooled connections");
}

/// Orders for many accounts leave in the order their bar completed, at the node's rate, and the node
/// never has to refuse one.
#[tokio::test]
async fn a_burst_of_orders_queues_under_the_nodes_limit() {
    let accounts: Vec<u16> = (0..12).collect();
    let stand_in = node(&accounts).await;
    // The node allows 20 requests a second and a burst of 5, and so does the bot's config.
    stand_in.with(|i| i.limit = Some((20.0, 5.0)));
    let dir = tempfile::tempdir().unwrap();
    let extra =
        "\n[node.limits]\nper_ip = { per_second = 20, burst = 5 }\nper_account = { per_second = 10, burst = 20 }\n";
    let (stop, stopped) = oneshot::channel::<()>();
    let bot = tokio::spawn(run(config_many(&stand_in, dir.path(), "1m", &accounts, extra), async {
        _ = stopped.await;
    }));
    // Every account's first read is in before the bar, so none is out of date by the time it closes.
    tokio::time::timeout(Duration::from_secs(20), async {
        while stand_in.with(|i| i.account_reads) < accounts.len() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    go_long(&stand_in).await;
    wait_for_orders(&stand_in, 12).await;
    let placed: Vec<_> = orders(&stand_in).into_iter().map(|(account, ..)| account).collect();
    // Two bars close while the first one's reads are out, so an account read for the first is read again
    // for the second, behind the accounts that were read after it: each account is ordered once, and the
    // order within a bar is the order its reads were answered in.
    let mut once = placed.clone();
    once.sort();
    once.dedup();
    assert_eq!(placed.len(), once.len(), "one order for each account");
    assert_eq!(once, {
        let mut all: Vec<_> = accounts.iter().map(|&n| address(n).to_string()).collect();
        all.sort();
        all
    });
    stand_in.with(|i| {
        i.limit = None;
        i.accounts.values_mut().for_each(|view| view["positions"] = json!({}));
        i.time_ms = (T0 + 10 * MINUTE) as u64;
    });
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(30), bot).await.unwrap().unwrap().unwrap();
    assert_eq!(stand_in.with(|i| i.rate_limited), 0, "no request was refused for rate");
}

/// An account the node never credited and one with no balance are reported and left out; the
/// funded one trades, and the run ends in an error naming them.
#[tokio::test]
async fn accounts_that_cannot_start_do_not_stop_the_others() {
    let stand_in = node(&[0]).await;
    stand_in.with(|i| {
        let held = json!({ "2": { "qty": "-1", "entry": "5" } });
        i.accounts.insert(address(2).to_string(), account_view("1000", held, json!({})));
    });
    let dir = tempfile::tempdir().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let bot = tokio::spawn(run(config_many(&stand_in, dir.path(), "1m", &[0, 1, 2], ""), async {
        _ = stopped.await;
    }));
    go_long(&stand_in).await;
    wait_for_orders(&stand_in, 1).await;
    assert_eq!(orders(&stand_in)[0].0, address(0).to_string());
    stand_in.with(|i| {
        i.accounts.get_mut(&address(0).to_string()).unwrap()["positions"] = json!({});
        i.time_ms = (T0 + 10 * MINUTE) as u64;
    });
    stop.send(()).unwrap();
    let err = tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap_err();
    assert!(err.to_string().contains("2 accounts could not start"), "{err:#}");
    assert_eq!(orders(&stand_in).len(), 1, "nothing was sent for the accounts that did not start");
}

/// A fill no order of this process signed stops its account and no other: the other keeps ordering,
/// and the stop leaves the stopped account as it is.
#[tokio::test]
async fn a_foreign_fill_stops_one_account_and_the_others_trade_on() {
    let stand_in = node(&[0, 1]).await;
    let dir = tempfile::tempdir().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let bot = tokio::spawn(run(config_many(&stand_in, dir.path(), "1m", &[0, 1], ""), async {
        _ = stopped.await;
    }));
    go_long(&stand_in).await;
    wait_for_orders(&stand_in, 2).await;
    // Someone else's order fills the first account, with a signature the bot never made.
    let theirs: Value = json!({ "account": address(0), "client_id": 1, "market": 1, "side": "buy", "qty": "1",
        "kind": { "kind": "market", "quoted": "100", "max_slippage_bps": 25 }, "reduce_only": false,
        "auth": { "nonce": 7, "expires_at": T0, "signature": format!("0x{}", "11".repeat(65)) } });
    stand_in.append((16, 0), own_fill(16, T0 + 2 * MINUTE + 40_000, &theirs, "1"));
    stand_in.with(|i| i.time_ms = (T0 + 3 * MINUTE + 1_000) as u64);
    stand_in.append((17, 0), command(17, T0 + 3 * MINUTE, prices(1, "103", T0 + 3 * MINUTE), json!([])));
    wait_for_orders(&stand_in, 3).await;
    stand_in.append((18, 0), command(18, T0 + 4 * MINUTE, prices(1, "104", T0 + 4 * MINUTE), json!([])));
    stand_in.applied((18, 0)).await;
    let accounts: Vec<_> = orders(&stand_in).into_iter().map(|(account, ..)| account).collect();
    assert_eq!(
        accounts,
        [address(0), address(1), address(1), address(1)].map(|a| a.to_string()),
        "the first account placed no more"
    );
    stand_in.with(|i| {
        i.accounts.values_mut().for_each(|view| {
            view["positions"] = json!({});
            view["orders"] = json!({ "9": resting_order(9) });
        });
        i.time_ms = (T0 + 10 * MINUTE) as u64;
    });
    stop.send(()).unwrap();
    let err = tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap_err();
    assert!(err.to_string().contains("1 stopped trading"), "{err:#}");
    let cancels: Vec<_> = stand_in
        .with(|i| i.posts.clone())
        .into_iter()
        .filter(|(path, _)| path == "/v1/orders/cancel-all")
        .map(|(_, body)| body["account"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(cancels, [address(1).to_string()], "the stopped account is left as it is");
}

/// The account's own count of its nonce carries it through orders the node answered; a rejection
/// for the nonce sends it back to the node's count before the next one.
#[tokio::test]
async fn a_rejected_nonce_is_read_again() {
    let stand_in = node(&[0]).await;
    let dir = tempfile::tempdir().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let bot = tokio::spawn(run(config_many(&stand_in, dir.path(), "1m", &[0], ""), async {
        _ = stopped.await;
    }));
    go_long(&stand_in).await;
    wait_for_orders(&stand_in, 1).await;
    // The next order is refused for its nonce; the node's count is 40.
    stand_in.with(|i| {
        i.replies.push_back(json!([{ "type": "order_rejected", "account": address(0), "client_id": 1,
            "reason": { "kind": "invalid_nonce", "expected": 40 } }]));
        i.time_ms = (T0 + 3 * MINUTE + 1_000) as u64;
    });
    stand_in.append((16, 0), command(16, T0 + 3 * MINUTE, prices(1, "103", T0 + 3 * MINUTE), json!([])));
    wait_for_orders(&stand_in, 2).await;
    stand_in.with(|i| i.nonces.insert(address(0).to_string(), 40));
    stand_in.append((17, 0), command(17, T0 + 4 * MINUTE, prices(1, "104", T0 + 4 * MINUTE), json!([])));
    wait_for_orders(&stand_in, 3).await;
    let nonces: Vec<_> = orders(&stand_in).into_iter().map(|(_, nonce, _)| nonce).collect();
    assert_eq!(nonces, [0, 1, 40]);
    stand_in.with(|i| {
        i.accounts.values_mut().for_each(|view| view["positions"] = json!({}));
        i.time_ms = (T0 + 10 * MINUTE) as u64;
    });
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap();
}

/// An account the node refuses (a 403) and one whose signature the engine rejects stop, each on its
/// own, and the third goes on ordering; the run ends in an error that counts the two.
#[tokio::test]
async fn a_refused_key_stops_only_its_account() {
    let stand_in = node(&[0, 1, 2]).await;
    stand_in.with(|i| {
        i.statuses.push_back(403);
        i.replies.push_back(json!([{ "type": "order_rejected", "account": address(1), "client_id": 1,
            "reason": { "kind": "invalid_signature" } }]));
    });
    let dir = tempfile::tempdir().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let bot = tokio::spawn(run(config_many(&stand_in, dir.path(), "1m", &[0, 1, 2], ""), async {
        _ = stopped.await;
    }));
    go_long(&stand_in).await;
    wait_for_orders(&stand_in, 3).await;
    stand_in.with(|i| i.time_ms = (T0 + 3 * MINUTE + 1_000) as u64);
    stand_in.append((16, 0), command(16, T0 + 3 * MINUTE, prices(1, "103", T0 + 3 * MINUTE), json!([])));
    wait_for_orders(&stand_in, 4).await;
    assert_eq!(orders(&stand_in)[3].0, address(2).to_string(), "only the third account orders on");
    stand_in.with(|i| {
        i.accounts.values_mut().for_each(|view| view["positions"] = json!({}));
        i.time_ms = (T0 + 10 * MINUTE) as u64;
    });
    stand_in.tick(T0 + 10 * MINUTE);
    stop.send(()).unwrap();
    let err = tokio::time::timeout(Duration::from_secs(30), bot).await.unwrap().unwrap().unwrap_err();
    assert!(err.to_string().contains("2 stopped trading"), "{err:#}");
}

/// An account that trades two markets is closed in both on stopping, one close for each slippage bound
/// its strategies set, each market quoted at the node's mark.
#[tokio::test]
async fn a_stop_closes_every_market_an_account_trades_under_its_own_bound() {
    let stand_in = StandIn::start().await;
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99")), eth_usd(Some("50"))]);
        let positions = json!({ "1": { "qty": "2", "entry": "99" }, "2": { "qty": "-3", "entry": "50" } });
        i.accounts.insert(address(0).to_string(), account_view("1000", positions, json!({})));
        i.head_seq = 10;
    });
    stand_in.tick(T0 + MINUTE);
    let dir = tempfile::tempdir().unwrap();
    let eth = "\n[[account.strategy]]\nmarket = \"ETH-USD\"\nbar = \"1m\"\nkind = \"sma_crossover\"\nfast = 1\nslow = 2\nmax_slippage_bps = 10\n";
    run(config_many(&stand_in, dir.path(), "1m", &[0], eth), async {}).await.unwrap();
    let closes: Vec<_> = stand_in
        .with(|i| i.posts.clone())
        .into_iter()
        .filter(|(path, _)| path == "/v1/positions/close-all")
        .map(|(_, body)| (body["max_slippage_bps"].clone(), body["closes"].clone()))
        .collect();
    assert_eq!(
        closes,
        [(json!(10), json!([{ "market": 2, "quoted": "50" }])), (json!(25), json!([{ "market": 1, "quoted": "99" }])),]
    );
}

/// An answer signed by another key to one account's startup nonce read ends the run, though the
/// answers about the other accounts are the signer's.
#[tokio::test]
async fn a_wrong_signer_on_a_startup_nonce_read_ends_the_run() {
    let stand_in = node(&[0, 1]).await;
    stand_in.with(|i| i.other_signer_path = Some(format!("{}/nonce", address(1))));
    let dir = tempfile::tempdir().unwrap();
    let err = run(config_many(&stand_in, dir.path(), "1m", &[0, 1], ""), std::future::pending()).await.unwrap_err();
    assert!(err.to_string().contains("not signed by the configured signer"), "{err:#}");
    assert!(stand_in.with(|i| i.posts.is_empty() && i.connections.is_empty()), "nothing was sent, no stream opened");
}

/// One account that cannot be read when the stop comes is left as it was and reported, while the other
/// account's stop completes.
#[tokio::test]
async fn one_account_that_will_not_stop_does_not_hold_back_the_other() {
    let stand_in = node(&[0, 1]).await;
    stand_in.with(|i| {
        for view in i.accounts.values_mut() {
            view["orders"] = json!({ "9": resting_order(9) });
        }
    });
    let dir = tempfile::tempdir().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let bot = tokio::spawn(run(config_many(&stand_in, dir.path(), "1m", &[0, 1], ""), async {
        _ = stopped.await;
    }));
    while stand_in.with(|i| i.connections.is_empty()) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // The node no longer knows the second account.
    stand_in.with(|i| i.accounts.remove(&address(1).to_string()));
    stand_in.tick(T0 + MINUTE);
    stop.send(()).unwrap();
    let err = tokio::time::timeout(Duration::from_secs(30), bot).await.unwrap().unwrap().unwrap_err();
    assert!(err.to_string().contains("1 accounts were not left as the stop sets out"), "{err:#}");
    let cancels: Vec<_> = stand_in
        .with(|i| i.posts.clone())
        .into_iter()
        .filter(|(path, _)| path == "/v1/orders/cancel-all")
        .map(|(_, body)| body["account"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(cancels.len(), 2, "the account that could not be read is cancelled for too: {cancels:?}");
    assert!(stand_in.with(|i| i.accounts[&address(0).to_string()]["orders"] == json!({})), "the first is stopped");
}
