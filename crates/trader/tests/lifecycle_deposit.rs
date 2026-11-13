//! Deposits against the stand-in node and chain: the payer approves the vault and deposits into the
//! account, and the bot waits for the node's credit.

mod common;

use alloy_primitives::{B256, U256};
use alloy_sol_types::{SolCall, SolError};
use serde_json::json;
use trader::accounts::deposit::{Deposited, Source};
use trader::chain::contracts::{ERC20, Vault};

use common::lifecycle::chain::{USDC, VAULT};
use common::lifecycle::{Setup, account, deposit_record, funding_wallet, until, usdc, wallet, within};

/// From the account's own wallet: an approval of the vault for the amount, the vault's `deposit`,
/// both sent by the account, and the credit the node records for that transaction.
#[tokio::test]
async fn a_deposit_from_the_accounts_wallet_is_approved_deposited_and_credited() {
    let s = Setup::start(false).await;
    s.chain.with(|l| l.usdc.insert(wallet(), usdc(500)));
    s.answer(&history_path(), 200, json!({ "events": [], "next": null }));
    let (deposited, tx) =
        tokio::join!(s.lifecycle.deposit(&s.key, "100".parse().unwrap(), Source::Account), credit(&s, 2, None));
    assert_eq!(deposited.unwrap(), Deposited::Credited { tx });
    let sent = s.chain.sent();
    assert_eq!(sent.len(), 2);
    assert_eq!((sent[0].from, sent[0].to), (wallet(), USDC));
    assert_eq!(
        ERC20::approveCall::abi_decode(&sent[0].input).unwrap(),
        ERC20::approveCall { spender: VAULT, value: usdc(100) }
    );
    assert_eq!((sent[1].from, sent[1].to), (wallet(), VAULT));
    assert_eq!(Vault::depositCall::abi_decode(&sent[1].input).unwrap(), Vault::depositCall { amount: usdc(100) });
    assert_eq!(s.chain.with(|l| (l.usdc[&wallet()], l.usdc[&VAULT])), (usdc(400), usdc(100)));
}

/// From the funding key, the funding key approves and deposits for the account with `depositFor`;
/// the account sends nothing. Without a funding key it is an error.
#[tokio::test]
async fn a_funding_key_deposits_for_the_account() {
    let s = Setup::start(true).await;
    s.chain.with(|l| l.usdc.insert(funding_wallet(), usdc(1_000)));
    s.answer(&history_path(), 200, json!({ "events": [], "next": null }));
    let (deposited, tx) =
        tokio::join!(s.lifecycle.deposit(&s.key, "250.5".parse().unwrap(), Source::Funding), credit(&s, 2, None));
    assert_eq!(deposited.unwrap(), Deposited::Credited { tx });
    let sent = s.chain.sent();
    assert!(sent.iter().all(|sent| sent.from == funding_wallet()), "the funding key pays");
    let call = Vault::depositForCall::abi_decode(&sent[1].input).unwrap();
    assert_eq!(call, Vault::depositForCall { account: wallet(), amount: U256::from(250_500_000) });
    let none = Setup::start(false).await;
    assert!(none.lifecycle.deposit(&none.key, "1".parse().unwrap(), Source::Funding).await.is_err());
    assert!(none.chain.sent().is_empty());
}

/// With a funding key configured, a deposit from the account's own wallet is still the account's.
#[tokio::test]
async fn a_funding_key_does_not_pay_an_accounts_own_deposit() {
    let s = Setup::start(true).await;
    s.chain.with(|l| l.usdc.insert(wallet(), usdc(500)));
    s.answer(&history_path(), 200, json!({ "events": [], "next": null }));
    let (deposited, _) =
        tokio::join!(s.lifecycle.deposit(&s.key, "100".parse().unwrap(), Source::Account), credit(&s, 2, None));
    deposited.unwrap();
    assert!(s.chain.sent().iter().all(|sent| sent.from == wallet()));
}

/// An allowance that covers the amount needs no approval.
#[tokio::test]
async fn an_allowance_that_covers_the_amount_is_not_approved_again() {
    let s = Setup::start(false).await;
    s.chain.with(|l| {
        l.usdc.insert(wallet(), usdc(500));
        l.allowance.insert((wallet(), VAULT), usdc(100));
    });
    s.answer(&history_path(), 200, json!({ "events": [], "next": null }));
    let (deposited, _) =
        tokio::join!(s.lifecycle.deposit(&s.key, "100".parse().unwrap(), Source::Account), credit(&s, 1, None));
    deposited.unwrap();
    let sent = s.chain.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(&sent[0].input[..4], Vault::depositCall::SELECTOR);
}

/// A deposit the node holds until the account declares ends there, and says so.
#[tokio::test]
async fn a_deposit_awaiting_the_declaration_is_reported() {
    let s = Setup::start(false).await;
    s.chain.with(|l| l.usdc.insert(wallet(), usdc(500)));
    s.answer(&history_path(), 200, json!({ "events": [], "next": null }));
    let pending = Some("eligibility_required");
    let (deposited, tx) =
        tokio::join!(s.lifecycle.deposit(&s.key, "100".parse().unwrap(), Source::Account), credit(&s, 2, pending));
    assert_eq!(deposited.unwrap(), Deposited::AwaitingDeclaration { tx });
}

/// A deposit the node holds for good is an error.
#[tokio::test]
async fn a_held_deposit_is_an_error() {
    let s = Setup::start(false).await;
    s.chain.with(|l| l.usdc.insert(wallet(), usdc(500)));
    s.answer(&history_path(), 200, json!({ "events": [], "next": null }));
    let held = Some("account_sanctioned");
    let (deposited, _) =
        tokio::join!(s.lifecycle.deposit(&s.key, "100".parse().unwrap(), Source::Account), credit(&s, 2, held));
    let err = deposited.unwrap_err();
    assert!(format!("{err:#}").contains("AccountSanctioned"), "{err:#}");
}

/// A history read that fails is read again, and the credit counts once it is read; a deposit never
/// credited within the wait is an error that says it may credit later.
#[tokio::test]
async fn the_credit_is_read_through_failures_until_the_deadline() {
    let s = Setup::start(false).await;
    s.chain.with(|l| l.usdc.insert(wallet(), usdc(500)));
    s.answer(&history_path(), 500, json!({ "error": "internal error" }));
    let recover = async {
        let sent = s.chain.sent_count(2).await;
        until("failed history reads", || s.node.with(|i| i.lifecycle.reads.get(&history_path()).copied()) >= Some(3))
            .await;
        s.answer(&history_path(), 200, deposit_record(sent.hash, "100", None));
        sent.hash
    };
    let (deposited, tx) =
        tokio::join!(within(s.lifecycle.deposit(&s.key, "100".parse().unwrap(), Source::Account)), recover);
    assert_eq!(deposited.unwrap(), Deposited::Credited { tx });

    let s = Setup::start(false).await;
    s.chain.with(|l| l.usdc.insert(wallet(), usdc(500)));
    s.answer(&history_path(), 200, json!({ "events": [], "next": null }));
    let err = within(s.lifecycle.deposit(&s.key, "100".parse().unwrap(), Source::Account)).await.unwrap_err();
    let err = format!("{err:#}");
    assert!(err.contains("may credit later") && err.contains("did not credit deposit"), "{err}");
}

/// A deposit that landed is not sent again while no endpoint holds its receipt: once its nonce is
/// mined, the vault's record of it, and its event, show it landed, and its credit is waited for.
#[tokio::test]
async fn a_landed_deposit_is_not_sent_again_while_its_receipt_lags() {
    let s = Setup::start(false).await;
    s.chain.with(|l| {
        l.usdc.insert(wallet(), usdc(500));
        l.allowance.insert((wallet(), VAULT), usdc(100));
        l.hide_receipts = true;
    });
    s.answer(&history_path(), 200, json!({ "events": [], "next": null }));
    let deposit = within(s.lifecycle.deposit(&s.key, "100".parse().unwrap(), Source::Account));
    let (deposited, tx) = tokio::join!(deposit, credit(&s, 1, None));
    assert_eq!(deposited.unwrap(), Deposited::Credited { tx });
    assert_eq!(s.chain.sent().len(), 1, "sent once");
    assert_eq!(s.chain.with(|l| l.usdc[&VAULT]), usdc(100));
}

/// A deposit whose nonce another transaction took, as when the block that held it is undone, with
/// nothing of it in the vault, is signed again once, and goes in once.
#[tokio::test]
async fn a_replaced_deposit_is_sent_again_and_goes_in_once() {
    let s = Setup::start(false).await;
    s.chain.with(|l| {
        l.usdc.insert(wallet(), usdc(500));
        l.allowance.insert((wallet(), VAULT), usdc(100));
        l.replace = true;
    });
    s.answer(&history_path(), 200, json!({ "events": [], "next": null }));
    let deposit = within(s.lifecycle.deposit(&s.key, "100".parse().unwrap(), Source::Account));
    let (deposited, tx) = tokio::join!(deposit, credit(&s, 1, None));
    assert_eq!(deposited.unwrap(), Deposited::Credited { tx });
    let sent = s.chain.sent();
    assert_eq!((sent.len(), sent[0].nonce), (1, 1), "signed again on the next nonce");
    assert_eq!(s.chain.with(|l| (l.usdc[&VAULT], l.deposits.len())), (usdc(100), 1));
}

/// While the vault cannot say whose a deposit is, one whose receipt no endpoint holds is followed,
/// never sent again: it ends saying it may still be mined.
#[tokio::test]
async fn a_deposit_is_not_sent_again_while_the_vault_cannot_tell() {
    let s = Setup::start(false).await;
    s.chain.with(|l| {
        l.usdc.insert(wallet(), usdc(500));
        l.allowance.insert((wallet(), VAULT), usdc(100));
        l.hide_receipts = true;
        l.limited.insert("eth_getLogs".into());
    });
    let err = within(s.lifecycle.deposit(&s.key, "100".parse().unwrap(), Source::Account)).await.unwrap_err();
    assert!(format!("{err:#}").contains("may still be mined"), "{err:#}");
    assert_eq!(s.chain.sent().len(), 1);
}

/// Nothing is sent for an amount finer than a USDC base unit, or one the payer does not hold.
#[tokio::test]
async fn amounts_that_cannot_be_deposited_send_nothing() {
    let s = Setup::start(false).await;
    s.chain.with(|l| l.usdc.insert(wallet(), usdc(50)));
    for amount in ["0.0000001", "0", "-1", "51"] {
        assert!(s.lifecycle.deposit(&s.key, amount.parse().unwrap(), Source::Account).await.is_err(), "{amount}");
    }
    assert!(s.chain.sent().is_empty());
}

/// A deposit the vault would refuse is not sent, and the vault's reason is reported.
#[tokio::test]
async fn a_deposit_that_would_revert_is_not_sent() {
    let s = Setup::start(false).await;
    s.chain.with(|l| {
        l.usdc.insert(wallet(), usdc(500));
        l.reverts.insert(Vault::depositCall::SELECTOR, Vault::DepositsPaused {}.abi_encode());
    });
    let err = s.lifecycle.deposit(&s.key, "100".parse().unwrap(), Source::Account).await.unwrap_err();
    assert!(format!("{err:#}").contains("would revert: DepositsPaused"), "{err:#}");
    let sent = s.chain.sent();
    assert_eq!(sent.len(), 1, "only the approval went out");
    assert_eq!(&sent[0].input[..4], ERC20::approveCall::SELECTOR);
}

/// A deposit that reverts once mined is reported with the reason its replay gives.
#[tokio::test]
async fn a_deposit_that_reverts_reports_the_reason() {
    let s = Setup::start(false).await;
    s.chain.with(|l| {
        l.usdc.insert(wallet(), usdc(500));
        l.allowance.insert((wallet(), VAULT), usdc(100));
        l.revert_on_chain = Some(Vault::CheckpointsStale {}.abi_encode());
    });
    let err = s.lifecycle.deposit(&s.key, "100".parse().unwrap(), Source::Account).await.unwrap_err();
    let err = format!("{err:#}");
    assert!(err.contains("reverted in 0x") && err.contains("CheckpointsStale"), "{err}");
}

// ── Helpers ──

fn history_path() -> String {
    format!("/v1/accounts/{}/history", account())
}

/// Once the bot has sent its `count`th transaction, has the node record it: credited, or
/// uncredited for `uncredited`. The transaction's hash.
async fn credit(s: &Setup, count: usize, uncredited: Option<&str>) -> B256 {
    let sent = s.chain.sent_count(count).await;
    s.answer(&history_path(), 200, deposit_record(sent.hash, "100", uncredited));
    sent.hash
}
