//! The console's funding as the tests drive it: a bot with funded and unfunded accounts on the stand-in
//! node and chain, a session, and the funding routes' answers.

use std::sync::Arc;
use std::time::Duration;

use hyper::Method;
use serde_json::{Value, json};
use trader::config::Config;

use crate::common::console::{Answer, Running, SAME, seed, within};
use crate::common::lifecycle::FUNDING_SECRET;
use crate::common::lifecycle::chain::ChainStandIn;
use crate::common::{StandIn, address, config_many};

pub const CONSOLE: &str = "\n[console]\nbind = \"127.0.0.1:0\"\ndata_dir = \"data\"\nrefresh_interval = \"1s\"\n";

/// The bot, with accounts 0 and 1 (only 0 credited, so only 0 trades), a chain and a funding key.
pub struct Funded {
    pub node: Arc<StandIn>,
    pub chain: Arc<ChainStandIn>,
    pub running: Running,
    pub cookie: String,
    _dir: tempfile::TempDir,
}

/// The `[chain]` section on `chain`, with the funding key when `funding`, and the console.
fn chain_section(chain: &ChainStandIn, funding: bool, waits: Waits) -> String {
    let funding = if funding { "[funding]\nkey = { type = \"hex_file\", path = \"funding.hex\" }\n" } else { "" };
    format!(
        "\n[chain]\nrpc = [\"{}\"]\nrpc_timeout = \"2s\"\nreceipt_timeout = \"{}\"\nsettle_timeout = \"{}\"\n\
         poll_interval = \"20ms\"\n{funding}{CONSOLE}",
        chain.url(),
        waits.receipt,
        waits.settle
    )
}

/// How long the bot waits for a transaction's receipt and for the node to settle what it sent.
#[derive(Clone, Copy)]
pub struct Waits {
    pub receipt: &'static str,
    pub settle: &'static str,
}

/// The waits most tests use.
pub const WAITS: Waits = Waits { receipt: "5s", settle: "3s" };

/// The bot with accounts 0 and 1, the funding key when `funding`.
pub async fn start(funding: bool) -> Funded {
    start_with(funding, &[0, 1]).await
}

/// As `start`, with the accounts `numbers` in the config, of which only account 0 is credited.
pub async fn start_with(funding: bool, numbers: &[u16]) -> Funded {
    start_waiting(funding, numbers, WAITS).await
}

/// As `start_with`, with the waits `waits`.
pub async fn start_waiting(funding: bool, numbers: &[u16], waits: Waits) -> Funded {
    let node = StandIn::start().await;
    let chain = ChainStandIn::start().await;
    seed(&node);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("funding.hex"), hex::encode(FUNDING_SECRET)).unwrap();
    let extra = chain_section(&chain, funding, waits);
    let cfg: Config = config_many(&node, dir.path(), "1m", numbers, &extra);
    for &n in numbers {
        node.with(|i| {
            i.lifecycle.answers.insert(
                format!("/v1/accounts/{}/eligibility", address(n)),
                (200, json!({ "class": null, "statement": null })),
            );
            for page in ["withdrawals", "payouts"] {
                let body = if page == "payouts" {
                    json!({ "seq": 40, "loaded": true, "payouts": [], "next": null })
                } else {
                    json!({ "seq": 20, "queue": [], "next": null })
                };
                i.lifecycle.answers.insert(format!("/v1/accounts/{}/{page}", address(n)), (200, body));
            }
        });
    }
    let running = Running::start(cfg).await;
    let cookie = running.session().await;
    Funded { node, chain, running, cookie, _dir: dir }
}

impl Funded {
    /// The funding read, once every account has been read.
    pub async fn funding(&self) -> Value {
        within(async {
            loop {
                let answer = self.running.get("/api/funding", &self.cookie).await;
                assert_eq!(answer.status, 200, "{answer:?}");
                if answer.body["accounts"].as_array().unwrap().iter().all(|a| a["reading"] == false) {
                    return answer.body;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
    }

    /// Asks for `action` with `body`.
    pub async fn act(&self, action: &str, body: Value) -> Answer {
        self.running.call(Method::POST, &format!("/api/funding/{action}"), SAME, Some(&self.cookie), Some(body)).await
    }

    /// The job `id`, once it has ended.
    pub async fn ended(&self, id: u64) -> Value {
        within(async {
            loop {
                let jobs = self.funding().await["jobs"].clone();
                if let Some(job) = jobs.as_array().unwrap().iter().find(|j| j["id"] == id)
                    && (job["state"] == "landed" || job["state"] == "failed")
                {
                    return job.clone();
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
    }
}

/// The id of the one job a request started.
pub fn job_id(answer: &Answer) -> u64 {
    assert_eq!(answer.status, 202, "{answer:?}");
    answer.body["jobs"][0]["id"].as_u64().unwrap()
}
