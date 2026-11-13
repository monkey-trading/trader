//! The console's funding against the stand-in node and chain: it reads every account that has a key,
//! one that did not start included, even when none did, takes a request only for the bot's own
//! accounts and with no destination, and halts the bot on an answer another key signed.

mod common;

use std::time::Duration;

use alloy_primitives::Address;
use engine_core::access::{EligibilityClass, EligibilityRequest};
use engine_core::eip712::{self, eligibility_hash, recover};
use engine_core::types::AccountId;
use hyper::Method;
use serde_json::{Value, json};

use common::console::{Running, seed, within};
use common::lifecycle::funding::{CONSOLE, job_id, start, start_with};
use common::lifecycle::{usdc, wallet};
use common::{CHAIN_ID, StandIn, VAULT, address, config_many};

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

/// A bot with no account the node has credited still serves the console, which lists each account
/// and declares it, so a fresh installation is funded from the console alone.
#[tokio::test]
async fn a_bot_with_no_funded_account_serves_the_console() {
    let f = start_with(true, &[1]).await;
    let second = address(1);
    let body = f.funding().await;
    assert_eq!(body["accounts"][0]["address"], json!(second.to_string()));
    assert_eq!(body["accounts"][0]["balance"], Value::Null);
    f.node.with(|i| {
        i.replies.push_back(json!([{ "type": "eligibility_attested", "account": second, "class": "professional" }]));
    });
    let statement = EligibilityClass::Professional.statement();
    let started = f
        .act("declare", json!({ "class": "professional", "statement": statement, "accounts": [second.to_string()] }))
        .await;
    assert_eq!(f.ended(job_id(&started)).await["state"], "landed");
}

/// A request naming as many accounts as the console allows is read whole, one naming more is refused
/// for its count, and the accounts' figures are read again after a job changed them.
#[tokio::test]
async fn a_large_selection_is_accepted_and_a_jobs_figures_are_read_again() {
    let f = start(false).await;
    let strangers: Vec<String> =
        (1..=501u16).map(|n| AccountId(Address::left_padding_from(&n.to_be_bytes()).0.0).to_string()).collect();
    assert!(
        serde_json::to_vec(&json!({ "accounts": strangers })).unwrap().len() > 4096,
        "past every other route's limit"
    );
    let most = f.act("claim", json!({ "accounts": strangers[..500] })).await;
    assert_eq!((most.status, most.body["code"].clone()), (404, json!("no_account")), "{most:?}");
    let more = f.act("claim", json!({ "accounts": strangers })).await;
    assert_eq!((more.status, more.body["code"].clone()), (422, json!("bad_accounts")), "{more:?}");
    let first = address(0).to_string();
    f.chain.with(|l| {
        l.usdc.insert(wallet(), usdc(7));
    });
    assert_eq!(f.funding().await["accounts"][0]["wallet_usdc"], "7");
    f.chain.with(|l| {
        l.usdc.insert(wallet(), usdc(9));
    });
    let started = f.act("claim", json!({ "accounts": [first] })).await;
    f.ended(job_id(&started)).await;
    within(async {
        while f.funding().await["accounts"][0]["wallet_usdc"] != "9" {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
}

/// An answer another key signed, to a funding read or to a job's request, halts the bot as one the
/// runner reads does, and nothing is sent.
#[tokio::test]
async fn an_answer_another_key_signed_to_the_funding_halts_the_bot() {
    // The status read alone reads eligibility here, and a declaration alone the nonce.
    for (path, declares) in [("eligibility", false), ("nonce", true)] {
        let f = start_with(true, &[1]).await;
        let second = address(1);
        f.node.with(|i| i.other_signer_path = Some(format!("/v1/accounts/{second}/{path}")));
        if declares {
            let statement = EligibilityClass::Professional.statement();
            let body = json!({ "class": "professional", "statement": statement, "accounts": [second.to_string()] });
            assert_eq!(f.act("declare", body).await.status, 202);
        } else {
            assert_eq!(f.running.get("/api/funding", &f.cookie).await.status, 200);
        }
        let ended = tokio::time::timeout(Duration::from_secs(30), f.running.task).await.expect("the bot stops");
        let error = ended.unwrap().expect_err("the bot halts");
        assert_eq!(error.to_string(), "the node's answers are not signed by the configured signer", "{path}");
        assert!(f.node.with(|i| i.posts.is_empty()), "{path}: nothing was sent to the node");
        assert!(f.chain.sent().is_empty(), "{path}");
    }
}
