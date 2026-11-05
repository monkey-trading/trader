//! The node client's pacing: requests spread out under a limit, a retry-later answer holds every
//! later request back for the delay it names, and the node's time counts on from its answers.

mod common;

use std::time::{Duration, Instant};

use common::{CHAIN_ID, StandIn, VAULT, address};
use serde_json::json;
use trader::config::{LimitsSection, NodeUrl, RateSection};
use trader::node::{CallError, Node, Tls};

fn node(stand_in: &StandIn, limits: LimitsSection) -> Node {
    let tls = Tls::new(
        NodeUrl { host: "127.0.0.1".into(), port: stand_in.addr.port() },
        Some(trader::config::TlsPin(stand_in.pin)),
    )
    .unwrap();
    Node::new(tls, stand_in.signer(), stand_in.deployment(), Duration::from_secs(2), &limits)
}

fn limits(per_second: u32, burst: u32) -> LimitsSection {
    let rate = RateSection { per_second, burst };
    LimitsSection { per_ip: rate, per_account: rate, connections: 2, ..LimitsSection::default() }
}

/// Requests over the configured rate wait instead of failing, and share the pool's connections.
#[tokio::test]
async fn requests_wait_for_the_limit_and_share_connections() {
    let stand_in = StandIn::start().await;
    stand_in.with(|i| i.limit = Some((10.0, 3.0)));
    let node = node(&stand_in, limits(10, 3));
    let started = Instant::now();
    for _ in 0..8 {
        node.nonce(address(0)).await.unwrap();
    }
    // Burst 3 keeps one in reserve: two go at once, the other six at ten a second.
    assert!(started.elapsed() >= Duration::from_millis(550), "{:?}", started.elapsed());
    assert_eq!(stand_in.with(|i| (i.rate_limited, i.tls_connections)), (0, 1), "one connection served all eight");
    assert_eq!(node.counts(), (8, 0));
}

/// A retry-later answer holds every request back for its delay; a read is sent again after it, and
/// an order is not, as it may have reached the node.
#[tokio::test]
async fn a_retry_later_answer_backs_off_every_request() {
    let stand_in = StandIn::start().await;
    stand_in.with(|i| i.limit = Some((1.0, 1.0)));
    // The bot believes the node is far more generous than it is.
    let node = node(&stand_in, limits(1000, 1000));
    node.nonce(address(0)).await.unwrap();
    let started = Instant::now();
    node.nonce(address(0)).await.unwrap();
    assert!(started.elapsed() >= Duration::from_millis(900), "{:?}", started.elapsed());
    let (requests, retry_later) = node.counts();
    assert_eq!((requests, retry_later), (3, 1), "refused once, then sent again after the delay");
    // An order refused for rate is an error, not sent again.
    let started = Instant::now();
    let request = serde_json::from_value(json!({ "account": address(0), "client_id": 1, "market": 1, "side": "buy",
        "qty": "1", "kind": { "kind": "market", "quoted": "100", "max_slippage_bps": 25 }, "reduce_only": false,
        "auth": null }))
    .unwrap();
    let err = node.place_order(&request).await.unwrap_err();
    assert!(matches!(err, CallError::RetryLater { .. }), "{err}");
    assert!(stand_in.with(|i| i.posts.is_empty()));
    // And the next request waits the delay out.
    node.nonce(address(0)).await.unwrap();
    assert!(started.elapsed() >= Duration::from_millis(900), "{:?}", started.elapsed());
    let _ = (CHAIN_ID, VAULT);
}

/// The node's time counts on from the answer it came in, and an answer to an earlier request is no
/// use to a caller that needs the time after some event.
#[tokio::test]
async fn the_nodes_time_counts_on_from_its_answers() {
    let stand_in = StandIn::start().await;
    let node = node(&stand_in, limits(1000, 1000));
    assert_eq!(node.latest_time_ms(), None);
    let before = tokio::time::Instant::now();
    node.nonce(address(0)).await.unwrap();
    let first = node.time_ms(before).unwrap();
    assert!((common::T0 as u64..common::T0 as u64 + 1_000).contains(&first), "{first}");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let later = node.latest_time_ms().unwrap();
    assert!(later >= first + 150, "{later} after {first}");
    assert_eq!(node.time_ms(tokio::time::Instant::now()), None, "no answer to a request sent since");
}
