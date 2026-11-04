//! The bot against the stand-in: its config, its run, and the log and account the tests script.

use std::path::Path;
use std::time::Duration;

use engine_core::eip712;
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
/// bars, a one-bar average over a two-bar one, one unit a side, closing on stop.
pub fn config(stand_in: &StandIn, dir: &Path, bar: &str) -> Config {
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
        vault = hex::encode(VAULT),
        signer = hex::encode(stand_in.signer()),
        pin = hex::encode(stand_in.pin),
    );
    let path = dir.join("bot.toml");
    std::fs::write(&path, text).unwrap();
    Config::load(&path).unwrap()
}

/// Runs the bot until `stop`.
pub async fn run(cfg: trader::config::Config, stop: impl Future<Output = ()> + Send + 'static) -> anyhow::Result<()> {
    trader::bot::run(cfg, |_| None, stop).await
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

/// Appends prices of 100 at T0 and T0 + 1.001 s as seqs 11 and 12, which make the bot's index exact,
/// and waits until the bot has warmed up from the marks history, so the node's time can move on.
pub async fn warm(stand_in: &StandIn) {
    for (seq, ts) in [(11, T0), (12, T0 + 1_001)] {
        stand_in.append((seq, 0), command(seq, ts, prices(1, "100", ts), json!([])));
    }
    while stand_in.with(|i| i.marks_reads) == 0 {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
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
