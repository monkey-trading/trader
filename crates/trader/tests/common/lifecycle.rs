//! The lifecycle against a stand-in node and a stand-in chain: its config, its keys, and the node's
//! answers the tests script.

pub mod chain;

use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use alloy_primitives::{Address, U256};
use axum::Router;
use axum::extract::State;
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use engine_core::eip712::{self, B256};
use engine_core::types::AccountId;
use k256::ecdsa::SigningKey;
use serde_json::{Value, json};
use tempfile::TempDir;
use trader::accounts::Lifecycle;
use trader::config::Config;
use trader::keys::AccountKey;

use self::chain::ChainStandIn;
use super::{ACCOUNT_SECRET, CHAIN_ID, StandIn, VAULT};

/// The funding key's secret: obviously fake.
pub const FUNDING_SECRET: [u8; 32] = [0x21; 32];

/// The stand-ins, the config's directory, the lifecycle over them, and the account's key.
pub struct Setup {
    pub node: Arc<StandIn>,
    pub chain: Arc<ChainStandIn>,
    pub dir: TempDir,
    pub cfg: Config,
    pub lifecycle: Lifecycle,
    pub key: AccountKey,
}

impl Setup {
    /// A lifecycle for the test account against fresh stand-ins, with a funding key when `funding`.
    pub async fn start(funding: bool) -> Self {
        let node = StandIn::start().await;
        let chain = ChainStandIn::start().await;
        let dir = TempDir::new().unwrap();
        let cfg = config(&node, &[chain.url()], dir.path(), funding, "");
        let lifecycle = Lifecycle::connect(&cfg, |_| None).await.unwrap();
        let key = AccountKey::load(&cfg.plans[0].key, |_| None).await.unwrap();
        Self { node, chain, dir, cfg, lifecycle, key }
    }

    /// Sets the node's answer to a GET of `path` to `body`, with `status`.
    pub fn answer(&self, path: &str, status: u16, body: Value) {
        self.node.with(|i| i.lifecycle.answers.insert(path.to_owned(), (status, body)));
    }

    /// The POSTs the node took to `path`.
    pub fn posts(&self, path: &str) -> Vec<Value> {
        self.node.with(|i| i.posts.iter().filter(|(p, _)| p == path).map(|(_, body)| body.clone()).collect())
    }
}

/// The longest a test waits for the bot to do something.
const WAIT: Duration = Duration::from_secs(20);

/// Waits until `done`, failing the test when it takes longer than `WAIT`.
pub async fn until(what: &str, done: impl Fn() -> bool) {
    let wait = async {
        while !done() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    tokio::time::timeout(WAIT, wait).await.unwrap_or_else(|_| panic!("gave up waiting for {what}"));
}

/// Runs `test` within `WAIT`, so a test that cannot finish fails instead of holding its run.
pub async fn within<T>(test: impl Future<Output = T>) -> T {
    tokio::time::timeout(WAIT, test).await.expect("the test did not finish in time")
}

/// The log lines a test's subscriber writes.
#[derive(Clone, Default)]
pub struct Lines(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Lines {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Lines {
    /// A subscriber that writes every line at `debug` and above here.
    pub fn subscriber(&self) -> impl tracing::Subscriber + Send + Sync {
        let lines = self.clone();
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_ansi(false)
            .with_writer(move || lines.clone())
            .finish()
    }

    /// What was written.
    pub fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

/// The test account.
pub fn account() -> AccountId {
    eip712::address(SigningKey::from_slice(&ACCOUNT_SECRET).unwrap().verifying_key())
}

/// The test account's wallet.
pub fn wallet() -> Address {
    Address::from(account().0)
}

/// The funding key's wallet.
pub fn funding_wallet() -> Address {
    Address::from(eip712::address(SigningKey::from_slice(&FUNDING_SECRET).unwrap().verifying_key()).0)
}

/// USDC base units of `usdc` whole USDC.
pub fn usdc(usdc: u64) -> U256 {
    U256::from(usdc) * U256::from(1_000_000)
}

/// A config for the stand-ins in `dir`, with the test account's key and, when `funding`, the funding
/// key in hex files, the chain's endpoints `rpc`, short waits, and `extra` appended.
pub fn config(node: &StandIn, rpc: &[String], dir: &Path, funding: bool, extra: &str) -> Config {
    std::fs::write(dir.join("account.hex"), hex::encode(ACCOUNT_SECRET)).unwrap();
    std::fs::write(dir.join("funding.hex"), hex::encode(FUNDING_SECRET)).unwrap();
    let rpc: Vec<String> = rpc.iter().map(|url| format!("{url:?}")).collect();
    let funding = if funding { "[funding]\nkey = { type = \"hex_file\", path = \"funding.hex\" }\n" } else { "" };
    let text = format!(
        r#"
[node]
url = "https://127.0.0.1:{port}"
chain_id = {CHAIN_ID}
vault = "0x{vault}"
signer = "0x{signer}"
tls_key_sha256 = "{pin}"
request_timeout = "2s"

[[account]]
key = {{ type = "hex_file", path = "account.hex" }}

[[account.strategy]]
market = "BTC-USD"
bar = "1m"
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

[chain]
rpc = [{rpc}]
rpc_timeout = "2s"
receipt_timeout = "5s"
settle_timeout = "5s"
poll_interval = "20ms"

{funding}
{extra}
"#,
        port = node.addr.port(),
        vault = hex::encode(VAULT),
        signer = hex::encode(node.signer()),
        pin = hex::encode(node.pin),
        rpc = rpc.join(", "),
    );
    let path = dir.join("bot.toml");
    std::fs::write(&path, text).unwrap();
    Config::load(&path).unwrap()
}

/// A history page of one custody record of deposit transaction `tx` to the test account:
/// credited, or uncredited for `uncredited`.
pub fn deposit_record(tx: B256, amount: &str, uncredited: Option<&str>) -> Value {
    let delta = if uncredited.is_some() { "0" } else { amount };
    json!({ "events": [{
        "cursor": "9:0", "ts": 1, "type": "custody_deposit_changed",
        "receipt": { "block": 16, "block_hash": B256::repeat_byte(1), "tx_hash": tx, "log_index": 0,
            "transaction_deposit_index": 0, "deposit_id": 0,
            "beneficiary": { "kind": "trader", "account": account() }, "amount": amount },
        "status": { "kind": "unfinalised" }, "principal_delta": delta, "uncredited": uncredited,
    }], "next": null })
}

/// The stand-in node's answers to the lifecycle: to the GETs no other route takes, by path without
/// its query, a status and a body; and how many times each path was read.
#[derive(Default)]
pub struct Canned {
    pub answers: BTreeMap<String, (u16, Value)>,
    pub reads: BTreeMap<String, usize>,
    /// The query string of each read of a path, in order.
    pub queries: BTreeMap<String, Vec<String>>,
    /// Answers given to the first reads of a path, before `answers` takes over.
    pub first: BTreeMap<String, VecDeque<(u16, Value)>>,
}

/// The lifecycle's routes: its POSTs, recorded and answered as orders are, and every path no other
/// route takes, answered from `Canned`.
pub fn routes() -> Router<Arc<StandIn>> {
    Router::new()
        .route("/v1/eligibility", post(super::record))
        .route("/v1/withdrawals", post(super::record))
        .route("/v1/accounts/{address}/withdrawals/{nonce}/reissue", post(super::record))
        .fallback(canned)
}

/// The canned answer to a GET of the path, or 404.
pub async fn canned(State(s): State<Arc<StandIn>>, uri: Uri) -> Response {
    let answer = s.with(|i| {
        *i.lifecycle.reads.entry(uri.path().to_owned()).or_default() += 1;
        i.lifecycle.queries.entry(uri.path().to_owned()).or_default().push(uri.query().unwrap_or_default().to_owned());
        let first = i.lifecycle.first.get_mut(uri.path()).and_then(VecDeque::pop_front);
        first.or_else(|| i.lifecycle.answers.get(uri.path()).cloned())
    });
    match answer {
        Some((status, body)) => (StatusCode::from_u16(status).unwrap(), axum::Json(body)).into_response(),
        None => (StatusCode::NOT_FOUND, axum::Json(json!({ "error": "not found" }))).into_response(),
    }
}
