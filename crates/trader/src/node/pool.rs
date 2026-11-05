//! The bot's HTTP/1.1 connections to the node: a bounded set kept open between requests, so a
//! thousand accounts share a handful of TLS connections rather than each call opening one.

use std::sync::{Mutex, PoisonError};

use anyhow::anyhow;
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::client::conn::http1::SendRequest;
use hyper::header::{CONTENT_TYPE, HOST};
use hyper::{HeaderMap, Method, Request};
use hyper_util::rt::TokioIo;
use tokio::sync::Semaphore;

use crate::node::Tls;

/// The largest answer body the bot reads: a page of marks history is a few hundred kilobytes.
const MAX_ANSWER_BYTES: usize = 4 << 20;

/// An answer as received.
pub struct Answer {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Bytes,
}

/// At most `limit` connections open at once, idle ones kept for the next request.
pub struct Pool {
    tls: Tls,
    idle: Mutex<Vec<SendRequest<Full<Bytes>>>>,
    permits: Semaphore,
}

impl Pool {
    /// A pool of at most `limit` connections to the node `tls` connects to.
    pub fn new(tls: Tls, limit: usize) -> Self {
        Self { tls, idle: Mutex::new(Vec::with_capacity(limit)), permits: Semaphore::new(limit) }
    }

    /// Sends one request on an idle connection, or a new one, and reads its answer whole. A reused
    /// connection the node had closed is replaced, once, when the request never left.
    /// Error cases:
    /// - the connection or the exchange fails, or the body is over `MAX_ANSWER_BYTES`
    pub async fn exchange(
        &self,
        method: &Method,
        path: &str,
        body: &Bytes,
        challenge: &[u8; 32],
    ) -> anyhow::Result<Answer> {
        let _permit = self.permits.acquire().await?;
        let mut reused = true;
        loop {
            let idle = self.idle.lock().unwrap_or_else(PoisonError::into_inner).pop();
            let mut sender = match idle {
                Some(sender) => sender,
                None => {
                    reused = false;
                    self.connect().await?
                }
            };
            if sender.ready().await.is_err() {
                continue;
            }
            let request = Request::builder()
                .method(method)
                .uri(path)
                .header(HOST, self.tls.url().authority())
                .header(CONTENT_TYPE, "application/json")
                .header(engine_wire::CHALLENGE_HEADER, hex::encode(challenge))
                .body(Full::new(body.clone()))?;
            let response = match sender.send_request(request).await {
                Ok(response) => response,
                Err(e) if reused && e.is_canceled() => {
                    reused = false;
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            let (parts, body) = response.into_parts();
            let body = Limited::new(body, MAX_ANSWER_BYTES).collect().await.map_err(|e| anyhow!(e))?.to_bytes();
            if !sender.is_closed() {
                self.idle.lock().unwrap_or_else(PoisonError::into_inner).push(sender);
            }
            return Ok(Answer { status: parts.status.as_u16(), headers: parts.headers, body });
        }
    }

    /// A new connection, driven by a task that ends when the connection does.
    /// Error cases:
    /// - the connection or the handshake fails
    async fn connect(&self) -> anyhow::Result<SendRequest<Full<Bytes>>> {
        let stream = self.tls.connect().await?;
        let (sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
        tokio::spawn(connection);
        Ok(sender)
    }
}
