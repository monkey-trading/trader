//! The public log over one signed stream connection (`/v1/stream?signed=1`). Each frame is checked
//! with `engine_wire::FrameVerifier` against the attested signer before its message is read, and
//! the messages must follow each other with no gap. A connection that fails a check, falls silent
//! for longer than the stream timeout, or drops is closed, and a new one resumes right after the
//! last message returned, so the caller sees every message once, in order.

pub mod message;

use std::time::Duration;

use anyhow::Context;
use engine_wire::{Connection, Deployment, Error as WireError, FrameError, FrameVerifier};
use futures_util::StreamExt;
use rand::RngCore;
use tokio::net::TcpStream;
use tokio::time::{sleep, timeout};
use tokio_rustls::client::TlsStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{WebSocketStream, client_async};
use tracing::{info, warn};

use crate::node::Tls;
use crate::stream::message::{Head, Offset};

/// The first wait before reconnecting.
const MIN_BACKOFF: Duration = Duration::from_secs(1);
/// The longest wait before reconnecting.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// A verified message of the log.
#[derive(Debug)]
pub struct Frame {
    pub offset: Offset,
    /// The node's time when it sent the frame, in milliseconds since the epoch, as it signed it.
    pub time: u64,
    /// The message's JSON, as the node signed it.
    pub msg: String,
}

/// Why the stream cannot go on.
#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    /// A frame's signature is not the attested signer's over it: another key signed it, or it is
    /// not the frame that was signed.
    #[error("a stream frame is not signed by the attested signer")]
    WrongSigner,
    /// A frame of a version or shape this crate cannot read, or a verified message the bot cannot
    /// place in the log: no type, or no offset it can read. The node would send it again on every
    /// connection.
    #[error("{0:#}")]
    Unreadable(anyhow::Error),
}

/// The log, read from one seq on across reconnections.
pub struct Stream {
    tls: Tls,
    signer: [u8; 20],
    deployment: Deployment,
    idle: Duration,
    /// The first seq, until a message has been returned.
    from: u64,
    /// The last message returned.
    last: Option<Offset>,
    open: Option<Open>,
    backoff: Duration,
    /// Whether the stream is down and has said so, until a frame verifies again.
    down: bool,
}

/// One open connection and the verifier of its frames.
struct Open {
    ws: WebSocketStream<TlsStream<TcpStream>>,
    verifier: FrameVerifier,
}

impl Stream {
    /// The log from seq `from` on, from the node `tls` connects to, whose frames `signer` signs for
    /// `deployment`; a connection silent for `idle` is dropped.
    pub fn new(tls: Tls, signer: [u8; 20], deployment: Deployment, idle: Duration, from: u64) -> Self {
        Self { tls, signer, deployment, idle, from, last: None, open: None, backoff: MIN_BACKOFF, down: false }
    }

    /// The next message after the last one returned, verified and in order, connecting and
    /// reconnecting as needed. Cancel-safe: a frame counts as returned only once this returns it.
    /// Error cases:
    /// - a frame's signature is not the attested signer's over it
    /// - a frame of another version or shape, or a verified message with no type or readable offset
    pub async fn next(&mut self) -> Result<Frame, StreamError> {
        loop {
            let Some(open) = self.open.as_mut() else {
                match self.connect().await {
                    Ok(open) => self.open = Some(open),
                    Err(e) => self.drop_connection(&format!("connecting: {e:#}")).await,
                }
                continue;
            };
            let text = match timeout(self.idle, open.ws.next()).await {
                Ok(Some(Ok(Message::Text(text)))) => text,
                Ok(Some(Ok(Message::Close(_)) | Err(_)) | None) => {
                    self.drop_connection("the connection closed").await;
                    continue;
                }
                Ok(Some(Ok(_))) => continue,
                Err(_) => {
                    self.drop_connection(&format!("no frame for {:?}", self.idle)).await;
                    continue;
                }
            };
            let frame = match open.verifier.verify_text(text.as_str()) {
                Ok(frame) => frame,
                Err(FrameError::Refused(WireError::WrongSigner)) => return Err(StreamError::WrongSigner),
                // A frame this crate cannot read, which every connection would send again.
                Err(e @ (FrameError::NotAFrame(_) | FrameError::Refused(WireError::UnsupportedVersion(_)))) => {
                    return Err(StreamError::Unreadable(e.into()));
                }
                Err(e) => {
                    self.drop_connection(&format!("a frame does not verify: {e}")).await;
                    continue;
                }
            };
            let offset = match read_offset(&frame.msg) {
                Ok(offset) => offset,
                Err(e) => return Err(StreamError::Unreadable(e)),
            };
            self.recovered();
            let Some(offset) = offset else { continue };
            if !self.follows(offset) {
                let after = self.last.map_or(format!("the start at {}", self.from), |o| o.to_string());
                self.drop_connection(&format!("message {offset} does not follow {after}")).await;
                continue;
            }
            self.last = Some(offset);
            return Ok(Frame { offset, time: frame.time, msg: frame.msg });
        }
    }

    /// Whether `offset` is the next message after the last one returned, or the first of the start.
    fn follows(&self, offset: Offset) -> bool {
        match self.last {
            Some(last) => last.is_followed_by(offset),
            None => offset == Offset { seq: self.from, index: 0 },
        }
    }

    /// Opens a connection from the start, or right after the last message returned, under a fresh
    /// challenge that every frame on it must be signed under.
    /// Error cases:
    /// - the connection, its TLS handshake, or its WebSocket upgrade fails or takes longer than `idle`
    async fn connect(&self) -> anyhow::Result<Open> {
        let mut challenge = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut challenge);
        let start = match self.last {
            Some(last) => format!("after={last}"),
            None => format!("from={}", self.from),
        };
        let path = format!("/v1/stream?{start}&signed=1&challenge={}", hex::encode(challenge));
        let request = format!("wss://{}{path}", self.tls.url().authority()).into_client_request()?;
        let open = async {
            let tls = self.tls.connect().await?;
            let (ws, _) = client_async(request, tls).await?;
            anyhow::Ok(ws)
        };
        let ws = timeout(self.idle, open).await.context("no connection in time")??;
        info!(start, "stream connected");
        let verifier = FrameVerifier::new(self.signer, self.deployment, Connection::new(challenge, &path));
        Ok(Open { ws, verifier })
    }

    /// Closes the connection after `reason`, says so once per outage, and waits before the next
    /// attempt, longer after each failure up to `MAX_BACKOFF`.
    async fn drop_connection(&mut self, reason: &str) {
        self.open = None;
        if !self.down {
            warn!(alert = "stream_down", resume_after = ?self.last, "the stream is down: {reason}; reconnecting");
            self.down = true;
        } else {
            info!(resume_after = ?self.last, backoff = ?self.backoff, "the stream is still down: {reason}");
        }
        sleep(self.backoff).await;
        self.backoff = (self.backoff * 2).min(MAX_BACKOFF);
    }

    /// Notes a verified frame: an outage is over.
    fn recovered(&mut self) {
        self.backoff = MIN_BACKOFF;
        if self.down {
            info!(alert_resolved = "stream_down", "the stream is back");
            self.down = false;
        }
    }
}

/// Where `msg` sits: `None` for a heartbeat, else its offset.
/// Error cases:
/// - a message with no type, or other than a heartbeat with no offset, or one that is not
///   `<seq>:<index>`
fn read_offset(msg: &str) -> anyhow::Result<Option<Offset>> {
    let head: Head<'_> = serde_json::from_str(msg).context("a stream message with no type")?;
    if head.kind == "heartbeat" {
        return Ok(None);
    }
    let offset = head.offset.with_context(|| format!("a {} stream message with no offset", head.kind))?;
    Ok(Some(offset.parse()?))
}
