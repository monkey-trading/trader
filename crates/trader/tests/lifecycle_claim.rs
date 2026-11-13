//! MONKEY claims against the stand-in node and chain: the node's proof claimed for the account,
//! paid for by the funding key when there is one, and only once enough is unclaimed.

mod common;

use std::sync::Arc;
use std::time::Duration;

use alloy_primitives::{B256, U256};
use alloy_sol_types::{SolCall, SolError};
use serde_json::{Value, json};
use trader::accounts::claim::{Batching, Claimed, watch, watch_batched};
use trader::chain::contracts::Vault;
use trader::keys::AccountKey;
use trader::metrics::Metrics;

use common::lifecycle::chain::VAULT;
use common::lifecycle::{Setup, account, funding_wallet, until, wallet, within};
use engine_core::types::AccountId;

/// One MONKEY in base units.
const MONKEY: u128 = 1_000_000_000_000_000_000;

/// The proof the node gives is claimed for the account, by the funding key, which receives nothing.
#[tokio::test]
async fn a_claimable_entitlement_is_claimed_for_the_account_by_the_funding_key() {
    let s = Setup::start(true).await;
    s.chain.with(|l| l.monkey_claimed.insert(wallet(), U256::from(MONKEY)));
    s.answer(&emissions_path(), 200, proof(5 * MONKEY, 6 * MONKEY, true));
    let claimed = s.lifecycle.claim(&s.key, U256::from(1)).await.unwrap();
    let sent = s.chain.sent();
    assert_eq!(claimed, Claimed::Minted { amount: U256::from(4 * MONKEY), tx: sent[0].hash });
    assert_eq!((sent.len(), sent[0].from, sent[0].to), (1, funding_wallet(), VAULT));
    let call = Vault::claimEmissionsCall::abi_decode(&sent[0].input).unwrap();
    let expected = Vault::claimEmissionsCall {
        account: wallet(),
        entitled: U256::from(5 * MONKEY),
        checkpointSeq: 40,
        siblings: vec![B256::repeat_byte(1), B256::repeat_byte(2)],
    };
    assert_eq!(call, expected);
    assert_eq!(
        s.chain.with(|l| (l.monkey.get(&wallet()).copied(), l.monkey.get(&funding_wallet()).copied())),
        (Some(U256::from(4 * MONKEY)), None)
    );
}

/// Without a funding key the account pays for its own claim.
#[tokio::test]
async fn without_a_funding_key_the_account_claims() {
    let s = Setup::start(false).await;
    s.answer(&emissions_path(), 200, proof(2 * MONKEY, 2 * MONKEY, true));
    assert!(matches!(s.lifecycle.claim(&s.key, U256::from(1)).await.unwrap(), Claimed::Minted { .. }));
    assert_eq!(s.chain.sent()[0].from, wallet());
}

/// Nothing is sent before there is an entitlement, while it waits for the vault's checkpoint, below
/// the threshold, or once everything is claimed.
#[tokio::test]
async fn nothing_is_claimed_when_nothing_is_due() {
    let s = Setup::start(false).await;
    assert_eq!(s.lifecycle.claim(&s.key, U256::from(1)).await.unwrap(), Claimed::Nothing);
    s.answer(&emissions_path(), 200, proof(2 * MONKEY, 3 * MONKEY, false));
    let waiting = s.lifecycle.claim(&s.key, U256::from(1)).await.unwrap();
    assert_eq!(waiting, Claimed::Waiting { unclaimed: U256::from(3 * MONKEY) });
    s.answer(&emissions_path(), 200, proof(2 * MONKEY, 3 * MONKEY, true));
    let below = s.lifecycle.claim(&s.key, U256::from(2 * MONKEY + 1)).await.unwrap();
    assert_eq!(below, Claimed::Below { unclaimed: U256::from(2 * MONKEY) });
    s.chain.with(|l| l.monkey_claimed.insert(wallet(), U256::from(2 * MONKEY)));
    assert_eq!(s.lifecycle.claim(&s.key, U256::ZERO).await.unwrap(), Claimed::Below { unclaimed: U256::ZERO });
    assert!(s.chain.sent().is_empty());
}

/// A claim mined after another sender claimed the entitlement, which the bot read unclaimed, is
/// reported as nothing to claim; another revert fails it.
#[tokio::test]
async fn a_claim_another_sender_made_first_is_nothing_to_claim() {
    let s = Setup::start(false).await;
    s.answer(&emissions_path(), 200, proof(2 * MONKEY, 2 * MONKEY, true));
    let claimed = Vault::NothingToClaim { entitled: U256::from(2 * MONKEY), claimed: U256::from(2 * MONKEY) };
    s.chain.with(|l| l.revert_on_chain = Some(claimed.abi_encode()));
    let below = within(s.lifecycle.claim(&s.key, U256::from(1))).await.unwrap();
    assert_eq!(below, Claimed::Below { unclaimed: U256::ZERO });
    assert_eq!(s.chain.sent().len(), 1, "mined, and reverted");
    s.chain.with(|l| {
        l.reverts.clear();
        l.revert_on_chain = Some(Vault::InvalidEntitlementProof {}.abi_encode());
    });
    let err = within(s.lifecycle.claim(&s.key, U256::from(1))).await.unwrap_err();
    assert!(format!("{err:#}").contains("InvalidEntitlementProof"), "{err:#}");
}

/// While a transaction of the payer's is still pending, as a claim that outlived its receipt wait,
/// no claim is sent beside it; once it is mined or dropped, the claim goes.
#[tokio::test]
async fn no_claim_is_sent_beside_a_pending_transaction() {
    let s = Setup::start(false).await;
    s.answer(&emissions_path(), 200, proof(2 * MONKEY, 2 * MONKEY, true));
    s.chain.with(|l| l.pending = 1);
    let err = within(s.lifecycle.claim(&s.key, U256::from(1))).await.unwrap_err();
    assert!(format!("{err:#}").contains("has a transaction pending"), "{err:#}");
    assert!(s.chain.sent().is_empty());
    s.chain.with(|l| l.pending = 0);
    assert!(matches!(within(s.lifecycle.claim(&s.key, U256::from(1))).await.unwrap(), Claimed::Minted { .. }));
}

/// A proof for another account is refused, and nothing is sent.
#[tokio::test]
async fn a_proof_for_another_account_is_refused() {
    let s = Setup::start(true).await;
    let mut other = proof(2 * MONKEY, 2 * MONKEY, true);
    other["account"] = json!(format!("0x{}", "07".repeat(20)));
    s.answer(&emissions_path(), 200, other);
    assert!(s.lifecycle.claim(&s.key, U256::from(1)).await.is_err());
    assert!(s.chain.sent().is_empty());
}

/// While the bot runs, the watch claims once the unclaimed amount reaches the threshold, and not
/// again once it is claimed.
#[tokio::test]
async fn the_watch_claims_once_the_threshold_is_reached() {
    let s = Setup::start(true).await;
    s.answer(&emissions_path(), 200, proof(MONKEY, MONKEY, true));
    let key = Arc::new(AccountKey::load(&s.cfg.plans[0].key, |_| None).await.unwrap());
    let lifecycle = Arc::new(trader::accounts::Lifecycle::connect(&s.cfg, |_| None).await.unwrap());
    let task = tokio::spawn(watch(lifecycle, vec![key], U256::from(2 * MONKEY), Duration::from_millis(20)));
    let path = emissions_path();
    until("three checks", || s.node.with(|i| i.lifecycle.reads.get(&path).copied().unwrap_or_default()) >= 3).await;
    assert!(s.chain.sent().is_empty(), "below the threshold");
    s.answer(&emissions_path(), 200, proof(3 * MONKEY, 3 * MONKEY, true));
    s.chain.sent_count(1).await;
    let reads = s.node.with(|i| i.lifecycle.reads[&path]);
    until("three more checks", || s.node.with(|i| i.lifecycle.reads[&path]) >= reads + 3).await;
    task.abort();
    assert_eq!(s.chain.sent().len(), 1, "claimed once");
}

/// With `[claims]`, `trader run` claims the account's MONKEY while it trades, through the funding
/// key, and stops as usual.
#[tokio::test]
async fn the_bot_claims_while_it_runs() {
    let s = Setup::start(true).await;
    s.answer(&emissions_path(), 200, proof(3 * MONKEY, 3 * MONKEY, true));
    s.node.with(|i| {
        i.markets = json!([common::btc_usd(Some("100"))]);
        i.accounts.insert(account().to_string(), common::account_view("5", json!({}), json!({})));
    });
    // The log passes the window orders signed before this start could still enter it in.
    s.node.tick(common::T0 + common::MINUTE);
    let claims = "[claims]\nthreshold = \"1\"\ninterval = \"50ms\"\n";
    let cfg = common::lifecycle::config(&s.node, &[s.chain.url()], s.dir.path(), true, claims);
    let chain = s.chain.clone();
    let stop = async move {
        chain.sent_count(1).await;
    };
    within(trader::bot::run(cfg, |_| None, stop, &Metrics::new())).await.unwrap();
    let sent = s.chain.sent();
    assert_eq!((sent[0].from, sent[0].to), (funding_wallet(), VAULT));
    assert_eq!(&sent[0].input[..4], Vault::claimEmissionsCall::SELECTOR);
}

/// An answer to the claims signed by another key halts the bot: it stops at once and leaves the
/// account as it is, cancelling nothing.
#[tokio::test]
async fn a_claims_answer_from_another_signer_halts_the_bot() {
    let s = Setup::start(true).await;
    s.node.with(|i| {
        i.markets = json!([common::btc_usd(Some("100"))]);
        i.accounts.insert(
            account().to_string(),
            common::account_view("5", json!({}), json!({ "9": common::resting_order(9) })),
        );
        i.other_signer_path = Some(emissions_path());
    });
    s.node.tick(common::T0 + common::MINUTE);
    let claims = "[claims]\nthreshold = \"1\"\ninterval = \"50ms\"\n";
    let cfg = common::lifecycle::config(&s.node, &[s.chain.url()], s.dir.path(), true, claims);
    let err = within(trader::bot::run(cfg, |_| None, std::future::pending(), &Metrics::new())).await.unwrap_err();
    assert!(format!("{err:#}").contains("not signed by the attested signer"), "{err:#}");
    assert!(s.node.with(|i| i.posts.is_empty()), "the resting order is left as it is");
    assert!(s.chain.sent().is_empty());
}

/// A failed claim raises its warning once while it lasts, and clears it when a check succeeds.
#[tokio::test]
async fn a_failing_claim_warns_once_and_resolves() {
    let s = Setup::start(true).await;
    s.answer(&emissions_path(), 200, proof(MONKEY, MONKEY, true));
    s.chain.with(|l| l.down = true);
    let lines = common::lifecycle::Lines::default();
    let _logs = tracing::subscriber::set_default(lines.subscriber());
    let key = Arc::new(AccountKey::load(&s.cfg.plans[0].key, |_| None).await.unwrap());
    let lifecycle = Arc::new(trader::accounts::Lifecycle::connect(&s.cfg, |_| None).await.unwrap());
    let task = tokio::spawn(watch(lifecycle, vec![key], U256::from(1), Duration::from_millis(20)));
    let path = emissions_path();
    until("three failed checks", || s.node.with(|i| i.lifecycle.reads.get(&path).copied().unwrap_or_default()) >= 3)
        .await;
    s.chain.with(|l| l.down = false);
    s.chain.sent_count(1).await;
    until("the warning to clear", || lines.text().contains("alert_resolved")).await;
    task.abort();
    let text = lines.text();
    assert_eq!(text.matches("alert=\"monkey_claim_failed\"").count(), 1, "{text}");
    assert_eq!(text.matches("alert_resolved=\"monkey_claim_failed\"").count(), 1, "{text}");
}

/// A claims pass asks its accounts a batch at a time and holds a batch while one of its accounts
/// waits for the vault to record a checkpoint, so the node's watch set of 128 accounts cannot
/// push that account out before it is claimable; the next batch is asked once it is claimed.
#[tokio::test]
async fn a_batch_is_held_while_an_account_waits_for_its_checkpoint() {
    let s = Setup::start(true).await;
    let (first, second) = (emissions_path(), other_emissions_path());
    s.answer(&first, 200, proof(MONKEY, MONKEY, false));
    s.answer(&second, 200, other_proof(MONKEY, MONKEY, true));
    let (lifecycle, keys) = two_keys(&s).await;
    let batching = Batching { size: 1, poll: Duration::from_millis(10), bound: Duration::from_secs(30) };
    let task = tokio::spawn(watch_batched(lifecycle, keys, U256::from(1), Duration::from_millis(20), batching));
    until("five checks of the waiting account", || reads(&s, &first) >= 5).await;
    assert_eq!(reads(&s, &second), 0, "the next batch waits for the first");
    s.answer(&first, 200, proof(MONKEY, MONKEY, true));
    until("both claimed", || s.chain.sent().len() >= 2).await;
    task.abort();
    assert!(reads(&s, &second) >= 1);
}

/// An account still waiting when its batch's bound passes is left for the next pass, which goes on
/// to the next batch, and is warned of once; the warning clears when it is claimed.
#[tokio::test]
async fn an_account_that_keeps_waiting_is_warned_of_once_and_does_not_hold_the_pass() {
    let s = Setup::start(true).await;
    let (first, second) = (emissions_path(), other_emissions_path());
    s.answer(&first, 200, proof(MONKEY, MONKEY, false));
    s.answer(&second, 200, other_proof(MONKEY, MONKEY, true));
    let lines = common::lifecycle::Lines::default();
    let _logs = tracing::subscriber::set_default(lines.subscriber());
    let (lifecycle, keys) = two_keys(&s).await;
    let batching = Batching { size: 1, poll: Duration::from_millis(5), bound: Duration::from_millis(50) };
    let task = tokio::spawn(watch_batched(lifecycle, keys, U256::from(1), Duration::from_millis(20), batching));
    until("the second account's claim", || !s.chain.sent().is_empty()).await;
    until("the first account asked in another pass", || reads(&s, &first) >= 30).await;
    s.answer(&first, 200, proof(MONKEY, MONKEY, true));
    until("the warning to clear", || lines.text().contains("alert_resolved=\"monkey_claim_waiting\"")).await;
    task.abort();
    let text = lines.text();
    assert_eq!(text.matches("alert=\"monkey_claim_waiting\"").count(), 1, "{text}");
}

// ── Helpers ──

fn emissions_path() -> String {
    format!("/v1/accounts/{}/emissions", account())
}

/// The node's proof of an entitlement of `entitled` at checkpoint 40, `newest` at the newest
/// checkpoint, claimable now or not.
fn proof(entitled: u128, newest: u128, claimable: bool) -> Value {
    json!({ "seq": 40, "root": B256::repeat_byte(9), "account": account(), "entitled": entitled.to_string(),
        "siblings": [B256::repeat_byte(1), B256::repeat_byte(2)], "accepted_seq": 40, "newest_seq": 44,
        "newest_entitled": newest.to_string(), "claimable": claimable })
}

fn other() -> AccountId {
    AccountId(funding_wallet().into_array())
}

fn other_emissions_path() -> String {
    format!("/v1/accounts/{}/emissions", other())
}

/// The proof of the funding key's wallet as a second account, which a claim pays for from its own
/// wallet.
fn other_proof(entitled: u128, newest: u128, claimable: bool) -> Value {
    let mut proof = proof(entitled, newest, claimable);
    proof["account"] = json!(other());
    proof
}

/// How many times the bot read `path`.
fn reads(s: &Setup, path: &str) -> usize {
    s.node.with(|i| i.lifecycle.reads.get(path).copied().unwrap_or_default())
}

/// A lifecycle and the keys of the test account and, as a second account, the funding key.
async fn two_keys(s: &Setup) -> (Arc<trader::accounts::Lifecycle>, Vec<Arc<AccountKey>>) {
    let lifecycle = Arc::new(trader::accounts::Lifecycle::connect(&s.cfg, |_| None).await.unwrap());
    let funding = &s.cfg.funding.as_ref().unwrap().key;
    let keys = vec![
        Arc::new(AccountKey::load(&s.cfg.plans[0].key, |_| None).await.unwrap()),
        Arc::new(AccountKey::load(funding, |_| None).await.unwrap()),
    ];
    (lifecycle, keys)
}
