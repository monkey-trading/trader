//! Declarations against the stand-in node: only a confirmed one is signed, by the account, and a
//! retry-later answer or a moved nonce asks again.

mod common;

use engine_core::access::EligibilityClass::{EligibleClient, Professional};
use engine_core::access::EligibilityRequest;
use engine_core::eip712::{self, eligibility_hash, recover};
use engine_core::types::AccountId;
use serde_json::{Value, json};
use trader::accounts::cli::{self, Action, Class};
use trader::accounts::declare::{Confirmed, Declared};

use common::lifecycle::{Setup, account, until};
use common::{CHAIN_ID, VAULT};

/// An account that declared is not asked to again.
#[tokio::test]
async fn an_account_that_declared_is_not_asked_again() {
    let s = Setup::start(false).await;
    s.answer(&eligibility_path(), 200, declared("professional"));
    let declared = s.lifecycle.declare(&s.key, Confirmed { account: account(), class: Professional }).await.unwrap();
    assert_eq!(declared, Declared::Already(Professional));
    assert!(s.posts("/v1/eligibility").is_empty());
}

/// A confirmed declaration is the account's signature over its class, nonce and expiry.
#[tokio::test]
async fn a_confirmed_declaration_is_signed_by_the_account_and_recorded() {
    let s = Setup::start(false).await;
    s.answer(&eligibility_path(), 200, declared_none());
    s.node.with(|i| {
        i.nonce = 4;
        i.replies.push_back(attested("eligible_client"));
    });
    let declared = s.lifecycle.declare(&s.key, Confirmed { account: account(), class: EligibleClient }).await.unwrap();
    assert_eq!(declared, Declared::Recorded(EligibleClient));
    let posts = s.posts("/v1/eligibility");
    assert_eq!(posts.len(), 1);
    let request = signed_declaration(&posts[0]);
    assert_eq!((request.account, request.class, request.auth.nonce), (account(), EligibleClient, 4));
    let now = s.node.with(|i| i.time_ms) as i64;
    assert_eq!(request.auth.expires_at, now + 60_000, "a minute past the node's time");
}

/// A retry-later answer, from the fresh-account budgets (429) or a full screening (503), and a stale
/// nonce (409) are each followed by the declaration again, on the account's current nonce.
#[tokio::test]
async fn retry_later_and_stale_nonce_answers_ask_again() {
    for status in [429, 503, 409] {
        let s = Setup::start(false).await;
        s.answer(&eligibility_path(), 200, declared_none());
        s.node.with(|i| {
            i.statuses.push_back(status);
            i.replies.push_back(attested("professional"));
        });
        let declared = s.lifecycle.declare(&s.key, Confirmed { account: account(), class: Professional }).await;
        assert_eq!(declared.unwrap(), Declared::Recorded(Professional), "after {status}");
        let posts = s.posts("/v1/eligibility");
        assert_eq!(posts.len(), 2, "after {status}");
        // The stand-in moves the nonce on every request it takes.
        assert_eq!(signed_declaration(&posts[1]).auth.nonce, 1, "after {status}");
    }
}

/// A refusal in the log is the outcome; the bot does not ask again.
#[tokio::test]
async fn a_refusal_in_the_log_is_reported() {
    let s = Setup::start(false).await;
    s.answer(&eligibility_path(), 200, declared_none());
    let rejected = json!([{ "type": "eligibility_rejected", "account": account(),
        "reason": { "kind": "access", "reason": "united_states" } }]);
    s.node.with(|i| i.replies.push_back(rejected));
    let declared = s.lifecycle.declare(&s.key, Confirmed { account: account(), class: Professional }).await.unwrap();
    assert!(matches!(&declared, Declared::Refused(reason) if reason.contains("UnitedStates")), "{declared:?}");
    assert_eq!(s.posts("/v1/eligibility").len(), 1);
}

/// When the answer never comes, the recorded declaration says whether it took: it is not asked
/// again once it has.
#[tokio::test]
async fn a_lost_answer_is_checked_against_the_record() {
    let s = Setup::start(false).await;
    s.answer(&eligibility_path(), 200, declared_none());
    // Past the config's two-second request timeout.
    s.node.with(|i| i.post_delay = Some(std::time::Duration::from_secs(3)));
    let declare = s.lifecycle.declare(&s.key, Confirmed { account: account(), class: Professional });
    let record = async {
        until("the declaration", || !s.posts("/v1/eligibility").is_empty()).await;
        s.answer(&eligibility_path(), 200, declared("professional"));
    };
    let (declared, ()) = tokio::join!(declare, record);
    assert_eq!(declared.unwrap(), Declared::Recorded(Professional));
    assert_eq!(s.posts("/v1/eligibility").len(), 1);
}

/// A confirmation for another account, and an account that declared another class, are refused
/// before anything is signed.
#[tokio::test]
async fn mismatched_confirmations_and_classes_are_refused() {
    let s = Setup::start(false).await;
    s.answer(&eligibility_path(), 200, declared_none());
    let other = Confirmed { account: AccountId([7; 20]), class: Professional };
    assert!(s.lifecycle.declare(&s.key, other).await.is_err());
    s.answer(&eligibility_path(), 200, declared("eligible_client"));
    let err = s.lifecycle.declare(&s.key, Confirmed { account: account(), class: Professional }).await.unwrap_err();
    assert!(err.to_string().contains("declared EligibleClient already"), "{err}");
    assert!(s.posts("/v1/eligibility").is_empty());
}

/// `trader accounts declare` shows the statement and declares for an account `--confirm` names,
/// never for one it does not name outside a terminal, refuses a `--confirm` of an account the
/// config does not have, and reports an account that declared as declared. It needs no chain.
#[tokio::test]
async fn the_command_declares_only_for_confirmed_accounts() {
    let s = Setup::start(false).await;
    s.chain.with(|l| l.down = true);
    s.answer(&eligibility_path(), 200, declared_none());
    s.node.with(|i| i.replies.push_back(attested("professional")));
    let declare = |confirm: Vec<AccountId>| Action::Declare { class: Class::Professional, confirm };
    let cfg = || common::lifecycle::config(&s.node, &[s.chain.url()], s.dir.path(), false, "");
    let run = |confirm: Vec<AccountId>| async {
        let mut out = Vec::new();
        let result = cli::run(cfg(), vec![], declare(confirm), |_| None, false, &mut out).await;
        result.map(|()| String::from_utf8(out).unwrap())
    };
    let out = run(vec![]).await.unwrap();
    assert!(
        out.contains("I attest that I am a professional client.") && out.contains("not declared: not confirmed"),
        "{out}"
    );
    assert!(s.posts("/v1/eligibility").is_empty(), "nothing is declared without a confirmation");
    assert!(run(vec![AccountId([7; 20])]).await.is_err());
    assert!(s.posts("/v1/eligibility").is_empty());
    let out = run(vec![account()]).await.unwrap();
    assert!(out.contains(&format!("{}: declared Professional", account())), "{out}");
    assert_eq!(s.posts("/v1/eligibility").len(), 1);
    s.answer(&eligibility_path(), 200, declared("professional"));
    let out = run(vec![account()]).await.unwrap();
    assert!(out.contains(&format!("{}: declared Professional already", account())), "{out}");
    assert_eq!(s.posts("/v1/eligibility").len(), 1);
}

// ── Helpers ──

fn eligibility_path() -> String {
    format!("/v1/accounts/{}/eligibility", account())
}

fn declared(class: &str) -> Value {
    json!({ "class": class, "statement": "I attest." })
}

fn declared_none() -> Value {
    json!({ "class": null, "statement": null })
}

fn attested(class: &str) -> Value {
    json!([{ "type": "eligibility_attested", "account": account(), "class": class }])
}

/// The declaration in `body`, checked to be signed by the account over its own terms.
fn signed_declaration(body: &Value) -> EligibilityRequest {
    let request: EligibilityRequest = serde_json::from_value(body.clone()).unwrap();
    let domain = eip712::domain(CHAIN_ID, AccountId(VAULT));
    let hash = eligibility_hash(&domain, request.account, request.class, request.auth.nonce, request.auth.expires_at);
    assert_eq!(recover(&hash, &request.auth.signature), Some(account()), "signed by the account");
    request
}
