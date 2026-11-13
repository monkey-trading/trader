//! The console's funding against the stand-in node and chain: it reads every account that has a key,
//! one that did not start included, and each action runs for the account it names, signed or paid
//! by that account's own key, as a job the read follows to its end.

mod common;

use std::time::Duration;

use engine_core::access::{EligibilityClass, EligibilityRequest};
use engine_core::eip712::{self, eligibility_hash, recover};
use engine_core::types::AccountId;
use hyper::Method;
use serde_json::{Value, json};
use trader::config::Config;

use common::console::{Answer, Running, seed, within};
use common::lifecycle::chain::ChainStandIn;
use common::lifecycle::{FUNDING_SECRET, usdc, wallet};
use common::{CHAIN_ID, StandIn, VAULT, address, config_many};

const CONSOLE: &str = "\n[console]\nbind = \"127.0.0.1:0\"\ndata_dir = \"data\"\nrefresh_interval = \"1s\"\n";

/// The bot, with accounts 0 and 1 (only 0 credited, so only 0 trades), a chain and a funding key.
struct Funded {
    node: std::sync::Arc<StandIn>,
    chain: std::sync::Arc<ChainStandIn>,
    running: Running,
    cookie: String,
    _dir: tempfile::TempDir,
}

fn chain_section(chain: &ChainStandIn, funding: bool) -> String {
    let funding = if funding { "[funding]\nkey = { type = \"hex_file\", path = \"funding.hex\" }\n" } else { "" };
    format!(
        "\n[chain]\nrpc = [\"{}\"]\nrpc_timeout = \"2s\"\nreceipt_timeout = \"5s\"\nsettle_timeout = \"3s\"\n\
         poll_interval = \"20ms\"\n{funding}{CONSOLE}",
        chain.url()
    )
}

async fn start(funding: bool) -> Funded {
    let node = StandIn::start().await;
    let chain = ChainStandIn::start().await;
    seed(&node);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("funding.hex"), hex::encode(FUNDING_SECRET)).unwrap();
    let extra = chain_section(&chain, funding);
    let cfg: Config = config_many(&node, dir.path(), "1m", &[0, 1], &extra);
    for n in [0, 1] {
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
    async fn funding(&self) -> Value {
        let answer = self.running.get("/api/funding", &self.cookie).await;
        assert_eq!(answer.status, 200, "{answer:?}");
        answer.body
    }

    async fn act(&self, action: &str, body: Value) -> Answer {
        self.running
            .call(
                Method::POST,
                &format!("/api/funding/{action}"),
                common::console::SAME,
                Some(&self.cookie),
                Some(body),
            )
            .await
    }

    /// The job `id`, once it has ended.
    async fn ended(&self, id: u64) -> Value {
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

fn job_id(answer: &Answer) -> u64 {
    assert_eq!(answer.status, 202, "{answer:?}");
    answer.body["jobs"][0]["id"].as_u64().unwrap()
}

/// Every account with an unlocked key is listed in the config's order, the one that never started
/// too, each with what it holds on chain and in the node; the answer needs a session.
#[tokio::test]
async fn the_read_lists_every_account_with_a_key() {
    let f = start(true).await;
    f.chain.with(|l| {
        l.usdc.insert(wallet(), usdc(7));
    });
    let body = f.funding().await;
    assert_eq!((body["enabled"].clone(), body["funding_key"].clone()), (json!(true), json!(true)));
    let accounts = body["accounts"].as_array().unwrap();
    let listed: Vec<_> = accounts.iter().map(|a| a["address"].clone()).collect();
    assert_eq!(listed, [json!(address(0).to_string()), json!(address(1).to_string())]);
    assert_eq!(accounts[0]["wallet_usdc"], "7");
    assert_eq!(accounts[0]["balance"], "1000");
    assert_eq!(accounts[0]["declared"], Value::Null);
    assert_eq!(accounts[1]["balance"], Value::Null, "the account the node never credited");
    assert_eq!(body["statements"]["professional"], EligibilityClass::Professional.statement());
    let anonymous = f.running.call(Method::GET, "/api/funding", None, None, None).await;
    assert_eq!(anonymous.status, 401);
}

/// Without a `[chain]` section the read says funding is off, and an action is refused.
#[tokio::test]
async fn without_a_chain_funding_is_off() {
    let node = StandIn::start().await;
    seed(&node);
    let dir = tempfile::tempdir().unwrap();
    let running = Running::start(config_many(&node, dir.path(), "1m", &[0], CONSOLE)).await;
    let cookie = running.session().await;
    assert_eq!(running.get("/api/funding", &cookie).await.body, json!({ "enabled": false }));
    let body = json!({ "accounts": [address(0).to_string()] });
    let refused =
        running.call(Method::POST, "/api/funding/claim", common::console::SAME, Some(&cookie), Some(body)).await;
    assert_eq!((refused.status, refused.body["code"].clone()), (409, json!("funding_off")));
}

/// A declaration is refused unless it carries the class's own statement; with it, it is signed by the
/// named account's key, recorded, and the account that never started can declare as well.
#[tokio::test]
async fn a_declaration_carries_its_statement_and_is_signed_by_the_account() {
    let f = start(false).await;
    let second = address(1);
    let wrong = f
        .act("declare", json!({ "class": "professional", "statement": "I agree.", "accounts": [second.to_string()] }))
        .await;
    assert_eq!((wrong.status, wrong.body["code"].clone()), (422, json!("statement_mismatch")));
    assert!(f.node.with(|i| i.posts.is_empty()));
    f.node.with(|i| {
        i.replies.push_back(json!([{ "type": "eligibility_attested", "account": second, "class": "professional" }]));
    });
    let statement = EligibilityClass::Professional.statement();
    let started = f
        .act("declare", json!({ "class": "professional", "statement": statement, "accounts": [second.to_string()] }))
        .await;
    let job = f.ended(job_id(&started)).await;
    assert_eq!((job["state"].clone(), job["account"].clone()), (json!("landed"), json!(second.to_string())));
    let posts = f.node.with(|i| i.posts.clone());
    assert_eq!(posts.len(), 1);
    let request: EligibilityRequest = serde_json::from_value(posts[0].1.clone()).unwrap();
    let hash = eligibility_hash(
        &eip712::domain(CHAIN_ID, AccountId(VAULT)),
        request.account,
        request.class,
        request.auth.nonce,
        request.auth.expires_at,
    );
    assert_eq!(recover(&hash, &request.auth.signature), Some(second), "signed by the account named");
}

/// A request naming an address the bot has no key for, no account, or a deposit from a funding key
/// the config does not have, starts nothing.
#[tokio::test]
async fn a_request_that_names_no_account_of_the_bots_starts_nothing() {
    let f = start(false).await;
    let stranger = AccountId([9; 20]).to_string();
    let unknown = f.act("claim", json!({ "accounts": [address(0).to_string(), stranger] })).await;
    assert_eq!((unknown.status, unknown.body["code"].clone()), (404, json!("no_account")));
    let none = f.act("claim", json!({ "accounts": [] })).await;
    assert_eq!((none.status, none.body["code"].clone()), (422, json!("bad_accounts")));
    let from =
        f.act("deposit", json!({ "accounts": [address(0).to_string()], "amount": "5", "from": "funding" })).await;
    assert_eq!((from.status, from.body["code"].clone()), (422, json!("no_funding_key")));
    let dust = f
        .act("deposit", json!({ "accounts": [address(0).to_string()], "amount": "0.0000001", "from": "account" }))
        .await;
    assert_eq!((dust.status, dust.body["code"].clone()), (422, json!("bad_amount")));
    assert!(f.funding().await["jobs"].as_array().unwrap().is_empty());
    assert!(f.chain.sent().is_empty());
}

/// An account with a job that has not ended is refused another, a job that fails says why, and the
/// transactions it sent are the account's own.
#[tokio::test]
async fn a_busy_account_is_refused_and_a_failed_job_says_why() {
    let f = start(false).await;
    let first = address(0).to_string();
    f.chain.with(|l| {
        l.usdc.insert(wallet(), usdc(500));
    });
    // The node never credits it, so the job waits out the settle time.
    f.node.with(|i| {
        i.lifecycle
            .answers
            .insert(format!("/v1/accounts/{first}/history"), (200, json!({ "events": [], "next": null })));
    });
    let started = f.act("deposit", json!({ "accounts": [first], "amount": "100", "from": "account" })).await;
    let id = job_id(&started);
    let again = f.act("claim", json!({ "accounts": [first] })).await;
    assert_eq!((again.status, again.body["code"].clone()), (409, json!("busy")));
    let job = f.ended(id).await;
    assert_eq!(job["state"], "failed");
    assert!(!job["detail"].as_str().unwrap().is_empty());
    assert!(f.chain.sent().iter().all(|sent| sent.from == wallet()), "the account's own wallet sent them");
    let free = f.act("claim", json!({ "accounts": [first] })).await;
    let claimed = f.ended(job_id(&free)).await;
    assert_eq!((claimed["state"].clone(), claimed["detail"].clone()), (json!("landed"), json!("no MONKEY to claim")));
}

/// A withdrawal with nothing queued or approved pays nothing, and says so; figures that changed are
/// read again.
#[tokio::test]
async fn a_withdrawal_with_nothing_approved_pays_nothing() {
    let f = start(false).await;
    let started = f.act("withdraw", json!({ "accounts": [address(0).to_string()] })).await;
    let job = f.ended(job_id(&started)).await;
    assert_eq!((job["state"].clone(), job["detail"].clone()), (json!("landed"), json!("nothing to pay")));
}

/// A deposit from the funding wallet is sent by the funding key for the named account, and its job
/// lands with the transaction's hash once the node credits it.
#[tokio::test]
async fn a_deposit_from_the_funding_wallet_lands_with_its_transaction() {
    let f = start(true).await;
    let first = address(0).to_string();
    f.chain.with(|l| {
        l.usdc.insert(common::lifecycle::funding_wallet(), usdc(1_000));
    });
    let path = format!("/v1/accounts/{first}/history");
    f.node.with(|i| i.lifecycle.answers.insert(path.clone(), (200, json!({ "events": [], "next": null }))));
    let started = f.act("deposit", json!({ "accounts": [first], "amount": "100", "from": "funding" })).await;
    let id = job_id(&started);
    let sent = f.chain.sent_count(2).await;
    f.node.with(|i| {
        i.lifecycle.answers.insert(path, (200, common::lifecycle::deposit_record(sent.hash, "100", None)));
    });
    let job = f.ended(id).await;
    assert_eq!(job["state"], "landed", "{job}");
    assert_eq!(job["txs"], json!([sent.hash.to_string()]));
    assert!(f.chain.sent().iter().all(|s| s.from == common::lifecycle::funding_wallet()), "the funding key pays");
}

/// No request can name where funds go: a field for a destination is refused, so a withdrawal and a
/// claim pay only the account they are for.
#[tokio::test]
async fn no_request_names_a_destination() {
    let f = start(true).await;
    let first = address(0).to_string();
    let elsewhere = AccountId([7; 20]).to_string();
    for (action, body) in [
        ("withdraw", json!({ "accounts": [first], "amount": "5", "to": elsewhere })),
        ("withdraw", json!({ "accounts": [first], "recipient": elsewhere })),
        ("claim", json!({ "accounts": [first], "to": elsewhere })),
        ("deposit", json!({ "accounts": [first], "amount": "5", "from": "account", "for": elsewhere })),
    ] {
        let answer = f.act(action, body).await;
        assert_eq!((answer.status, answer.body["code"].clone()), (422, json!("bad_request")), "{action}: {answer:?}");
    }
    assert!(f.funding().await["jobs"].as_array().unwrap().is_empty());
    assert!(f.chain.sent().is_empty());
}
