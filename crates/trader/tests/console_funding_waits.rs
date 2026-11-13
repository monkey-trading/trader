//! The console's funding jobs while they wait: a job that only waits for the node leaves its place to the
//! others, and a transaction that outlives the receipt wait is followed to its receipt.

mod common;

use std::time::Duration;

use serde_json::{Value, json};

use common::address;
use common::console::within;
use common::lifecycle::funding::{Funded, Waits, job_id, start_waiting};
use common::lifecycle::{usdc, wallet};

/// A job in the funding read.
async fn job(f: &Funded, id: u64) -> Value {
    let jobs = f.funding().await["jobs"].clone();
    jobs.as_array().unwrap().iter().find(|j| j["id"] == id).expect("the job is kept").clone()
}

/// Four withdrawals that wait in the node's queue hold no place, so a claim for another account starts and
/// lands at once, though the jobs' four places are more than taken.
#[tokio::test]
async fn jobs_that_only_wait_for_the_node_leave_their_places_to_the_others() {
    let f = start_waiting(false, &[0, 1, 2, 3, 4], Waits { receipt: "5s", settle: "10m" }).await;
    let waiting: Vec<String> = (0..4).map(|n| address(n).to_string()).collect();
    f.node.with(|i| {
        // The node's replies go to the requests in whatever order they arrive, so every one names seq 12.
        for account in &waiting {
            let seq = 12;
            i.replies.push_back(json!([
                { "type": "withdrawal_queued", "account": account, "amount": "50", "requested_seq": seq }
            ]));
            let queued = json!({
                "position": 1, "account": account, "amount": "50", "remaining": "50", "requested_seq": seq
            });
            i.lifecycle.answers.insert(
                format!("/v1/accounts/{account}/withdrawals"),
                (200, json!({ "seq": 20, "queue": [queued], "next": null })),
            );
        }
    });
    let started = f.act("withdraw", json!({ "accounts": waiting, "amount": "50" })).await;
    assert_eq!(started.status, 202, "{started:?}");
    // Each has asked the node and now polls the queue for its payout.
    within(async {
        while f.node.with(|i| i.posts.iter().filter(|(path, _)| path == "/v1/withdrawals").count()) < 4 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    let claim = f.act("claim", json!({ "accounts": [address(4).to_string()] })).await;
    let claimed = f.ended(job_id(&claim)).await;
    assert_eq!(claimed["state"], "landed", "{claimed}");
    for id in started.body["jobs"].as_array().unwrap().iter().map(|j| j["id"].as_u64().unwrap()) {
        let withdrawal = job(&f, id).await;
        assert_eq!(withdrawal["state"], "running", "still waiting for its payout: {withdrawal}");
    }
    let _ = f.running.stop_after(&f.node).await;
}

/// A deposit's transaction that stays pending past the receipt wait fails its job and shows as sent; once it
/// is mined the job shows it mined, and nothing was sent again.
#[tokio::test]
async fn a_transaction_past_the_receipt_wait_is_followed_to_its_receipt() {
    let f = start_waiting(false, &[0, 1], Waits { receipt: "1s", settle: "3s" }).await;
    let first = address(0).to_string();
    f.chain.with(|l| {
        l.usdc.insert(wallet(), usdc(500));
        l.hold = true;
    });
    let started = f.act("deposit", json!({ "accounts": [first], "amount": "100", "from": "account" })).await;
    let id = job_id(&started);
    let ended = f.ended(id).await;
    assert_eq!(ended["state"], "failed", "{ended}");
    assert!(ended["detail"].as_str().unwrap().contains("no receipt"), "{ended}");
    assert_eq!(ended["txs"][0]["state"], "sent", "{ended}");
    f.chain.release();
    let mined = within(async {
        loop {
            let now = job(&f, id).await;
            if now["txs"][0]["state"] != "sent" {
                return now;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert_eq!(mined["txs"][0]["state"], "mined", "{mined}");
    assert_eq!(mined["state"], "failed", "the job stays as it ended");
    assert_eq!(f.chain.sent().len(), 1, "nothing was sent again");
}
