//! Declarations against the stand-in node: only a confirmed one is signed, by the account, and a
//! retry-later answer or a moved nonce asks again.

mod common;

use engine_core::access::EligibilityClass::{EligibleClient, Professional};
use engine_core::access::EligibilityRequest;
use engine_core::eip712::{self, eligibility_hash, recover};
use engine_core::types::AccountId;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::time::Instant;
use trader::accounts::Lifecycle;
use trader::accounts::cli::{self, Action, Class};
use trader::accounts::declare::{Confirmed, Declared};

use common::lifecycle::{Setup, account, until, within};
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

/// Each declaration waits its turn under the node's fresh-account budget for this client: at one a
/// second with nothing in hand, the second request, after a stale nonce, goes a second after the first.
#[tokio::test]
async fn declarations_wait_their_turn_under_the_fresh_account_budget() {
    let s = Setup::start(false).await;
    let budget = "[node.limits]\nfresh_accounts = { per_minute = 60, burst = 1 }\n";
    let cfg = common::lifecycle::config(&s.node, &[s.chain.url()], s.dir.path(), false, budget);
    let lifecycle = Lifecycle::connect(&cfg, |_| None).await.unwrap();
    s.answer(&eligibility_path(), 200, declared_none());
    s.node.with(|i| {
        i.statuses.push_back(409);
        i.replies.push_back(attested("professional"));
    });
    let started = Instant::now();
    let declared = within(lifecycle.declare(&s.key, Confirmed { account: account(), class: Professional })).await;
    assert_eq!(declared.unwrap(), Declared::Recorded(Professional));
    assert_eq!(s.posts("/v1/eligibility").len(), 2);
    assert!(started.elapsed() >= Duration::from_millis(950), "took {:?}", started.elapsed());
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

/// A declaration the node refuses in the log fails the command, so nothing run after it takes the
/// account for declared.
#[tokio::test]
async fn the_command_fails_for_a_refused_declaration() {
    let s = Setup::start(false).await;
    s.answer(&eligibility_path(), 200, declared_none());
    let rejected = json!([{ "type": "eligibility_rejected", "account": account(),
        "reason": { "kind": "access", "reason": "united_states" } }]);
    s.node.with(|i| i.replies.push_back(rejected));
    let declare = Action::Declare { class: Class::Professional, confirm: vec![account()] };
    let mut out = Vec::new();
    let cfg = common::lifecycle::config(&s.node, &[s.chain.url()], s.dir.path(), false, "");
    let err = within(cli::run(cfg, vec![], declare, |_| None, false, &mut out)).await.unwrap_err();
    assert!(err.to_string().contains("1 of 1 accounts failed"), "{err}");
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains(&format!("{}: failed: the node refused the declaration", account())), "{out}");
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

/// An account whose key does not unlock, or that is listed twice, is reported under its key's path
/// and fails the command, and the others carry on; an account named alone is acted for without them.
#[tokio::test]
async fn an_account_that_cannot_be_used_is_reported_and_the_others_carry_on() {
    let s = Setup::start(false).await;
    s.answer(&eligibility_path(), 200, declared("professional"));
    let entry = |file: &str| {
        format!(
            "[[account]]\nkey = {{ type = \"hex_file\", path = \"{file}\" }}\n[[account.strategy]]\n\
             market = \"BTC-USD\"\nbar = \"1m\"\nkind = \"sma_crossover\"\nfast = 1\nslow = 2\nmax_slippage_bps = 25\n"
        )
    };
    let extra = format!("{}{}", entry("missing.hex"), entry("account.hex"));
    let cfg = || common::lifecycle::config(&s.node, &[s.chain.url()], s.dir.path(), false, &extra);
    let declare = || Action::Declare { class: Class::Professional, confirm: vec![] };
    let mut out = Vec::new();
    let err = within(cli::run(cfg(), vec![], declare(), |_| None, false, &mut out)).await.unwrap_err();
    assert!(err.to_string().contains("2 of 3 accounts failed"), "{err}");
    let out = String::from_utf8(out).unwrap();
    let missing = s.dir.path().join("missing.hex").display().to_string();
    let twice = format!("{}: {} is listed twice", s.dir.path().join("account.hex").display(), account());
    assert!(out.contains(&format!("{missing}: ")) && out.contains(&twice), "{out}");
    assert!(out.contains(&format!("{}: declared Professional already", account())), "{out}");
    let mut out = Vec::new();
    within(cli::run(cfg(), vec![account()], declare(), |_| None, false, &mut out)).await.unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(!out.contains("missing.hex") && !out.contains("listed twice"), "{out}");
}

/// An action that sends nothing from the funding key goes ahead without it: a funding key that does
/// not load fails only the actions it pays for.
#[tokio::test]
async fn the_funding_key_is_unlocked_only_for_the_actions_it_pays_for() {
    let s = Setup::start(false).await;
    s.answer(&eligibility_path(), 200, declared("professional"));
    let unloadable = "[funding]\nkey = { type = \"hex_file\", path = \"missing.hex\" }\n";
    let cfg = || common::lifecycle::config(&s.node, &[s.chain.url()], s.dir.path(), false, unloadable);
    let declare = Action::Declare { class: Class::Professional, confirm: vec![] };
    within(cli::run(cfg(), vec![], declare, |_| None, false, &mut Vec::new())).await.unwrap();
    let err = within(cli::run(cfg(), vec![], Action::Claim, |_| None, false, &mut Vec::new())).await.unwrap_err();
    assert!(format!("{err:#}").contains("the funding key"), "{err:#}");
}

/// Without a `[chain]` section, `trader accounts declare` declares, and an action that reads the
/// chain says it needs one.
#[tokio::test]
async fn declaring_needs_no_chain_section() {
    let s = Setup::start(false).await;
    s.answer(&eligibility_path(), 200, declared_none());
    s.node.with(|i| i.replies.push_back(attested("professional")));
    let text = std::fs::read_to_string(s.dir.path().join("bot.toml")).unwrap();
    let start = text.find("[chain]").unwrap();
    let end = start + text[start..].find("\n\n").unwrap();
    let path = s.dir.path().join("no-chain.toml");
    std::fs::write(&path, format!("{}{}", &text[..start], &text[end..])).unwrap();
    let cfg = || trader::config::Config::load(&path).unwrap();
    assert!(cfg().chain.is_none());
    let declare = Action::Declare { class: Class::Professional, confirm: vec![account()] };
    let mut out = Vec::new();
    within(cli::run(cfg(), vec![], declare, |_| None, false, &mut out)).await.unwrap();
    assert!(String::from_utf8(out).unwrap().contains(&format!("{}: declared Professional", account())));
    let mut out = Vec::new();
    within(cli::run(cfg(), vec![], Action::Status, |_| None, false, &mut out)).await.unwrap_err();
    assert!(String::from_utf8(out).unwrap().contains("has no [chain] section"));
}

/// An account whose declaration cannot be read is reported in its own line, and the others are
/// asked and declared as usual.
#[tokio::test]
async fn a_declaration_that_cannot_be_read_fails_its_account_alone() {
    let s = Setup::start(false).await;
    s.answer(&eligibility_path(), 500, json!({ "error": "internal error" }));
    let other = common::lifecycle::funding_wallet();
    s.answer(&format!("/v1/accounts/{}/eligibility", AccountId(other.into_array())), 200, declared("professional"));
    let entry = "[[account]]\nkey = { type = \"hex_file\", path = \"funding.hex\" }\n[[account.strategy]]\n\
                 market = \"BTC-USD\"\nbar = \"1m\"\nkind = \"sma_crossover\"\nfast = 1\nslow = 2\nmax_slippage_bps = 25\n";
    let cfg = common::lifecycle::config(&s.node, &[s.chain.url()], s.dir.path(), false, entry);
    let declare = Action::Declare { class: Class::Professional, confirm: vec![] };
    let mut out = Vec::new();
    let err = within(cli::run(cfg, vec![], declare, |_| None, false, &mut out)).await.unwrap_err();
    assert!(err.to_string().contains("1 of 2 accounts failed"), "{err}");
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains(&format!("{}: failed:", account())), "{out}");
    assert!(out.contains(&format!("{}: declared Professional already", AccountId(other.into_array()))), "{out}");
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
