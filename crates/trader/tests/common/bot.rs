//! The bot against the stand-in: its config, its run, and the log and account the tests script.

use std::path::Path;
use std::time::Duration;

use engine_core::command::OrderRequest;
use engine_core::eip712::{self, order_hash, recover};
use engine_core::types::AccountId;
use k256::ecdsa::SigningKey;
use serde_json::{Value, json};
use trader::config::Config;

use super::{ACCOUNT_SECRET, CHAIN_ID, StandIn, VAULT, command, prices};

/// The stand-in's time at start: a minute boundary.
pub const T0: i64 = 1_800_000_000_000;
pub const MINUTE: i64 = 60_000;

/// The test account's address.
pub fn account() -> AccountId {
    eip712::address(SigningKey::from_slice(&ACCOUNT_SECRET).unwrap().verifying_key())
}

/// The domain the stand-in's deployment signs under.
pub fn domain() -> eip712::Eip712Domain {
    eip712::domain(CHAIN_ID, AccountId(VAULT))
}

/// A config for the stand-in, with the test account's key in a hex file in `dir`: BTC-USD, `bar`
/// bars, a one-bar average over a two-bar one, 100 USD a side, closing on stop.
pub fn config(stand_in: &StandIn, dir: &Path, bar: &str) -> Config {
    config_with(stand_in, dir, bar, &[])
}

/// `config` with each `(from, to)` replacing the first `from` in the config's text.
pub fn config_with(stand_in: &StandIn, dir: &Path, bar: &str, edits: &[(&str, &str)]) -> Config {
    std::fs::write(dir.join("account.hex"), hex::encode(ACCOUNT_SECRET)).unwrap();
    let text = format!(
        r#"
[node]
url = "https://127.0.0.1:{port}"
chain_id = {CHAIN_ID}
vault = "0x{vault}"
signer = "0x{signer}"
tls_key_sha256 = "{pin}"
request_timeout = "2s"
stream_timeout = "30s"

[[account]]
key = {{ type = "hex_file", path = "account.hex" }}
close_on_stop = true

[[account.strategy]]
market = "BTC-USD"
bar = "{bar}"
kind = "sma_crossover"
fast = 1
slow = 2
max_slippage_bps = 25

[sizing]
by = "fixed_notional"
notional = "100"
max_leverage = "50"

[risk]
max_open_positions = 1
max_total_notional = "100000"
daily_loss = {{ usd = "100000" }}
margin_buffer_bps = 0
min_hold = "0s"
"#,
        port = stand_in.addr.port(),
        vault = hex::encode(VAULT),
        signer = hex::encode(stand_in.signer()),
        pin = hex::encode(stand_in.pin),
    );
    let text = edits.iter().fold(text, |text, (from, to)| {
        assert!(text.contains(from), "{from:?} is not in the config");
        text.replacen(from, to, 1)
    });
    let path = dir.join("bot.toml");
    std::fs::write(&path, text).unwrap();
    Config::load(&path).unwrap()
}

/// The secret of test account `n`, obviously fake: the first, `account()`'s, is `ACCOUNT_SECRET`.
pub fn secret(n: u8) -> [u8; 32] {
    if n == 0 { ACCOUNT_SECRET } else { [0x20 + n; 32] }
}

/// Test account `n`'s address.
pub fn address(n: u8) -> AccountId {
    eip712::address(SigningKey::from_slice(&secret(n)).unwrap().verifying_key())
}

/// A config for the stand-in with test accounts `accounts`, each trading BTC-USD in `bar` bars as
/// `config`'s does and closing on stop, and `extra` TOML after the rest.
pub fn config_many(stand_in: &StandIn, dir: &Path, bar: &str, accounts: &[u8], extra: &str) -> Config {
    let mut text = format!(
        r#"
[node]
url = "https://127.0.0.1:{port}"
chain_id = {CHAIN_ID}
vault = "0x{vault}"
signer = "0x{signer}"
tls_key_sha256 = "{pin}"
request_timeout = "2s"
stream_timeout = "30s"

[sizing]
by = "fixed_notional"
notional = "100"
max_leverage = "50"

[risk]
max_open_positions = 16
max_total_notional = "100000"
daily_loss = {{ usd = "100000" }}
margin_buffer_bps = 0
min_hold = "0s"
"#,
        port = stand_in.addr.port(),
        vault = hex::encode(VAULT),
        signer = hex::encode(stand_in.signer()),
        pin = hex::encode(stand_in.pin),
    );
    for &n in accounts {
        std::fs::write(dir.join(format!("account{n}.hex")), hex::encode(secret(n))).unwrap();
        text += &format!(
            r#"
[[account]]
key = {{ type = "hex_file", path = "account{n}.hex" }}
close_on_stop = true

[[account.strategy]]
market = "BTC-USD"
bar = "{bar}"
kind = "sma_crossover"
fast = 1
slow = 2
max_slippage_bps = 25
"#
        );
    }
    text += extra;
    let path = dir.join("bot.toml");
    std::fs::write(&path, text).unwrap();
    Config::load(&path).unwrap()
}

/// Runs the bot until `stop`.
pub async fn run(cfg: trader::config::Config, stop: impl Future<Output = ()> + Send + 'static) -> anyhow::Result<()> {
    // `TRADER_TEST_LOG=info` (or `debug`) shows the bot's log in a failing test's output.
    if let Ok(filter) = std::env::var("TRADER_TEST_LOG") {
        let _ = tracing_subscriber::fmt().with_env_filter(filter).with_test_writer().try_init();
    }
    // A run that never ends fails its test instead of hanging the suite.
    tokio::time::timeout(Duration::from_secs(120), trader::bot::run(cfg, |_| None, stop))
        .await
        .unwrap_or_else(|_| Err(anyhow::anyhow!("the bot did not finish within two minutes")))
}

/// The `n`th POST, from 1, once the stand-in has it.
pub async fn posted(stand_in: &StandIn, n: usize) -> (String, Value) {
    for _ in 0..200 {
        if let Some(post) = stand_in.with(|i| i.posts.get(n - 1).cloned()) {
            return post;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no POST {n}: {:?}", stand_in.with(|i| i.posts.clone()));
}

/// Appends prices of 100 at T0 and T0 + 1.001 s as seqs 11 and 12, which make the bot's index
/// exact, and a tick 5 s later as seq 13, when it reads the marks history, whose minute 0 is 100;
/// waits until it has, so the node's time can move on.
pub async fn warm(stand_in: &StandIn) {
    stand_in.with(|i| i.marks = vec![bucket(T0, "100")]);
    for (seq, ts) in [(11, T0), (12, T0 + 1_001)] {
        stand_in.append((seq, 0), command(seq, ts, prices(1, "100", ts), json!([])));
    }
    stand_in.append((13, 0), command(13, T0 + 6_001, json!({ "type": "tick" }), json!([])));
    for _ in 0..500 {
        if stand_in.with(|i| i.marks_reads) > 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the bot did not read the marks history");
}

/// A log-derived one-minute bucket of marks starting at `ts` and closing at `close`.
pub fn bucket(ts: i64, close: &str) -> Value {
    json!({ "ts": ts, "open": close, "high": close, "low": close, "close": close, "provenance": "log_derived" })
}

/// The command that placed the posted order `body`, with its fill leaving `position`.
pub fn own_fill(seq: u64, ts: i64, body: &Value, position: &str) -> Value {
    let mut cmd = body.clone();
    cmd["type"] = json!("place_order");
    let fill = json!({ "type": "fill", "account": account(), "order_id": seq, "market": 1, "side": body["side"],
        "qty": body["qty"], "price": "100", "index": "100", "spread_bps": 0, "realised_pnl": "0",
        "profit_fee": "0", "position": position, "liquidation": false });
    command(seq, ts, cmd, json!([fill]))
}

/// An account view with `balance`, positions and resting orders.
pub fn account_view(balance: &str, positions: Value, orders: Value) -> Value {
    json!({ "balance": balance, "positions": positions, "orders": orders })
}

/// A resting limit order, as an account view lists it.
pub fn resting_order(order_id: u64) -> Value {
    json!({ "order_id": order_id, "client_id": 1, "market": 1, "side": "buy", "qty": "1",
        "kind": { "kind": "limit", "price": "50" }, "reduce_only": false })
}

/// The `n`th POST, which must be an order.
pub async fn order(stand_in: &StandIn, n: usize) -> Value {
    let (path, body) = posted(stand_in, n).await;
    assert_eq!(path, "/v1/orders", "{body}");
    body
}

/// Checks a posted order's fields and that its signature is the account's over the engine's hash.
pub fn check_order(body: &Value, client_id: i64, side: &str, qty: &str, quoted: &str, reduce_only: bool, nonce: u64) {
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
    // The expiry counts from the node's signed time, counted on by the clock since the answer.
    assert!((T0 + 30_000..T0 + 35_000).contains(&auth.expires_at), "{}", auth.expires_at);
    let hash = order_hash(&domain(), &order, auth.nonce, auth.expires_at);
    assert_eq!(recover(&hash, &auth.signature), Some(account()));
}
