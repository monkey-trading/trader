//! The chain client against stand-in endpoints: reads move past an endpoint that does not answer,
//! every transaction goes to each endpoint, a spent nonce is signed again, and no endpoint's URL is
//! ever printed.

mod common;

use alloy_primitives::U256;
use alloy_sol_types::SolCall;
use trader::chain::contracts::Vault;
use trader::chain::{Call, Chain, TxError};
use trader::config::ChainSection;

use common::chain::{CHAIN_ID, ChainStandIn, USDC, VAULT};
use common::lifecycle::{Setup, config, wallet};

/// A closed loopback port whose path carries a stand-in credential.
const DOWN: &str = "http://127.0.0.1:9/v2/secret-key";

/// With the first endpoint down, reads and transactions go to the next.
#[tokio::test]
async fn an_endpoint_that_does_not_answer_is_passed_over() {
    let s = Setup::start(false).await;
    let chain = Chain::connect(&section(&s, &[DOWN.into(), s.chain.url()]), CHAIN_ID).await.unwrap();
    assert_eq!(chain.view(VAULT, Vault::usdcCall {}).await.unwrap(), USDC);
    chain.send(&s.key, &deposit()).await.unwrap();
    assert_eq!(s.chain.sent().len(), 1);
}

/// A transaction goes to every endpoint, which each take the same signed transaction.
#[tokio::test]
async fn a_transaction_is_sent_to_every_endpoint() {
    let s = Setup::start(false).await;
    let second = ChainStandIn::start().await;
    let chain = Chain::connect(&section(&s, &[s.chain.url(), second.url()]), CHAIN_ID).await.unwrap();
    let receipt = chain.send(&s.key, &deposit()).await.unwrap();
    for endpoint in [&s.chain, &second] {
        let sent = endpoint.sent();
        assert_eq!((sent.len(), sent[0].hash, sent[0].from), (1, receipt.transaction_hash, wallet()));
    }
}

/// A nonce another transaction spent is signed again on the next; another refusal is reported and
/// nothing retried.
#[tokio::test]
async fn a_spent_nonce_is_signed_again_and_other_refusals_reported() {
    let s = Setup::start(false).await;
    let chain = Chain::connect(&section(&s, &[s.chain.url()]), CHAIN_ID).await.unwrap();
    s.chain.with(|l| l.refusals.push_back("nonce too low: next nonce 1, tx nonce 0".into()));
    chain.send(&s.key, &deposit()).await.unwrap();
    let sent = s.chain.sent();
    assert_eq!((sent.len(), sent[0].nonce), (1, 1), "signed again on the next nonce");
    s.chain.with(|l| l.refusals.push_back("insufficient funds for gas * price + value".into()));
    let err = chain.send(&s.key, &deposit()).await.unwrap_err();
    assert!(matches!(&err, TxError::NotSent { reason, .. } if reason.contains("insufficient funds")), "{err}");
    assert_eq!(s.chain.sent().len(), 1);
}

/// A transaction an endpoint took without answering is followed to its receipt, not signed again;
/// and when another endpoint answers that its nonce is spent because it was mined, it is that
/// transaction's receipt that counts.
#[tokio::test]
async fn a_transaction_taken_without_an_answer_is_followed_not_sent_again() {
    let s = Setup::start(false).await;
    s.chain.with(|l| l.swallow = true);
    let chain = Chain::connect(&section(&s, &[s.chain.url()]), CHAIN_ID).await.unwrap();
    let receipt = chain.send(&s.key, &deposit()).await.unwrap();
    assert_eq!(s.chain.sent().iter().map(|sent| sent.hash).collect::<Vec<_>>(), vec![receipt.transaction_hash]);

    let s = Setup::start(false).await;
    s.chain.with(|l| l.swallow = true);
    let spent = ChainStandIn::start().await;
    spent.with(|l| l.refusals.push_back("nonce too low: next nonce 1, tx nonce 0".into()));
    let chain = Chain::connect(&section(&s, &[s.chain.url(), spent.url()]), CHAIN_ID).await.unwrap();
    let receipt = chain.send(&s.key, &deposit()).await.unwrap();
    assert_eq!(s.chain.sent().iter().map(|sent| sent.hash).collect::<Vec<_>>(), vec![receipt.transaction_hash]);
    assert!(spent.sent().is_empty());
}

/// Once a transaction has landed, reads go to an endpoint that has reached its block: one behind it
/// is passed over.
#[tokio::test]
async fn reads_after_a_transaction_skip_an_endpoint_behind_it() {
    let s = Setup::start(false).await;
    let ahead = ChainStandIn::start().await;
    let chain = Chain::connect(&section(&s, &[s.chain.url(), ahead.url()]), CHAIN_ID).await.unwrap();
    let calls = |endpoint: &ChainStandIn| endpoint.with(|l| l.methods.get("eth_call").copied().unwrap_or_default());
    chain.view(VAULT, Vault::usdcCall {}).await.unwrap();
    assert_eq!((calls(&s.chain), calls(&ahead)), (1, 0), "the first endpoint answers while nothing has landed");
    chain.send(&s.key, &deposit()).await.unwrap();
    s.chain.with(|l| l.lag = 1);
    chain.view(VAULT, Vault::usdcCall {}).await.unwrap();
    assert_eq!((calls(&s.chain), calls(&ahead)), (1, 1), "the endpoint behind the transaction is passed over");
    s.chain.with(|l| l.lag = 0);
    chain.view(VAULT, Vault::usdcCall {}).await.unwrap();
    assert_eq!((calls(&s.chain), calls(&ahead)), (2, 1), "and answers again once it has caught up");
}

/// A fee over the configured cap is not offered: nothing is sent.
#[tokio::test]
async fn a_fee_over_the_cap_is_not_sent() {
    let s = Setup::start(false).await;
    let mut capped = section(&s, &[s.chain.url()]);
    // The stand-in's price is 0.1 gwei, so the fee offered is 0.2 gwei.
    capped.max_fee_gwei = trader::config::Gwei(199_999_999);
    let chain = Chain::connect(&capped, CHAIN_ID).await.unwrap();
    let err = chain.send(&s.key, &deposit()).await.unwrap_err();
    assert!(err.to_string().contains("over chain.max_fee_gwei"), "{err}");
    assert!(s.chain.sent().is_empty());
    capped.max_fee_gwei = trader::config::Gwei(200_000_000);
    Chain::connect(&capped, CHAIN_ID).await.unwrap().send(&s.key, &deposit()).await.unwrap();
}

/// An endpoint on another chain is refused; one that never answers is named by its place, and its
/// URL appears in no error, log of the config, or debug print.
#[tokio::test]
async fn endpoints_are_checked_and_never_printed() {
    let s = Setup::start(false).await;
    s.chain.with(|l| l.chain_id = 1);
    let err = Chain::connect(&section(&s, &[s.chain.url()]), CHAIN_ID).await.err().unwrap();
    assert!(err.to_string().contains("is on chain 1"), "{err}");
    let down = section(&s, &[DOWN.into()]);
    let err = Chain::connect(&down, CHAIN_ID).await.err().unwrap();
    assert!(!format!("{err:#} {err:?} {down:?}").contains("secret-key"), "{err:#}");
}

// ── Helpers ──

/// The `[chain]` section for `rpc`.
fn section(s: &Setup, rpc: &[String]) -> ChainSection {
    config(&s.node, rpc, s.dir.path(), false, "").chain.unwrap()
}

/// A deposit the stand-in chain takes: it holds the USDC and the allowance.
fn deposit() -> Call {
    Call { to: VAULT, data: Vault::depositCall { amount: U256::ZERO }.abi_encode().into(), what: "a deposit".into() }
}
