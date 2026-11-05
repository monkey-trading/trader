//! The node's REST API. Every call carries a fresh challenge, and its answer counts only once the
//! enclave key's signature over it, the request, and that challenge verifies (`engine_wire`). An
//! answer that does not verify is an error, never data. Calls share a small pool of connections and
//! are paced under the node's limits (`pace`).

pub mod accounts;
pub mod api;
mod pace;
mod pool;
mod tls;
pub use tls::Tls;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, anyhow};
use bytes::Bytes;
use engine_core::types::AccountId;
use engine_wire::{
    CHALLENGE_HEADER, Deployment, Error as WireError, Received, ReceivedError, SEQ_HEADER, SIGNATURE_HEADER,
    SentRequest, TIME_HEADER, verify_received,
};
use hyper::Method;
use hyper::header::RETRY_AFTER;
use rand::RngCore;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::time::{Instant, sleep, timeout};

use crate::config::LimitsSection;
use crate::node::pace::{NodeClock, Pacer};
use crate::node::pool::{Checkout, Pool};

/// How many times a read the node asks to retry later is sent again before it fails.
const READ_RETRIES: u32 = 3;

/// The pacer's leave for one request to go now, and the connection it will go on. A request that acts
/// for an account is signed only once it has one, and sent at once, so what it says of the node's time
/// is as late as it can be.
#[must_use]
pub struct Admission(Checkout);

/// How long to wait before asking again for a connection that every request in flight holds.
const NO_CONNECTION_WAIT: Duration = Duration::from_millis(50);

/// A node, its signer, and the deployment its signatures name.
pub struct Node {
    pool: Arc<Pool>,
    signer: [u8; 20],
    deployment: Deployment,
    timeout: Duration,
    pacer: Pacer,
    clock: NodeClock,
    requests: AtomicU64,
    retry_later: AtomicU64,
}

/// A verified answer: its body, and the node's time and log head it was signed at.
#[derive(Debug)]
pub struct Verified<T> {
    pub body: T,
    pub time_ms: u64,
    pub head_seq: u64,
}

/// Why a call has no answer to act on.
#[derive(Debug, thiserror::Error)]
pub enum CallError {
    /// The answer's signature is not the configured signer's over it: another key signed it, or it
    /// is not the answer that was signed. `engine_wire` cannot tell the two apart, and over a
    /// pinned TLS connection neither passes by chance.
    #[error("{target}: the answer is not signed by the configured signer")]
    WrongSigner { target: String },
    /// The answer is unsigned, malformed, or not for this request.
    #[error("{target}: the answer does not verify: {source}")]
    Unverified { target: String, source: ReceivedError },
    /// A verified refusal: the node's status and its answer.
    #[error("{target}: refused with {status}: {error}")]
    Refused { target: String, status: u16, error: String },
    /// The node asked the bot to try again later (429 or 503): every call waits `wait`. The answer
    /// may be unsigned, as the node's per-address limit answers before it signs, so it is a delay
    /// and nothing more, and the request may still have reached the node.
    #[error("{target}: the node asks to try again in {wait:?}")]
    RetryLater { target: String, wait: Duration },
    /// No answer: the connection, the timeout, or a body that is not the expected JSON.
    #[error("{target}: {source:#}")]
    Failed { target: String, source: anyhow::Error },
}

impl Node {
    /// The node `tls` connects to, whose answers `signer` signs for `deployment`, paced under
    /// `limits`.
    pub fn new(tls: Tls, signer: [u8; 20], deployment: Deployment, timeout: Duration, limits: &LimitsSection) -> Self {
        Self {
            pool: Pool::new(tls, limits.connections),
            signer,
            deployment,
            timeout,
            pacer: Pacer::new(limits, Instant::now()),
            clock: NodeClock::default(),
            requests: AtomicU64::new(0),
            retry_later: AtomicU64::new(0),
        }
    }

    /// Leave for a request that acts for `account` to go now.
    /// Error cases:
    /// - the node's limits or a retry-later hold allow it later, or every connection is busy: the time
    ///   to wait before asking again
    pub fn try_admit(&self, account: Option<AccountId>) -> Result<Admission, Duration> {
        let checkout = self.pool.try_checkout().ok_or(NO_CONNECTION_WAIT)?;
        // A request the limits do not let go gives its connection back as the checkout drops.
        self.pacer.try_reserve(account, Instant::now())?;
        Ok(Admission(checkout))
    }

    /// Waits until the node's limits allow a request for `account` and a connection is free, and takes
    /// leave for it. A hold the node asks for during the wait is waited out too.
    pub async fn admit(&self, account: Option<AccountId>) -> Admission {
        loop {
            match self.try_admit(account) {
                Ok(admission) => return admission,
                Err(wait) => sleep(wait).await,
            }
        }
    }

    /// Waits until the node's limits allow a request for `account`.
    async fn pace(&self, account: Option<AccountId>) {
        while let Err(wait) = self.pacer.try_reserve(account, Instant::now()) {
            sleep(wait).await;
        }
    }

    /// The node's time now, from an answer to a request sent at or after `not_before`.
    pub fn time_ms(&self, not_before: Instant) -> Option<u64> {
        self.clock.now_ms(Instant::now(), not_before)
    }

    /// The node's time now, from the latest answer whenever it was sent.
    pub fn latest_time_ms(&self) -> Option<u64> {
        self.clock.latest_ms(Instant::now())
    }

    /// The longest an answer can take, which bounds how far the node's time counted on from an answer
    /// can lead the node's own.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Waits for a declaration's turn under the node's fresh-account budget for this client.
    pub async fn setup_turn(&self) {
        while let Err(wait) = self.pacer.try_reserve_setup(Instant::now()) {
            sleep(wait).await;
        }
    }

    /// How many requests were sent, and how many of them the node asked the bot to retry later.
    pub fn counts(&self) -> (u64, u64) {
        (self.requests.load(Ordering::Relaxed), self.retry_later.load(Ordering::Relaxed))
    }

    /// `GET path_and_query`, verified; sent again after a retry-later answer, up to `READ_RETRIES` times.
    /// Error cases:
    /// - see `CallError`
    pub async fn get<T: DeserializeOwned>(&self, path_and_query: &str) -> Result<Verified<T>, CallError> {
        let mut retries = 0;
        loop {
            match self.call(Method::GET, path_and_query, Bytes::new(), None, None).await {
                // Reading again changes nothing, and the pacer holds the next try back for the delay.
                Err(CallError::RetryLater { .. }) if retries < READ_RETRIES => retries += 1,
                other => return other,
            }
        }
    }

    /// `POST path` of `body` as JSON, verified; `account` when the request acts for one, which
    /// counts against its limit.
    /// Error cases:
    /// - see `CallError`
    pub async fn post<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
        account: Option<AccountId>,
    ) -> Result<Verified<T>, CallError> {
        self.post_admitted(path, body, account, None).await
    }

    /// `POST path` like `post`, with the leave `admission` gives when the caller has it already.
    /// Error cases:
    /// - see `CallError`
    pub async fn post_admitted<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
        account: Option<AccountId>,
        admission: Option<Admission>,
    ) -> Result<Verified<T>, CallError> {
        let body = serde_json::to_vec(body)
            .map_err(|e| CallError::Failed { target: target("POST", path), source: e.into() })?;
        self.call(Method::POST, path, Bytes::from(body), account, admission).await
    }

    /// One call under a fresh challenge, once the pacer lets it go: sends it, verifies the answer
    /// against the challenge and the request as sent, and parses a verified 200 body as `T`. A 429
    /// or 503 holds every later call back for the delay it names.
    /// Error cases:
    /// - see `CallError`
    async fn call<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Bytes,
        account: Option<AccountId>,
        admission: Option<Admission>,
    ) -> Result<Verified<T>, CallError> {
        let target = target(method.as_str(), path);
        let mut challenge = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut challenge);
        let (slot, checkout) = match admission {
            Some(Admission(checkout)) => (None, Some(checkout)),
            // A read waits for its room first, and then for the limits, so that a hold the node asks
            // for while it waits reaches it, and the wait is not counted in the time it may take.
            None => {
                let slot = self
                    .pool
                    .read_slot()
                    .await
                    .map_err(|source| CallError::Failed { target: target.clone(), source })?;
                self.pace(account).await;
                (Some(slot), None)
            }
        };
        let sent = Instant::now();
        self.requests.fetch_add(1, Ordering::Relaxed);
        let answer = timeout(self.timeout, self.pool.exchange(slot, checkout, &method, path, &body, &challenge))
            .await
            .map_err(|_| anyhow!("no answer within {:?}", self.timeout))
            .and_then(|answer| answer)
            .map_err(|source| CallError::Failed { target: target.clone(), source })?;
        if matches!(answer.status, 429 | 503) {
            let wait = retry_after(&answer.headers);
            self.retry_later.fetch_add(1, Ordering::Relaxed);
            self.pacer.back_off(wait, Instant::now());
            return Err(CallError::RetryLater { target, wait });
        }
        let header = |name: &str| answer.headers.get(name).and_then(|value| value.to_str().ok());
        let received = Received {
            status: answer.status,
            signature: header(SIGNATURE_HEADER),
            time: header(TIME_HEADER),
            seq: header(SEQ_HEADER),
            challenge: header(CHALLENGE_HEADER),
            body: &answer.body,
        };
        let request = SentRequest { method: method.as_str(), path_and_query: path, body: &body };
        let verified = match verify_received(&self.signer, &self.deployment, &challenge, &request, &received) {
            Ok(verified) => verified,
            Err(ReceivedError::Signature(WireError::WrongSigner)) => return Err(CallError::WrongSigner { target }),
            Err(source) => return Err(CallError::Unverified { target, source }),
        };
        self.clock.observe(verified.time_ms, sent);
        if answer.status != 200 {
            let error = String::from_utf8_lossy(&answer.body).into_owned();
            return Err(CallError::Refused { target, status: answer.status, error });
        }
        let body = serde_json::from_slice(&answer.body)
            .with_context(|| format!("unexpected answer: {}", String::from_utf8_lossy(&answer.body)))
            .map_err(|source| CallError::Failed { target, source })?;
        Ok(Verified { body, time_ms: verified.time_ms, head_seq: verified.head_seq })
    }
}

/// The delay a `Retry-After` header names, in whole seconds, at least one second.
fn retry_after(headers: &hyper::HeaderMap) -> Duration {
    let secs = headers.get(RETRY_AFTER).and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<u64>().ok());
    Duration::from_secs(secs.unwrap_or(1).max(1))
}

/// `METHOD path`, as errors name a call.
fn target(method: &str, path: &str) -> String {
    format!("{method} {path}")
}
