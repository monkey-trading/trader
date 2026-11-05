//! The bot's HTTP/1.1 connections to the node: a bounded set kept open between requests, so a
//! thousand accounts share a handful of TLS connections rather than each call opening one.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use anyhow::anyhow;
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::client::conn::http1::SendRequest;
use hyper::header::{CONTENT_TYPE, HOST};
use hyper::{HeaderMap, Method, Request};
use hyper_util::rt::TokioIo;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::timeout;

use crate::node::Tls;

/// The longest a connection made in the background may take; a request's own time is the config's
/// `request_timeout`.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The largest answer body the bot reads: a page of marks history is a few hundred kilobytes.
const MAX_ANSWER_BYTES: usize = 4 << 20;

/// An answer as received.
pub struct Answer {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Bytes,
}

type Sender = SendRequest<Full<Bytes>>;

/// At most `limit` connections open at once, idle ones kept for the next request.
pub struct Pool {
    tls: Tls,
    idle: Mutex<Vec<Sender>>,
    permits: Arc<Semaphore>,
    /// The connections reads may hold at once: all but one, so that an order never waits behind
    /// reads for a connection when the pool has more than one.
    reads: Arc<Semaphore>,
    /// Whether a connection is being made for the idle set.
    filling: AtomicBool,
}

/// The room for one request in the pool: a permit, and for a read one of the read slots too.
pub struct Slot {
    _read: Option<OwnedSemaphorePermit>,
    _conn: OwnedSemaphorePermit,
}

/// A connection ready to send one request, with its slot: what an order is signed against. If it is
/// dropped unused the connection goes back to the idle set.
pub struct Checkout {
    _slot: Slot,
    sender: Option<Sender>,
    pool: Arc<Pool>,
}

impl Drop for Checkout {
    /// Gives an unused connection back.
    fn drop(&mut self) {
        if let Some(sender) = self.sender.take() {
            self.pool.give_back(sender);
        }
    }
}

impl Pool {
    /// A pool of at most `limit` connections to the node `tls` connects to.
    pub fn new(tls: Tls, limit: usize) -> Arc<Self> {
        Arc::new(Self {
            tls,
            idle: Mutex::new(Vec::with_capacity(limit)),
            permits: Arc::new(Semaphore::new(limit)),
            reads: Arc::new(Semaphore::new(limit.saturating_sub(1).max(1))),
            filling: AtomicBool::new(false),
        })
    }

    /// A free connection, ready to send, with its slot, for a request that must go at once. When none
    /// is ready a connection is made in the background for the next ask.
    pub fn try_checkout(self: &Arc<Self>) -> Option<Checkout> {
        let conn = Arc::clone(&self.permits).try_acquire_owned().ok()?;
        let Some(sender) = self.pop_ready() else {
            // The slot is held while the connection is made, so no read connects beside it past the limit.
            self.fill(conn);
            return None;
        };
        Some(Checkout { _slot: Slot { _read: None, _conn: conn }, sender: Some(sender), pool: Arc::clone(self) })
    }

    /// The room for a read, once there is one.
    /// Error cases:
    /// - the pool is closed
    pub async fn read_slot(&self) -> anyhow::Result<Slot> {
        let read = Arc::clone(&self.reads).acquire_owned().await?;
        let conn = Arc::clone(&self.permits).acquire_owned().await?;
        Ok(Slot { _read: Some(read), _conn: conn })
    }

    /// Sends one request in `slot` and reads its answer whole: on `sender` when the caller has a ready
    /// one, else on an idle connection or a new one. A reused connection the node had closed is
    /// replaced, once, when the request never left, unless the caller's own was ready.
    /// Error cases:
    /// - the connection or the exchange fails, or the body is over `MAX_ANSWER_BYTES`
    pub async fn exchange(
        &self,
        slot: Option<Slot>,
        mut checkout: Option<Checkout>,
        method: &Method,
        path: &str,
        body: &Bytes,
        challenge: &[u8; 32],
    ) -> anyhow::Result<Answer> {
        let _slot = slot;
        let mut given = checkout.as_mut().and_then(|c| c.sender.take());
        let mut reused = given.is_none();
        loop {
            let mut sender = match given.take() {
                Some(sender) => sender,
                None => match self.pop_ready() {
                    Some(sender) => sender,
                    None => {
                        reused = false;
                        self.connect().await?
                    }
                },
            };
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
            self.give_back(sender);
            return Ok(Answer { status: parts.status.as_u16(), headers: parts.headers, body });
        }
    }

    /// An idle connection that can send now.
    fn pop_ready(&self) -> Option<Sender> {
        let mut idle = self.idle.lock().unwrap_or_else(PoisonError::into_inner);
        while let Some(sender) = idle.pop() {
            if sender.is_ready() {
                return Some(sender);
            }
        }
        None
    }

    /// Keeps `sender` for the next request if the node has not closed it.
    fn give_back(&self, sender: Sender) {
        if !sender.is_closed() {
            self.idle.lock().unwrap_or_else(PoisonError::into_inner).push(sender);
        }
    }

    /// Makes one connection for the idle set in the background, unless one is being made.
    fn fill(self: &Arc<Self>, conn: OwnedSemaphorePermit) {
        if self.filling.swap(true, Ordering::AcqRel) {
            return;
        }
        let pool = Arc::clone(self);
        tokio::spawn(async move {
            let _conn = conn;
            if let Ok(Ok(sender)) = timeout(CONNECT_TIMEOUT, pool.connect()).await {
                pool.give_back(sender);
            }
            pool.filling.store(false, Ordering::Release);
        });
    }

    /// A new connection, driven by a task that ends when the connection does.
    /// Error cases:
    /// - the connection or the handshake fails
    async fn connect(&self) -> anyhow::Result<Sender> {
        let stream = self.tls.connect().await?;
        let (sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
        tokio::spawn(connection);
        Ok(sender)
    }
}
