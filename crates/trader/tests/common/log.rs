//! The stand-in's log over signed stream connections: each frame signed as the node signs it, with
//! the faults a test injects.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::Uri;
use axum::response::Response;
use engine_wire::{Connection, FRAME_VERSION, SignedFrame, frame_digest, parse_challenge};
use futures_util::{SinkExt, StreamExt};

use super::{Fault, StandIn};

/// The log over a signed stream connection, from `from` or after `after`, then whatever is
/// appended, with each fault injected once.
pub(super) async fn stream(State(s): State<Arc<StandIn>>, uri: Uri, ws: WebSocketUpgrade) -> Response {
    let target = uri.path_and_query().unwrap().to_string();
    let query: BTreeMap<String, String> = uri
        .query()
        .unwrap_or_default()
        .split('&')
        .filter_map(|pair| pair.split_once('=').map(|(k, v)| (k.to_owned(), v.to_owned())))
        .collect();
    let challenge = parse_challenge(&query["challenge"]).unwrap();
    let start = match (query.get("from"), query.get("after")) {
        (Some(seq), None) => (seq.parse::<u64>().unwrap(), 0),
        (None, Some(after)) => {
            let (seq, index) = after.split_once(':').unwrap();
            (seq.parse().unwrap(), index.parse::<u32>().unwrap() + 1)
        }
        _ => panic!("a stream connection names from or after: {target}"),
    };
    let place = s.with(|i| {
        i.connections.push(target.clone());
        i.connections.len() - 1
    });
    ws.on_upgrade(move |socket| serve_log(s, socket, Connection::new(challenge, &target), start, place))
}

/// Sends the log's messages at or after `start` in signed frames, with the faults set for the
/// connection at `place`, each frame followed by a ping naming its offset. The bot's client answers
/// a ping when it next reads, which it does only once it has applied the frame before it.
async fn serve_log(s: Arc<StandIn>, socket: WebSocket, connection: Connection, start: (u64, u32), place: usize) {
    let (mut socket, mut replies) = socket.split();
    let pongs = Arc::clone(&s);
    tokio::spawn(async move {
        while let Some(Ok(reply)) = replies.next().await {
            let Message::Pong(payload) = reply else { continue };
            let offset = std::str::from_utf8(&payload).ok().and_then(|text| {
                let (seq, index) = text.split_once(':')?;
                Some((seq.parse::<u64>().ok()?, index.parse::<u32>().ok()?))
            });
            pongs.with(|i| i.applied = i.applied.max(offset));
        }
    });
    let (mut n, mut previous) = (0u64, [0u8; 32]);
    let mut next = 0;
    loop {
        let appended = s.appended.notified();
        let entry = s.with(|i| {
            let pending: Vec<_> = i.log.iter().filter(|(offset, _)| *offset >= start).cloned().collect();
            pending.get(next).cloned().map(|(offset, msg)| {
                (offset, msg, i.faults.remove(&(place, offset)), i.frame_time.unwrap_or(i.time_ms))
            })
        });
        let Some((offset, msg, fault, time)) = entry else {
            appended.await;
            continue;
        };
        next += 1;
        let (msg, other) = match fault {
            Some(Fault::Skip) => continue,
            Some(Fault::Silence) => return std::future::pending().await,
            Some(Fault::OutOfSequence) => {
                n += 1;
                (msg, Some(false))
            }
            Some(Fault::BadSignature) => (msg, None),
            Some(Fault::OtherSigner) => (msg, Some(true)),
            None => (msg, Some(false)),
        };
        let digest = frame_digest(&s.deployment(), &connection, &previous, n, time, &msg);
        let sig = match other {
            Some(other) => s.sign(digest, other),
            None => s.sign(frame_digest(&s.deployment(), &connection, &previous, n, time, "altered"), false),
        };
        let frame = SignedFrame { v: FRAME_VERSION, n, time, msg, sig };
        (n, previous) = (n + 1, digest);
        if socket.send(Message::Text(serde_json::to_string(&frame).unwrap().into())).await.is_err() {
            return;
        }
        if socket.send(Message::Ping(format!("{}:{}", offset.0, offset.1).into())).await.is_err() {
            return;
        }
    }
}
