//! The node client against the stand-in: an answer counts only once its signature verifies against
//! the configured signer, the request as sent, and the challenge sent with it.

mod common;

use std::time::Duration;

use common::{Answers, StandIn};
use engine_wire::Deployment;
use serde_json::json;
use trader::config::{LimitsSection, NodeUrl, TlsPin};
use trader::node::{CallError, Node, Tls};

/// A signed answer parses, with the node's time and log head it was signed at.
#[tokio::test]
async fn a_signed_answer_is_read() {
    let stand_in = StandIn::start().await;
    stand_in.with(|i| {
        i.nonce = 7;
        i.head_seq = 42;
    });
    let answer = node(&stand_in).nonce("0x1111111111111111111111111111111111111111".parse().unwrap()).await.unwrap();
    assert_eq!((answer.body.next_nonce, answer.head_seq, answer.time_ms), (7, 42, 1_800_000_000_000));
}

/// An answer another key signed, or one altered after signing, is not the signer's; an unsigned
/// one, or one bound to another challenge, does not verify. None is read.
#[tokio::test]
async fn an_answer_that_does_not_verify_is_no_answer() {
    let stand_in = StandIn::start().await;
    let node = node(&stand_in);
    let account = "0x1111111111111111111111111111111111111111".parse().unwrap();
    for answers in [Answers::OtherSigner, Answers::Tampered, Answers::Unsigned, Answers::OtherChallenge] {
        stand_in.with(|i| i.answers = Some(answers));
        let err = node.nonce(account).await.unwrap_err();
        match (&err, answers) {
            (CallError::WrongSigner { .. }, Answers::OtherSigner | Answers::Tampered)
            | (CallError::Unverified { .. }, Answers::Unsigned | Answers::OtherChallenge) => {}
            _ => panic!("{answers:?}: {err}"),
        }
    }
}

/// A verified refusal carries the node's status and answer.
#[tokio::test]
async fn a_signed_refusal_is_a_refusal() {
    let stand_in = StandIn::start().await;
    let err = node(&stand_in).account("0x2222222222222222222222222222222222222222".parse().unwrap()).await.unwrap_err();
    let CallError::Refused { status, error, .. } = &err else { panic!("{err}") };
    assert_eq!((*status, error.as_str()), (404, r#"{"error":"not found"}"#));
}

/// The signature binds the request: an answer to a POST verifies against the body sent.
#[tokio::test]
async fn a_post_answer_binds_its_body() {
    let stand_in = StandIn::start().await;
    stand_in.with(|i| i.replies.push_back(json!([{ "type": "admin_applied" }])));
    let answer: trader::node::Verified<serde_json::Value> =
        node(&stand_in).post("/v1/orders", &json!({ "client_id": 1 }), None).await.unwrap();
    assert_eq!(answer.body, json!([{ "type": "admin_applied" }]));
    assert_eq!(stand_in.with(|i| i.posts.clone()), [("/v1/orders".to_owned(), json!({ "client_id": 1 }))]);
}

/// A node whose TLS key is not the pinned one is not reached at all.
#[tokio::test]
async fn a_tls_key_other_than_the_pin_is_refused() {
    let stand_in = StandIn::start().await;
    let url = NodeUrl { host: "127.0.0.1".into(), port: stand_in.addr.port() };
    let mut pin = stand_in.pin;
    pin[0] ^= 1;
    let node = Node::new(
        Tls::new(url, Some(TlsPin(pin))).unwrap(),
        stand_in.signer(),
        deployment(),
        TIMEOUT,
        &LimitsSection::default(),
    );
    let err = node.nonce("0x1111111111111111111111111111111111111111".parse().unwrap()).await.unwrap_err();
    assert!(matches!(err, CallError::Failed { .. }), "{err}");
    assert!(stand_in.with(|i| i.posts.is_empty()));
}

/// Without a pin the certificate must chain to a public root, which a self-signed one does not.
#[tokio::test]
async fn without_a_pin_a_self_signed_node_is_refused() {
    let stand_in = StandIn::start().await;
    let url = NodeUrl { host: "127.0.0.1".into(), port: stand_in.addr.port() };
    let node =
        Node::new(Tls::new(url, None).unwrap(), stand_in.signer(), deployment(), TIMEOUT, &LimitsSection::default());
    let err = node.markets().await.unwrap_err();
    assert!(format!("{err}").contains("certificate"), "{err}");
}

/// The marks history is read page by page through its window, each page verified, until the node
/// says the window is done.
#[tokio::test]
async fn the_marks_history_pages_through_its_window() {
    let stand_in = StandIn::start().await;
    stand_in.with(|i| {
        i.marks = (0..5).map(|minute| common::bucket(common::T0 + minute * common::MINUTE, "100")).collect();
        i.marks_page = Some(2);
    });
    let (from, to) = (common::T0 + common::MINUTE, common::T0 + 3 * common::MINUTE);
    let buckets = node(&stand_in).marks(1, "1m", from, to).await.unwrap();
    let starts: Vec<_> = buckets.iter().map(|bucket| bucket.ts).collect();
    assert_eq!(starts, [from, from + common::MINUTE, to]);
    assert_eq!(stand_in.with(|i| i.marks_reads), 2);
}

/// A page whose `next` does not move past the window it answered is refused, not followed.
#[tokio::test]
async fn a_marks_page_that_does_not_move_on_is_refused() {
    let stand_in = StandIn::start().await;
    stand_in.with(|i| {
        i.marks = vec![common::bucket(common::T0, "100")];
        i.marks_next = Some(common::T0);
    });
    let err = node(&stand_in).marks(1, "1m", common::T0, common::T0 + common::MINUTE).await.unwrap_err();
    assert!(err.to_string().contains("outside"), "{err}");
}

// ── Helpers ──

const TIMEOUT: Duration = Duration::from_secs(5);

/// The stand-in's deployment.
fn deployment() -> Deployment {
    Deployment { chain_id: common::CHAIN_ID, vault: common::VAULT }
}

/// A client of the stand-in, pinned to its key and checking its signer.
fn node(stand_in: &StandIn) -> Node {
    let url = NodeUrl { host: "127.0.0.1".into(), port: stand_in.addr.port() };
    Node::new(
        Tls::new(url, Some(TlsPin(stand_in.pin))).unwrap(),
        stand_in.signer(),
        deployment(),
        TIMEOUT,
        &LimitsSection::default(),
    )
}
