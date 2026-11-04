//! The stream against the stand-in: every message once, in order, each frame verified, and a
//! connection that fails a check, leaves a gap, or falls silent replaced by one that resumes right
//! after the last message returned.

mod common;

use std::time::Duration;

use common::{Fault, StandIn};
use serde_json::json;
use tokio::time::timeout;
use trader::config::{NodeUrl, TlsPin};
use trader::node::Tls;
use trader::stream::Stream;
use trader::stream::message::Offset;

/// From the first seq, the messages arrive in order, live ones included, and a heartbeat is not a
/// message.
#[tokio::test]
async fn reads_the_log_in_order_from_its_start() {
    let stand_in = StandIn::start().await;
    stand_in.append((4, 0), tick(4));
    stand_in.append((5, 0), tick(5));
    stand_in.append((5, 1), json!({ "offset": "5:1", "type": "checkpoint", "seq": 5 }));
    stand_in.append((5, 2), json!({ "type": "heartbeat", "head": "5:1" }));
    let mut stream = stream(&stand_in, 5, Duration::from_secs(30));
    assert_eq!(next(&mut stream).await, (5, 0));
    assert_eq!(next(&mut stream).await, (5, 1));
    stand_in.append((6, 0), tick(6));
    assert_eq!(next(&mut stream).await, (6, 0));
    assert_eq!(stand_in.with(|i| i.connections.len()), 1);
    assert!(stand_in.with(|i| i.connections[0].starts_with("/v1/stream?from=5&signed=1&challenge=")));
}

/// A frame out of its place on the connection, a gap in the log, and a silent connection each close
/// the connection; each new one resumes after the last message returned, and none is returned twice.
#[tokio::test]
async fn resumes_after_each_failed_connection() {
    let stand_in = StandIn::start().await;
    for seq in 1..=4 {
        stand_in.append((seq, 0), tick(seq));
    }
    stand_in.with(|i| {
        i.faults.insert((0, (2, 0)), Fault::OutOfSequence);
        i.faults.insert((1, (3, 0)), Fault::Skip);
        i.faults.insert((2, (4, 0)), Fault::Silence);
    });
    let mut stream = stream(&stand_in, 1, Duration::from_secs(2));
    let seen: Vec<_> =
        [next(&mut stream).await, next(&mut stream).await, next(&mut stream).await, next(&mut stream).await].into();
    assert_eq!(seen, [(1, 0), (2, 0), (3, 0), (4, 0)]);
    let connections = stand_in.with(|i| i.connections.clone());
    let starts: Vec<_> = connections.iter().map(|c| c.split('&').next().unwrap().to_owned()).collect();
    assert_eq!(starts, ["/v1/stream?from=1", "/v1/stream?after=1:0", "/v1/stream?after=2:0", "/v1/stream?after=3:0"]);
    // Each connection is under its own challenge.
    let mut challenges: Vec<_> = connections.iter().map(|c| c.rsplit('=').next().unwrap().to_owned()).collect();
    challenges.dedup();
    assert_eq!(challenges.len(), 4);
}

/// A frame signed by another key, or altered after signing, stops the stream: nothing it says is
/// trusted.
#[tokio::test]
async fn a_frame_not_the_signers_stops_the_stream() {
    for fault in [Fault::OtherSigner, Fault::BadSignature] {
        let stand_in = StandIn::start().await;
        stand_in.append((1, 0), tick(1));
        stand_in.append((2, 0), tick(2));
        stand_in.with(|i| i.faults.insert((0, (2, 0)), fault));
        let mut stream = stream(&stand_in, 1, Duration::from_secs(30));
        assert_eq!(next(&mut stream).await, (1, 0));
        assert!(timeout(Duration::from_secs(10), stream.next()).await.unwrap().is_err(), "{fault:?}");
    }
}

/// A stream whose first message is not the seq it asked for has a gap, and is not read.
#[tokio::test]
async fn a_start_after_the_requested_seq_is_a_gap() {
    let stand_in = StandIn::start().await;
    stand_in.append((1, 0), tick(1));
    stand_in.append((2, 0), tick(2));
    stand_in.with(|i| i.faults.insert((0, (1, 0)), Fault::Skip));
    let mut stream = stream(&stand_in, 1, Duration::from_secs(30));
    // The first connection skips 1:0 and is dropped; the next serves 1:0 in order.
    assert_eq!(next(&mut stream).await, (1, 0));
    assert_eq!(next(&mut stream).await, (2, 0));
    assert_eq!(stand_in.with(|i| i.connections.len()), 2);
}

// ── Helpers ──

/// A `Tick` command at `seq`.
fn tick(seq: u64) -> serde_json::Value {
    common::command(seq, 1_000 * seq as i64, json!({ "type": "tick" }), json!([]))
}

/// The stand-in's log from `from`, verified against its signer.
fn stream(stand_in: &StandIn, from: u64, idle: Duration) -> Stream {
    let url = NodeUrl { host: "127.0.0.1".into(), port: stand_in.addr.port() };
    let tls = Tls::new(url, Some(TlsPin(stand_in.pin))).unwrap();
    Stream::new(tls, stand_in.signer(), stand_in.deployment(), idle, from)
}

/// The next message's offset, as `(seq, index)`.
async fn next(stream: &mut Stream) -> (u64, u32) {
    let frame = timeout(Duration::from_secs(20), stream.next()).await.expect("a message in time").unwrap();
    let Offset { seq, index } = frame.offset;
    let parsed: serde_json::Value = serde_json::from_str(&frame.msg).unwrap();
    assert_eq!(parsed["offset"], format!("{seq}:{index}"));
    (seq, index)
}
