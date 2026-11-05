//! Withdrawals against the stand-in node and chain: a signed request, the queue's approval, each
//! payout's authorization paid to the account, and an expired one replaced.

mod common;

use std::time::Duration;

use alloy_primitives::{Address, B256, Bytes};
use alloy_sol_types::{SolCall, SolError};
use engine_core::command::WithdrawalRequest;
use engine_core::eip712::{self, Signature, recover, withdrawal_hash};
use engine_core::types::AccountId;
use serde_json::{Value, json};
use trader::accounts::withdraw::{Paid, reissue_hash};
use trader::chain::contracts::Vault;

use common::chain::VAULT;
use common::lifecycle::{Setup, account, funding_wallet, until, usdc, wallet};
use common::{CHAIN_ID, VAULT as NODE_VAULT};

/// The stand-in node's time, in seconds.
const NOW_S: u64 = 1_800_000_000;

/// The request is the account's signature over the amount; once the queue approves it, the payout's
/// authorization is sent to the vault exactly as signed, by the funding key, and pays the account.
#[tokio::test]
async fn a_withdrawal_is_requested_approved_and_paid_to_the_account() {
    let s = Setup::start(true).await;
    s.node.with(|i| {
        i.nonce = 3;
        i.replies.push_back(
            json!([{ "type": "withdrawal_queued", "account": account(), "amount": "50", "requested_seq": 12 }]),
        );
    });
    s.answer(&queue_path(), 200, queue(&[12]));
    s.answer(&payouts_path(), 200, payouts(vec![]));
    let approve = async {
        wait_reads(&s, &queue_path(), 2).await;
        s.answer(&queue_path(), 200, queue(&[]));
        s.answer(&payouts_path(), 200, payouts(vec![payout(5, "50", Some(NOW_S + 86_400))]));
    };
    let (paid, ()) = tokio::join!(s.lifecycle.withdraw(&s.key, Some("50".parse().unwrap())), approve);
    let sent = s.chain.sent();
    assert_eq!(paid.unwrap(), vec![Paid { nonce: 5, amount: "50".parse().unwrap(), tx: sent[0].hash }]);

    let posts = s.posts("/v1/withdrawals");
    assert_eq!(posts.len(), 1);
    let request: WithdrawalRequest = serde_json::from_value(posts[0].clone()).unwrap();
    let auth = request.auth.unwrap();
    let hash = withdrawal_hash(&domain(), request.account, request.amount, auth.nonce, auth.expires_at);
    assert_eq!((request.account, request.amount, auth.nonce), (account(), "50".parse().unwrap(), 3));
    assert_eq!(recover(&hash, &auth.signature), Some(account()));

    assert_eq!((sent.len(), sent[0].from, sent[0].to), (1, funding_wallet(), VAULT));
    let call = Vault::withdrawCall::abi_decode(&sent[0].input).unwrap();
    let expected = Vault::withdrawCall {
        withdrawal: Vault::Withdrawal {
            account: wallet(),
            amount: usdc(50),
            nonce: 5,
            approvedSeq: 30,
            signerEpoch: 2,
            issuedAt: NOW_S,
            expiresAt: NOW_S + 86_400,
        },
        checkpointSeq: 31,
        siblings: vec![B256::repeat_byte(3)],
        signature: Bytes::from(vec![0xab; 65]),
    };
    assert_eq!(call, expected);
    assert_eq!(
        s.chain.with(|l| (l.usdc.get(&wallet()).copied(), l.usdc.get(&funding_wallet()).copied())),
        (Some(usdc(50)), None)
    );
}

/// An authorization that expired unpaid is replaced on the account's signed request, and the new
/// one is paid.
#[tokio::test]
async fn an_expired_authorization_is_reissued_and_paid() {
    let s = Setup::start(false).await;
    let expired = NOW_S - 3_600;
    s.answer(&payouts_path(), 200, payouts(vec![payout(5, "50", Some(expired))]));
    let path = format!("/v1/accounts/{}/withdrawals/5/reissue", account());
    let reissue = async {
        until("the reissue", || !s.posts(&path).is_empty()).await;
        s.answer(&payouts_path(), 200, payouts(vec![payout(5, "50", Some(NOW_S + 86_400))]));
    };
    let (paid, ()) = tokio::join!(s.lifecycle.withdraw(&s.key, None), reissue);
    assert_eq!(paid.unwrap().len(), 1);
    let posts = s.posts(&path);
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0]["replaces_expires_at"], json!(expired));
    let signature: Signature = serde_json::from_value(posts[0]["signature"].clone()).unwrap();
    assert_eq!(recover(&reissue_hash(&domain(), account(), 5, expired), &signature), Some(account()));
    let sent = s.chain.sent();
    assert_eq!((sent.len(), sent[0].from), (1, wallet()));
    assert_eq!(Vault::withdrawCall::abi_decode(&sent[0].input).unwrap().withdrawal.expiresAt, NOW_S + 86_400);
}

/// A payout the vault already paid, by whoever sent it, is not sent again; with nothing to pay and
/// no amount, nothing is requested.
#[tokio::test]
async fn paid_payouts_and_no_payouts_send_nothing() {
    let s = Setup::start(false).await;
    s.answer(&payouts_path(), 200, payouts(vec![]));
    assert_eq!(s.lifecycle.withdraw(&s.key, None).await.unwrap(), vec![]);
    s.answer(&payouts_path(), 200, payouts(vec![payout(5, "50", Some(NOW_S + 86_400))]));
    s.chain.with(|l| l.used_nonces.insert((wallet(), 5)));
    assert_eq!(s.lifecycle.withdraw(&s.key, None).await.unwrap(), vec![]);
    assert!(s.chain.sent().is_empty());
    assert!(s.posts("/v1/withdrawals").is_empty());
}

/// An authorization that pays another account, or is for another vault, is refused and nothing is
/// sent.
#[tokio::test]
async fn an_authorization_for_another_account_or_vault_is_refused() {
    for (field, value) in
        [("account", json!(format!("0x{}", "07".repeat(20)))), ("vault", json!(Address::repeat_byte(7)))]
    {
        let s = Setup::start(true).await;
        let mut other = payout(5, "50", Some(NOW_S + 86_400));
        other["withdrawal"][field] = value;
        s.answer(&payouts_path(), 200, payouts(vec![other]));
        let err = s.lifecycle.withdraw(&s.key, None).await.unwrap_err();
        assert!(err.to_string().contains("authorization"), "{field}: {err}");
        assert!(s.chain.sent().is_empty(), "{field}");
    }
}

/// A refusal in the log, and a request whose answer never came, end the withdrawal without asking
/// again.
#[tokio::test]
async fn refused_and_unanswered_requests_are_not_asked_again() {
    let s = Setup::start(false).await;
    let rejected = json!([{ "type": "withdrawal_rejected", "account": account(), "amount": "50",
        "reason": { "kind": "insufficient_funds", "available": "10" } }]);
    s.node.with(|i| i.replies.push_back(rejected));
    let err = s.lifecycle.withdraw(&s.key, Some("50".parse().unwrap())).await.unwrap_err();
    assert!(err.to_string().contains("InsufficientFunds"), "{err}");
    s.node.with(|i| i.post_delay = Some(Duration::from_secs(3)));
    let err = s.lifecycle.withdraw(&s.key, Some("50".parse().unwrap())).await.unwrap_err();
    assert!(format!("{err:#}").contains("may still enter the log"), "{err:#}");
    assert_eq!(s.posts("/v1/withdrawals").len(), 2);
    assert!(s.chain.sent().is_empty());
}

/// A retry-later answer and a stale nonce are followed by the request again, signed on the
/// account's current nonce, and the withdrawal then goes ahead.
#[tokio::test]
async fn retry_later_and_stale_nonce_answers_request_again() {
    let s = Setup::start(false).await;
    s.node.with(|i| {
        i.statuses.extend([429, 409]);
        i.replies.push_back(
            json!([{ "type": "withdrawal_queued", "account": account(), "amount": "50", "requested_seq": 12 }]),
        );
    });
    s.answer(&queue_path(), 200, queue(&[]));
    s.answer(&payouts_path(), 200, payouts(vec![payout(5, "50", Some(NOW_S + 86_400))]));
    assert_eq!(s.lifecycle.withdraw(&s.key, Some("50".parse().unwrap())).await.unwrap().len(), 1);
    let nonces: Vec<u64> = s
        .posts("/v1/withdrawals")
        .iter()
        .map(|post| serde_json::from_value::<WithdrawalRequest>(post.clone()).unwrap().auth.unwrap().nonce)
        .collect();
    assert_eq!(nonces, vec![0, 1, 2], "each request on the nonce the node gave");
}

/// An authorization too close to its expiry to be mined in time is not sent; its replacement is.
#[tokio::test]
async fn an_authorization_near_its_expiry_is_not_sent() {
    let s = Setup::start(false).await;
    s.answer(&payouts_path(), 200, payouts(vec![payout(5, "50", Some(NOW_S + 60))]));
    let replace = async {
        wait_reads(&s, &payouts_path(), 3).await;
        assert!(s.chain.sent().is_empty(), "nothing sent so close to the expiry");
        s.answer(&payouts_path(), 200, payouts(vec![payout(5, "50", Some(NOW_S + 86_400))]));
    };
    let (paid, ()) = tokio::join!(s.lifecycle.withdraw(&s.key, None), replace);
    assert_eq!(paid.unwrap().len(), 1);
    assert_eq!(Vault::withdrawCall::abi_decode(&s.chain.sent()[0].input).unwrap().withdrawal.expiresAt, NOW_S + 86_400);
}

/// An expired authorization still not replaced is asked for again after a while, as when the node
/// lost the request in a restart or did not take it.
#[tokio::test]
async fn an_unreplaced_authorization_is_asked_for_again() {
    let s = Setup::start(false).await;
    let expired = NOW_S - 3_600;
    s.answer(&payouts_path(), 200, payouts(vec![payout(5, "50", Some(expired))]));
    // The first request is not taken; the second is.
    s.node.with(|i| i.statuses.push_back(400));
    let path = format!("/v1/accounts/{}/withdrawals/5/reissue", account());
    let reissue = async {
        until("a second reissue", || s.posts(&path).len() >= 2).await;
        s.answer(&payouts_path(), 200, payouts(vec![payout(5, "50", Some(NOW_S + 86_400))]));
    };
    let (paid, ()) = tokio::join!(s.lifecycle.withdraw(&s.key, None), reissue);
    assert_eq!(paid.unwrap().len(), 1);
    // Thirty polls apart at the stand-in's 20 ms, not every poll.
    assert_eq!(s.posts(&path).len(), 2);
}

/// A payout the vault refuses as paid already, by whoever sent it, is not an error.
#[tokio::test]
async fn a_payout_the_vault_refuses_as_paid_is_left() {
    let s = Setup::start(false).await;
    s.answer(&payouts_path(), 200, payouts(vec![payout(5, "50", Some(NOW_S + 86_400))]));
    s.chain.with(|l| l.reverts.insert(Vault::withdrawCall::SELECTOR, Vault::WithdrawalAlreadyPaid {}.abi_encode()));
    assert_eq!(s.lifecycle.withdraw(&s.key, None).await.unwrap(), vec![]);
    s.chain.with(|l| l.reverts.insert(Vault::withdrawCall::SELECTOR, Vault::WithdrawalsPaused {}.abi_encode()));
    let err = s.lifecycle.withdraw(&s.key, None).await.unwrap_err();
    assert!(format!("{err:#}").contains("WithdrawalsPaused"), "{err:#}");
    assert!(s.chain.sent().is_empty());
}

/// The status reads the account on chain and in the node, and leaves out a payout the vault has
/// paid that the node still lists.
#[tokio::test]
async fn the_status_leaves_out_payouts_the_vault_paid() {
    let s = Setup::start(false).await;
    s.answer(
        &format!("/v1/accounts/{}/eligibility", account()),
        200,
        json!({ "class": "professional", "statement": "I attest." }),
    );
    s.answer(&queue_path(), 200, queue(&[12]));
    let unsigned = payout(6, "20", None);
    s.answer(&payouts_path(), 200, payouts(vec![payout(5, "50", Some(NOW_S + 86_400)), unsigned]));
    s.chain.with(|l| {
        l.used_nonces.insert((wallet(), 5));
        l.usdc.insert(wallet(), usdc(7));
    });
    let status = s.lifecycle.status(account()).await.unwrap();
    assert_eq!(status.unpaid.iter().map(|payout| payout.nonce).collect::<Vec<_>>(), vec![6]);
    assert_eq!(status.queued.iter().map(|queued| queued.requested_seq).collect::<Vec<_>>(), vec![12]);
    assert_eq!((status.wallet_usdc, status.balance), (usdc(7), None));
    assert_eq!(status.declared, Some(engine_core::access::EligibilityClass::Professional));
    assert!(status.payouts_loaded);
    let mut loading = payouts(vec![]);
    loading["loaded"] = json!(false);
    s.answer(&payouts_path(), 200, loading);
    assert!(!s.lifecycle.status(account()).await.unwrap().payouts_loaded);
}

// ── Helpers ──

fn domain() -> eip712::Eip712Domain {
    eip712::domain(CHAIN_ID, AccountId(NODE_VAULT))
}

fn queue_path() -> String {
    format!("/v1/accounts/{}/withdrawals", account())
}

fn payouts_path() -> String {
    format!("/v1/accounts/{}/payouts", account())
}

/// The account's queue page holding requests `seqs`.
fn queue(seqs: &[u64]) -> Value {
    let queue: Vec<Value> = seqs
        .iter()
        .map(|seq| json!({ "position": 1, "account": account(), "amount": "50", "remaining": "50", "requested_seq": seq }))
        .collect();
    json!({ "seq": 20, "queue": queue, "next": null })
}

/// A payouts page.
fn payouts(payouts: Vec<Value>) -> Value {
    json!({ "seq": 40, "loaded": true, "payouts": payouts, "next": null })
}

/// Payout `nonce` of `amount`, authorized until `expires_at` when given.
fn payout(nonce: u64, amount: &str, expires_at: Option<u64>) -> Value {
    let withdrawal = expires_at.map(|expires_at| {
        json!({ "account": account(), "amount": amount, "nonce": nonce, "chain_id": CHAIN_ID,
            "vault": AccountId(NODE_VAULT), "signature": format!("0x{}", "ab".repeat(65)), "approved_seq": 30,
            "authorization": { "signer": AccountId([5; 20]), "signer_epoch": 2, "issued_at": expires_at - 86_400,
                "expires_at": expires_at, "checkpoint_seq": 31, "checkpoint_root": B256::repeat_byte(4),
                "siblings": [B256::repeat_byte(3)] } })
    });
    json!({ "nonce": nonce, "amount": amount, "withdrawal": withdrawal })
}

/// Waits until the bot has read `path` `count` times.
async fn wait_reads(s: &Setup, path: &str, count: usize) {
    until(path, || s.node.with(|i| i.reads.get(path).copied().unwrap_or_default()) >= count).await;
}
