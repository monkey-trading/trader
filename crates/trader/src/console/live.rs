//! The console's live feed: one WebSocket per tab. It opens with every account and the totals,
//! then, at most once every `PUSH_INTERVAL`, sends each account whose status or figures changed,
//! its fills when they changed, and the totals. Each account carries only its equity curve's newest
//! point. It closes when the session ends; the browser sends nothing on it.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade, close_code};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use serde_json::{Value, json};
use tokio::time::{Instant, MissedTickBehavior, interval, timeout};
use tracing::{Instrument, debug, info, info_span};
use zeroize::Zeroizing;

use crate::console::guard::refusal;
use crate::console::{Server, session};

/// The feed's path.
pub const LIVE_PATH: &str = "/api/live";
/// How often the feed looks for changes.
pub const PUSH_INTERVAL: Duration = Duration::from_secs(1);
/// The longest a message may take to send; a browser slower than that is dropped.
const SEND_TIMEOUT: Duration = Duration::from_secs(10);
/// The largest message the feed takes from a browser, which has nothing to send.
const MAX_INCOMING: usize = 1024;

/// Opens the feed for a live session, within the cap on open feeds.
pub async fn live(State(server): State<Arc<Server>>, headers: HeaderMap, upgrade: WebSocketUpgrade) -> Response {
    let Some(token) = session::token(&headers).map(|token| Zeroizing::new(token.to_owned())) else {
        return refusal(StatusCode::UNAUTHORIZED, "log in first");
    };
    let Some(permit) = server.open_feed() else {
        return refusal(StatusCode::SERVICE_UNAVAILABLE, "too many live feeds are open");
    };
    let span = info_span!("live_feed", id = server.next_request_id());
    upgrade.max_message_size(MAX_INCOMING).on_upgrade(move |socket| {
        async move {
            info!("live feed opened");
            feed(&server, socket, &token).await;
            drop(permit);
            info!("live feed closed");
        }
        .instrument(span)
    })
}

/// Sends the opening snapshot, then the changes, until the browser leaves or the session ends.
async fn feed(server: &Server, mut socket: WebSocket, token: &str) {
    let mut watched = server.watch();
    let (accounts, totals) = server.overview_of(&mut watched, true);
    if !send(&mut socket, json!({ "type": "snapshot", "accounts": accounts, "totals": totals })).await {
        return;
    }
    let mut tick = interval(PUSH_INTERVAL);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Close(_)) | Err(_)) | None => return,
                Some(Ok(_)) => {}
            },
            _ = tick.tick() => {
                if !server.session_live(token, Instant::now()) {
                    let frame = CloseFrame { code: close_code::POLICY, reason: "the session ended".into() };
                    let _ = timeout(SEND_TIMEOUT, socket.send(Message::Close(Some(frame)))).await;
                    return;
                }
                let changes = server.changes(&mut watched);
                for change in changes {
                    if !send(&mut socket, change).await {
                        return;
                    }
                }
            }
        }
    }
}

/// Sends `message` as JSON text; whether it went within `SEND_TIMEOUT`.
async fn send(socket: &mut WebSocket, message: Value) -> bool {
    let sent = timeout(SEND_TIMEOUT, socket.send(Message::Text(message.to_string().into()))).await;
    let ok = matches!(sent, Ok(Ok(())));
    if !ok {
        debug!("the live feed's browser did not take a message");
    }
    ok
}
