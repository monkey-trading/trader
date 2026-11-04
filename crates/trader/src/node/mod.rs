//! The node's REST API. Every call carries a fresh challenge, and its answer counts only once the
//! enclave key's signature over it, the request, and that challenge verifies (`engine_wire`). An
//! answer that does not verify is an error, never data.

pub mod api;
mod tls;
pub use tls::Tls;

use std::time::Duration;

use anyhow::Context;
use bytes::Bytes;
use engine_wire::{
    CHALLENGE_HEADER, Deployment, Error as WireError, Received, ReceivedError, SEQ_HEADER, SIGNATURE_HEADER,
    SentRequest, TIME_HEADER, verify_received,
};
use http_body_util::{BodyExt, Full, Limited};
use hyper::header::{CONTENT_TYPE, HOST};
use hyper::{Method, Request};
use hyper_util::rt::TokioIo;
use rand::RngCore;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::time::timeout;

/// The largest answer body the bot reads: a page of marks history is a few hundred kilobytes.
const MAX_ANSWER_BYTES: usize = 4 << 20;

/// A node, its signer, and the deployment its signatures name.
pub struct Node {
    tls: Tls,
    signer: [u8; 20],
    deployment: Deployment,
    timeout: Duration,
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
    /// No answer: the connection, the timeout, or a body that is not the expected JSON.
    #[error("{target}: {source:#}")]
    Failed { target: String, source: anyhow::Error },
}

impl Node {
    /// The node `tls` connects to, whose answers `signer` signs for `deployment`.
    pub fn new(tls: Tls, signer: [u8; 20], deployment: Deployment, timeout: Duration) -> Self {
        Self { tls, signer, deployment, timeout }
    }

    /// The connections this node's calls use.
    pub fn tls(&self) -> &Tls {
        &self.tls
    }

    /// The configured signer.
    pub fn signer(&self) -> [u8; 20] {
        self.signer
    }

    /// The deployment the node's signatures name.
    pub fn deployment(&self) -> Deployment {
        self.deployment
    }

    /// `GET path_and_query`, verified.
    /// Error cases:
    /// - see `CallError`
    pub async fn get<T: DeserializeOwned>(&self, path_and_query: &str) -> Result<Verified<T>, CallError> {
        self.call(Method::GET, path_and_query, Bytes::new()).await
    }

    /// `POST path` of `body` as JSON, verified.
    /// Error cases:
    /// - see `CallError`
    pub async fn post<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<Verified<T>, CallError> {
        let body = serde_json::to_vec(body)
            .map_err(|e| CallError::Failed { target: target("POST", path), source: e.into() })?;
        self.call(Method::POST, path, Bytes::from(body)).await
    }

    /// One call under a fresh challenge: sends it, verifies the answer against the challenge and the
    /// request as sent, and parses a verified 200 body as `T`.
    /// Error cases:
    /// - see `CallError`
    async fn call<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Bytes,
    ) -> Result<Verified<T>, CallError> {
        let target = target(method.as_str(), path);
        let mut challenge = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut challenge);
        let answer = timeout(self.timeout, self.exchange(&method, path, &body, &challenge))
            .await
            .map_err(|_| anyhow::anyhow!("no answer within {:?}", self.timeout))
            .and_then(|answer| answer)
            .map_err(|source| CallError::Failed { target: target.clone(), source })?;
        let header = |name: &str| answer.headers.get(name).and_then(|value| value.to_str().ok());
        let received = Received {
            status: answer.status,
            signature: header(SIGNATURE_HEADER),
            time: header(TIME_HEADER),
            seq: header(SEQ_HEADER),
            challenge: header(CHALLENGE_HEADER),
            body: &answer.body,
        };
        let sent = SentRequest { method: method.as_str(), path_and_query: path, body: &body };
        let verified = match verify_received(&self.signer, &self.deployment, &challenge, &sent, &received) {
            Ok(verified) => verified,
            Err(ReceivedError::Signature(WireError::WrongSigner)) => return Err(CallError::WrongSigner { target }),
            Err(source) => return Err(CallError::Unverified { target, source }),
        };
        if answer.status != 200 {
            let error = String::from_utf8_lossy(&answer.body).into_owned();
            return Err(CallError::Refused { target, status: answer.status, error });
        }
        let body = serde_json::from_slice(&answer.body)
            .with_context(|| format!("unexpected answer: {}", String::from_utf8_lossy(&answer.body)))
            .map_err(|source| CallError::Failed { target, source })?;
        Ok(Verified { body, time_ms: verified.time_ms, head_seq: verified.head_seq })
    }

    /// Sends one HTTP/1.1 request on a new connection and reads its answer whole.
    /// Error cases:
    /// - the connection or the exchange fails, or the body is over `MAX_ANSWER_BYTES`
    async fn exchange(
        &self,
        method: &Method,
        path: &str,
        body: &Bytes,
        challenge: &[u8; 32],
    ) -> anyhow::Result<Answer> {
        let stream = self.tls.connect().await?;
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header(HOST, self.tls.url().authority())
            .header(CONTENT_TYPE, "application/json")
            .header(CHALLENGE_HEADER, hex::encode(challenge))
            .body(Full::new(body.clone()))?;
        // The connection closes once the answer is read and `sender` is dropped.
        let answer = async move {
            let response = sender.send_request(request).await?;
            let (parts, body) = response.into_parts();
            let body = Limited::new(body, MAX_ANSWER_BYTES).collect().await.map_err(|e| anyhow::anyhow!(e))?;
            anyhow::Ok(Answer { status: parts.status.as_u16(), headers: parts.headers, body: body.to_bytes() })
        };
        let (answer, _) = tokio::join!(answer, connection);
        answer
    }
}

/// An answer as received.
struct Answer {
    status: u16,
    headers: hyper::HeaderMap,
    body: Bytes,
}

/// `METHOD path`, as errors name a call.
fn target(method: &str, path: &str) -> String {
    format!("{method} {path}")
}
