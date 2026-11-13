//! The console's funding jobs against the stand-in node and chain: each action runs for the account it
//! names, signed or paid by that account's own key or the funding key, as a job the read follows to its
//! end with the transactions it sent; jobs the funding key pays take turns.

mod common;

use std::time::Duration;

use alloy_sol_types::SolCall;
use engine_core::types::AccountId;
use serde_json::json;

use common::address;
use common::console::within;
use common::lifecycle::chain::VAULT as VAULT_ADDRESS;
use common::lifecycle::funding::{job_id, start, start_with};
use common::lifecycle::{deposit_record, funding_wallet, usdc, wallet};
use trader::chain::contracts::{ERC20, Vault};

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
    // The transactions it sent stay on the failed job, each with where it stands.
    let txs = job["txs"].as_array().unwrap();
    assert_eq!(txs.len(), 2, "{job}");
    assert!(txs.iter().all(|tx| tx["state"] == "mined"), "{job}");
    assert_eq!(txs[0]["what"], "approving 100 USDC");
    assert!(f.chain.sent().iter().all(|sent| sent.from == wallet()), "the account's own wallet sent them");
    let free = f.act("claim", json!({ "accounts": [first] })).await;
    let claimed = f.ended(job_id(&free)).await;
    assert_eq!((claimed["state"].clone(), claimed["detail"].clone()), (json!("landed"), json!("no MONKEY to claim")));
}

/// A withdrawal with nothing queued or approved pays nothing, and says so.
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
    let txs = job["txs"].as_array().unwrap();
    assert_eq!(
        txs.iter().map(|tx| tx["what"].clone()).collect::<Vec<_>>(),
        [json!("approving 100 USDC"), json!("depositing 100 USDC")]
    );
    assert_eq!(txs[1]["hash"], json!(sent.hash.to_string()));
    assert!(txs.iter().all(|tx| tx["state"] == "mined"));
    assert!(f.chain.sent().iter().all(|s| s.from == common::lifecycle::funding_wallet()), "the funding key pays");
}

/// Jobs the funding key pays take turns: while a deposit's approval waits to be mined, a claim the
/// funding key pays and a second deposit send nothing, and all land, each deposit with its own approval,
/// so neither deposit spends the allowance the other set.
#[tokio::test]
async fn jobs_the_funding_key_pays_take_turns() {
    let f = start_with(true, &[0, 1, 2]).await;
    let (claimer, depositors) = (address(0), [address(1).to_string(), address(2).to_string()]);
    f.node.with(|i| {
        let answer = common::emissions_answer(&claimer.to_string(), "5000000000000000000", "5000000000000000000");
        i.emissions.insert(claimer.to_string().to_lowercase(), answer);
        // No deposit is credited until it is sent.
        for depositor in &depositors {
            let empty = (200, json!({ "events": [], "next": null }));
            i.lifecycle.answers.insert(format!("/v1/accounts/{depositor}/history"), empty);
        }
    });
    f.chain.with(|l| {
        l.usdc.insert(funding_wallet(), usdc(1_000));
        l.hold = true;
    });
    let held = || f.chain.with(|l| l.held.len());
    let deposits = f.act("deposit", json!({ "accounts": depositors, "amount": "100", "from": "funding" })).await;
    assert_eq!(deposits.status, 202, "{deposits:?}");
    within(async {
        while held() == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    let claim = f.act("claim", json!({ "accounts": [claimer.to_string()] })).await;
    assert_eq!(claim.status, 202, "{claim:?}");
    // Time for the other jobs to send, were they not waiting their turn.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(held(), 1, "one transaction of the funding key at a time");
    f.chain.release();
    // Each deposit is credited once it is sent.
    within(async {
        loop {
            let sent: Vec<_> = f
                .chain
                .sent()
                .into_iter()
                .filter(|s| s.input.starts_with(&Vault::depositForCall::SELECTOR[..]))
                .collect();
            for tx in &sent {
                let account = AccountId(Vault::depositForCall::abi_decode(&tx.input).unwrap().account.0.0);
                let mut record = deposit_record(tx.hash, "100", None);
                record["events"][0]["receipt"]["beneficiary"]["account"] = json!(account);
                f.node.with(|i| i.lifecycle.answers.insert(format!("/v1/accounts/{account}/history"), (200, record)));
            }
            if sent.len() == 2 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    let jobs = deposits.body["jobs"].as_array().unwrap().iter().chain(claim.body["jobs"].as_array().unwrap());
    for id in jobs.map(|j| j["id"].as_u64().unwrap()).collect::<Vec<_>>() {
        let job = f.ended(id).await;
        assert_eq!(job["state"], "landed", "{job}");
    }
    let sent = f.chain.sent();
    assert_eq!(sent.len(), 5, "an approval and a deposit for each depositor, and the claim");
    assert!(sent.iter().all(|s| s.from == funding_wallet()));
    assert_eq!(f.chain.with(|l| l.deposits.clone()), [(funding_wallet(), usdc(100)), (funding_wallet(), usdc(100))]);
    assert_eq!(
        f.chain.with(|l| l.allowance.get(&(funding_wallet(), VAULT_ADDRESS)).copied().unwrap_or_default()),
        usdc(0)
    );
}

/// A withdrawal the queue does not pay in time fails saying it is queued, and that a withdrawal with no
/// amount pays it while asking again queues another.
#[tokio::test]
async fn a_withdrawal_left_in_the_queue_says_how_to_pay_it() {
    let f = start(false).await;
    let first = address(0);
    f.node.with(|i| {
        i.replies
            .push_back(json!([{ "type": "withdrawal_queued", "account": first, "amount": "50", "requested_seq": 12 }]));
        let queued = json!({ "position": 1, "account": first, "amount": "50", "remaining": "50", "requested_seq": 12 });
        i.lifecycle.answers.insert(
            format!("/v1/accounts/{first}/withdrawals"),
            (200, json!({ "seq": 20, "queue": [queued], "next": null })),
        );
    });
    let started = f.act("withdraw", json!({ "accounts": [first.to_string()], "amount": "50" })).await;
    let job = f.ended(job_id(&started)).await;
    assert_eq!(job["state"], "failed", "{job}");
    let detail = job["detail"].as_str().unwrap();
    assert!(
        detail.starts_with(
            "the withdrawal is queued as seq 12 but was not paid in time: once the queue approves it, a withdrawal \
             with no amount pays it, and asking again with an amount queues another"
        ),
        "{detail}"
    );
    assert_eq!(f.node.with(|i| i.posts.iter().filter(|(path, _)| path == "/v1/withdrawals").count()), 1);
}

/// A transaction whose nonce another transaction took shows as replaced, and the one signed again in its
/// place as mined.
#[tokio::test]
async fn a_replaced_transaction_shows_as_replaced() {
    let f = start(false).await;
    let first = address(0).to_string();
    f.chain.with(|l| {
        l.usdc.insert(wallet(), usdc(500));
        l.replace = true;
    });
    // The node never credits it, so the job fails once its transactions are mined.
    let empty = (200, json!({ "events": [], "next": null }));
    f.node.with(|i| i.lifecycle.answers.insert(format!("/v1/accounts/{first}/history"), empty));
    let started = f.act("deposit", json!({ "accounts": [first], "amount": "100", "from": "account" })).await;
    let job = f.ended(job_id(&started)).await;
    let txs: Vec<_> =
        job["txs"].as_array().unwrap().iter().map(|tx| (tx["what"].clone(), tx["state"].clone())).collect();
    let approving = json!("approving 100 USDC");
    assert_eq!(
        txs,
        [
            (approving.clone(), json!("replaced")),
            (approving, json!("mined")),
            (json!("depositing 100 USDC"), json!("mined"))
        ],
        "{job}"
    );
}

/// When the bot stops or halts, its funding jobs end at once and send nothing more, though a transaction
/// they sent may still land.
#[tokio::test]
async fn a_stop_or_a_halt_ends_the_funding_jobs() {
    for halts in [false, true] {
        let f = start(true).await;
        f.chain.with(|l| {
            l.usdc.insert(funding_wallet(), usdc(1_000));
            l.hold = true;
        });
        let second = address(1).to_string();
        let started = f.act("deposit", json!({ "accounts": [second], "amount": "100", "from": "funding" })).await;
        assert_eq!(started.status, 202, "{started:?}");
        within(async {
            while f.chain.with(|l| l.held.len()) == 0 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        let ended = if halts {
            // Only the console reads the trading account's entitlement.
            f.node.with(|i| i.other_signer_path = Some(format!("/v1/accounts/{}/emissions", address(0))));
            tokio::time::timeout(Duration::from_secs(30), f.running.task).await.expect("the bot halts").unwrap()
        } else {
            // The log moves on, so the stop need not wait for orders an earlier run may have signed.
            f.running.stop_after(&f.node).await
        };
        assert!(ended.is_err(), "halts {halts}: the bot ends with an account that could not start, or halts");
        // The approval lands; the job, ended, sends no deposit after it.
        f.chain.release();
        tokio::time::sleep(Duration::from_secs(1)).await;
        let sent = f.chain.sent();
        assert_eq!(sent.len(), 1, "halts {halts}: only the approval was sent: {sent:?}");
        assert!(sent[0].input.starts_with(&ERC20::approveCall::SELECTOR[..]));
    }
}
