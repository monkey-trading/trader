//! The bot against the stand-in: it reads the market and the account, follows the log's prices into
//! bars, orders toward the crossover's target with requests the engine's own hashes verify, takes
//! its position from its fills in the log, retries a declined order on the next bar, and on stopping
//! cancels resting orders and closes its position.

mod common;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use common::{ACCOUNT_SECRET, StandIn, btc_usd, command, prices};
use engine_core::command::{CancelAllRequest, CloseAllRequest, OrderRequest};
use engine_core::eip712::{self, cancel_all_hash, close_all_hash, order_hash, recover};
use engine_core::types::AccountId;
use k256::ecdsa::SigningKey;
use serde_json::{Value, json};
use tokio::sync::oneshot;
use trader::config::Config;

/// The stand-in's time: a minute boundary.
const T0: i64 = 1_800_000_000_000;
const MINUTE: i64 = 60_000;

#[tokio::test]
async fn trades_toward_the_crossover_and_stops_clean() {
    let stand_in = StandIn::start().await;
    let account = account();
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("100"))]);
        i.accounts.insert(account.to_string(), json!({ "balance": "1000", "positions": {}, "orders": {} }));
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
    let first = posted(&stand_in, 1).await;
    check_order(&first, T0 + MINUTE, "buy", "1", "102", false, 3);

    // The declined order is retried at the next bar, still long.
    stand_in.append((15, 0), command(15, T0 + 3 * MINUTE, prices(1, "103", T0 + 3 * MINUTE), json!([])));
    let second = posted(&stand_in, 2).await;
    check_order(&second, T0 + 2 * MINUTE, "buy", "1", "103", false, 4);

    // Its fill in the log makes the position long 1; at minute 4, 99 falls below the average of 103
    // and 99, so the target is short and the bot sells 2.
    let fill = json!({ "type": "fill", "account": account, "order_id": 1, "market": 1, "side": "buy", "qty": "1",
        "price": "103", "index": "103", "spread_bps": 0, "realised_pnl": "0", "profit_fee": "0", "position": "1",
        "liquidation": false });
    stand_in.append((16, 0), command(16, T0 + 3 * MINUTE + 1, json!({ "type": "tick" }), json!([fill])));
    stand_in.append((17, 0), command(17, T0 + 4 * MINUTE, prices(1, "99", T0 + 4 * MINUTE), json!([])));
    stand_in.append((18, 0), command(18, T0 + 5 * MINUTE, prices(1, "98", T0 + 5 * MINUTE), json!([])));
    let third = posted(&stand_in, 3).await;
    check_order(&third, T0 + 4 * MINUTE, "sell", "2", "98", false, 5);

    // Stopping reads the account: an order rests and a position is held, so it cancels and closes.
    stand_in.with(|i| {
        let view = json!({ "balance": "1000", "positions": { "1": { "qty": "-1", "entry": "98" } },
            "orders": { "9": { "order_id": 9, "client_id": 1, "market": 1, "side": "buy", "qty": "1",
                "kind": { "kind": "limit", "price": "50" }, "reduce_only": false } } });
        i.accounts.insert(account.to_string(), view);
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
    assert_eq!(body["closes"], json!([{ "market": 1, "quoted": "98" }]));
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
        i.accounts.insert(account().to_string(), json!({ "balance": "5", "positions": {}, "orders": {} }));
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
        i.accounts.insert(account().to_string(), json!({ "balance": "5", "positions": {}, "orders": {} }));
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
    let account = account();
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("100"))]);
        i.accounts.insert(account.to_string(), json!({ "balance": "1000", "positions": {}, "orders": {} }));
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
    stand_in.append((13, 0), command(13, T0 + 2 * MINUTE, prices(1, "99", T0 + 2 * MINUTE), json!([])));
    let order = posted(&stand_in, 1).await;
    check_order(&order, T0, "sell", "1", "99", false, 0);
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap();
}

// ── Helpers ──

/// The test account's address.
fn account() -> AccountId {
    eip712::address(SigningKey::from_slice(&ACCOUNT_SECRET).unwrap().verifying_key())
}

/// The domain the stand-in's deployment signs under.
fn domain() -> eip712::Eip712Domain {
    eip712::domain(common::CHAIN_ID, AccountId(common::VAULT))
}

/// Writes a config for the stand-in, with the test account's key in a hex file, and loads it.
fn config(stand_in: &StandIn, dir: &Path, bar: &str) -> Config {
    std::fs::write(dir.join("account.hex"), hex::encode(ACCOUNT_SECRET)).unwrap();
    let text = format!(
        r#"
[node]
url = "https://127.0.0.1:{port}"
chain_id = {chain_id}
vault = "0x{vault}"
signer = "0x{signer}"
tls_key_sha256 = "{pin}"
stream_timeout = "30s"

[account]
key = {{ type = "hex_file", path = "account.hex" }}
close_on_stop = true

[strategy]
market = "BTC-USD"
bar = "{bar}"
fast = 1
slow = 2
qty = "1"
max_slippage_bps = 25
"#,
        port = stand_in.addr.port(),
        chain_id = common::CHAIN_ID,
        vault = hex::encode(common::VAULT),
        signer = hex::encode(stand_in.signer()),
        pin = hex::encode(stand_in.pin),
    );
    let path = dir.join("bot.toml");
    std::fs::write(&path, text).unwrap();
    Config::load(&path).unwrap()
}

/// Runs the bot until `stop`.
async fn run(cfg: Config, stop: impl Future<Output = ()> + Send + 'static) -> anyhow::Result<()> {
    trader::bot::run(cfg, |_| None, stop).await
}

/// The `n`th POST once the stand-in has it.
async fn posted(stand_in: &Arc<StandIn>, n: usize) -> Value {
    for _ in 0..200 {
        if let Some((path, body)) = stand_in.with(|i| i.posts.get(n - 1).cloned()) {
            assert_eq!(path, "/v1/orders");
            return body;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no POST {n}: {:?}", stand_in.with(|i| i.posts.clone()));
}

/// Checks a posted order's fields and that its signature is the account's over the engine's hash.
fn check_order(body: &Value, client_id: i64, side: &str, qty: &str, quoted: &str, reduce_only: bool, nonce: u64) {
    assert_eq!(body["client_id"], client_id);
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
