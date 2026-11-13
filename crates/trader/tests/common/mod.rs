//! A local stand-in for the node: its API over TLS with a self-signed certificate, signing every
//! answer and stream frame as the node does (`engine_wire`), with the faults a test injects.

#![allow(dead_code, unused_imports)]

mod bot;
pub mod console;
mod controls;
mod messages;
mod reads;
pub use bot::*;
pub use controls::*;
pub use messages::*;
pub mod lifecycle;
pub use reads::{emissions_answer, history_fill};

use std::collections::{BTreeMap, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::body::{Body, Bytes, to_bytes};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, Request, State};
use axum::http::{HeaderValue, StatusCode, Uri};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use engine_core::eip712::{self, B256};
use engine_wire::{Connection, Deployment, FRAME_VERSION, SignedFrame, frame_digest, parse_challenge, response_digest};
use futures_util::{SinkExt, StreamExt};
use hyper_util::rt::TokioIo;
use hyper_util::service::TowerToHyperService;
use k256::ecdsa::SigningKey;
use rcgen::PublicKeyData;
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio_rustls::TlsAcceptor;

pub const CHAIN_ID: u64 = 31337;
pub const VAULT: [u8; 20] = [0; 20];
/// The stand-in enclave key's secret, and another key's.
const SIGNER_SECRET: [u8; 32] = [0x51; 32];
const OTHER_SECRET: [u8; 32] = [0x52; 32];
/// A test account's secret: obviously fake.
pub const ACCOUNT_SECRET: [u8; 32] = [0x11; 32];

/// How the stand-in answers REST calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answers {
    Signed,
    /// Signed by another key than the signer.
    OtherSigner,
    /// Signed, then the body altered.
    Tampered,
    Unsigned,
    /// Signed, but echoing another challenge than the one sent.
    OtherChallenge,
}

/// A fault a stream connection injects in place of a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// The frame's signature is over another message.
    BadSignature,
    /// The frame carries the place after its own.
    OutOfSequence,
    /// The frame is signed by another key.
    OtherSigner,
    /// The message is skipped, leaving a gap.
    Skip,
    /// The connection goes silent instead.
    Silence,
}

/// What the stand-in holds and has seen.
#[derive(Default)]
pub struct Inner {
    pub answers: Option<Answers>,
    /// A request path (or its end) whose answers another key signs, the rest staying the signer's.
    pub other_signer_path: Option<String>,
    pub time_ms: u64,
    /// How far the log head moves after each answer.
    pub head_step: u64,
    /// The time stream frames are signed with, when it is not `time_ms`.
    pub frame_time: Option<u64>,
    pub head_seq: u64,
    pub markets: Value,
    /// Account views by lower-case address; an absent one answers 404.
    pub accounts: BTreeMap<String, Value>,
    /// The nonce an account starts at; each POST for an account moves its own on by one.
    pub nonce: u64,
    pub nonces: BTreeMap<String, u64>,
    /// A per-address limit as the node applies it, answering 429 with a `Retry-After` of one second,
    /// unsigned, to a request over it: requests per second, and the burst.
    pub limit: Option<(f64, f64)>,
    pub tokens: Option<(f64, std::time::Instant)>,
    /// How many requests the limit refused, and how many TLS connections the stand-in accepted.
    pub rate_limited: usize,
    pub tls_connections: usize,
    /// The marks history, served by window and page as the node serves it.
    pub marks: Vec<Value>,
    /// The most buckets one page holds, below the request's limit; and a `next` to answer with
    /// instead of the right one.
    pub marks_page: Option<usize>,
    pub marks_next: Option<i64>,
    /// Every POST, by path, with its body.
    pub posts: Vec<(String, Value)>,
    /// The events each POST answers with, in turn; `[]` once none are left.
    pub replies: VecDeque<Value>,
    /// The status each POST answers with, in turn, before the replies; 200 once none are left. A
    /// status other than 200 answers `{"error": "refused"}`.
    pub statuses: VecDeque<u16>,
    /// How long each POST waits before it answers.
    pub post_delay: Option<std::time::Duration>,
    /// The log: offsets with their messages.
    pub log: Vec<((u64, u32), String)>,
    /// Faults by the connection's place among those opened, from 0, and the message's offset.
    pub faults: BTreeMap<(usize, (u64, u32)), Fault>,
    /// The request target of each stream connection opened.
    pub connections: Vec<String>,
    /// The latest message the bot has applied: it answers the ping sent after a message only once
    /// it reads on past that message.
    pub applied: Option<(u64, u32)>,
    /// How many times an account, and its nonce, were read.
    pub account_reads: usize,
    pub nonce_reads: usize,
    /// How many times the markets, and the marks history, were read.
    pub market_reads: usize,
    pub marks_reads: usize,
    /// Whether a cancel-all clears the account's resting orders and a close-all its positions in
    /// the markets it names, as the engine's do when they succeed.
    pub settle: bool,
    /// The most units of a position one close-all takes (the engine's `Fixed::MAX_INPUT`); the rest stays open.
    pub close_cap: Option<engine_core::Fixed>,
    pub lifecycle: lifecycle::Canned,
    /// A position a settled cancel-all opens, by market id: a resting order that filled first.
    pub filled_before_cancel: Option<(String, Value)>,
    /// Each account's fill history, newest first, by lower-case address, and the most events a page
    /// of it holds, below the request's limit.
    pub history: BTreeMap<String, Vec<Value>>,
    pub history_page: Option<usize>,
    pub history_reads: usize,
    /// Each account's emissions answer, by lower-case address; an absent one answers 404.
    pub emissions: BTreeMap<String, Value>,
}

/// A running stand-in.
pub struct StandIn {
    pub inner: Mutex<Inner>,
    appended: Notify,
    signer: SigningKey,
    other: SigningKey,
    pub addr: SocketAddr,
    pub pin: [u8; 32],
}

impl StandIn {
    /// Starts a stand-in on a free loopback port, answering signed.
    pub async fn start() -> Arc<Self> {
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let pin: [u8; 32] = Sha256::digest(cert.signing_key.subject_public_key_info()).into();
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der()));
        let config = ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![CertificateDer::from(cert.cert.der().to_vec())], key)
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let stand_in = Arc::new(StandIn {
            inner: Mutex::new(Inner {
                answers: Some(Answers::Signed),
                time_ms: 1_800_000_000_000,
                settle: true,
                close_cap: None,
                ..Inner::default()
            }),
            appended: Notify::new(),
            signer: SigningKey::from_slice(&SIGNER_SECRET).unwrap(),
            other: SigningKey::from_slice(&OTHER_SECRET).unwrap(),
            addr,
            pin,
        });
        let router = lifecycle::routes()
            .route("/v1/markets", get(markets))
            .route("/v1/markets/{id}/marks", get(marks))
            .route("/v1/accounts/{address}", get(reads::account_view))
            .route("/v1/accounts/{address}/nonce", get(reads::nonce))
            .route("/v1/accounts/{address}/history", get(reads::history))
            .route("/v1/accounts/{address}/emissions", get(reads::emissions))
            .route("/v1/orders", post(record))
            .route("/v1/orders/cancel-all", post(record))
            .route("/v1/positions/close-all", post(record))
            .route("/v1/stream", get(stream))
            .layer(from_fn_with_state(stand_in.clone(), sign_answers))
            .layer(from_fn_with_state(stand_in.clone(), limit_ip))
            .with_state(stand_in.clone());
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let counted = Arc::clone(&stand_in);
        tokio::spawn(async move {
            loop {
                let Ok((tcp, _)) = listener.accept().await else { return };
                let (acceptor, router) = (acceptor.clone(), router.clone());
                let counted = Arc::clone(&counted);
                tokio::spawn(async move {
                    counted.with(|i| i.tls_connections += 1);
                    let Ok(tls) = acceptor.accept(tcp).await else { return };
                    let service = TowerToHyperService::new(router);
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(tls), service)
                        .with_upgrades()
                        .await;
                });
            }
        });
        stand_in
    }

    /// The enclave key's address.
    pub fn signer(&self) -> [u8; 20] {
        eip712::address(self.signer.verifying_key()).0
    }

    /// The deployment the stand-in signs for.
    pub fn deployment(&self) -> Deployment {
        Deployment { chain_id: CHAIN_ID, vault: VAULT }
    }

    /// Runs `edit` on the stand-in's state.
    pub fn with<T>(&self, edit: impl FnOnce(&mut Inner) -> T) -> T {
        edit(&mut self.inner.lock().unwrap())
    }

    /// Appends a `Tick` command at `ts` as the log's next seq, after the last message or the head;
    /// its offset.
    pub fn tick(&self, ts: i64) -> (u64, u32) {
        let seq = self.with(|i| i.log.last().map_or(i.head_seq, |((seq, _), _)| *seq) + 1);
        self.append((seq, 0), command(seq, ts, json!({ "type": "tick" }), json!([])));
        (seq, 0)
    }

    /// Waits until the bot has applied the message at `offset` and read on, so whatever applying
    /// it does is done.
    pub async fn applied(&self, offset: (u64, u32)) {
        for _ in 0..500 {
            if self.with(|i| i.applied >= Some(offset)) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("the bot did not read on past {offset:?}");
    }

    /// Appends a message at `offset` to the log, waking the open connections.
    pub fn append(&self, offset: (u64, u32), msg: Value) {
        self.with(|inner| inner.log.push((offset, msg.to_string())));
        self.appended.notify_waiters();
    }

    /// Signs `digest` with the signer, or with another key.
    fn sign(&self, digest: [u8; 32], other: bool) -> String {
        let key = if other { &self.other } else { &self.signer };
        format!("0x{}", hex::encode(eip712::sign(key, &B256::from(digest))))
    }
}

/// Refuses a request over the per-address limit, as the node does before it signs: 429, a
/// `Retry-After`, and no signature.
async fn limit_ip(State(s): State<Arc<StandIn>>, req: Request, next: Next) -> Response {
    if req.uri().path() != "/v1/stream" {
        let over = s.with(|i| {
            let Some((rate, burst)) = i.limit else { return false };
            let now = std::time::Instant::now();
            let (tokens, last) = i.tokens.unwrap_or((burst, now));
            let tokens = (tokens + now.duration_since(last).as_secs_f64() * rate).min(burst);
            let over = tokens < 1.0;
            i.tokens = Some((if over { tokens } else { tokens - 1.0 }, now));
            i.rate_limited += usize::from(over);
            over
        });
        if over {
            return (StatusCode::TOO_MANY_REQUESTS, [("retry-after", "1")], "rate limited").into_response();
        }
    }
    next.run(req).await
}

/// Signs each answer as the node does, or answers as `Inner::answers` says. The stream answers
/// unsigned, as the node's upgrade does.
async fn sign_answers(State(s): State<Arc<StandIn>>, req: Request, next: Next) -> Response {
    if req.uri().path() == "/v1/stream" {
        return next.run(req).await;
    }
    let challenge = req.headers().get("x-engine-challenge").and_then(|v| v.to_str().ok()).and_then(parse_challenge);
    let challenge = challenge.unwrap_or([0; 32]);
    let method = req.method().clone();
    let target = req.uri().path_and_query().unwrap().to_string();
    let (parts, body) = req.into_parts();
    let request_body = to_bytes(body, usize::MAX).await.unwrap();
    let response = next.run(Request::from_parts(parts, Body::from(request_body.clone()))).await;
    let (mut parts, body) = response.into_parts();
    let mut body = to_bytes(body, usize::MAX).await.unwrap();
    let (answers, time_ms, head_seq) = s.with(|i| {
        let head = i.head_seq;
        i.head_seq += i.head_step;
        (i.answers.unwrap(), i.time_ms, head)
    });
    if answers != Answers::Unsigned {
        let response = engine_wire::Response {
            challenge,
            time_ms,
            head_seq,
            status: parts.status.as_u16(),
            method: method.as_str(),
            path_and_query: &target,
            request_body: &request_body,
            body: &body,
        };
        let other = answers == Answers::OtherSigner
            || s.with(|i| i.other_signer_path.as_ref().is_some_and(|p| target.ends_with(p.as_str())));
        let signature = s.sign(response_digest(&s.deployment(), &response), other);
        let echoed = if answers == Answers::OtherChallenge { [9; 32] } else { challenge };
        let headers = &mut parts.headers;
        headers.insert("x-engine-signature", HeaderValue::from_str(&signature).unwrap());
        headers.insert("x-engine-time", HeaderValue::from(time_ms));
        headers.insert("x-engine-seq", HeaderValue::from(head_seq));
        headers.insert("x-engine-challenge", HeaderValue::from_str(&hex::encode(echoed)).unwrap());
        if answers == Answers::Tampered {
            body = Bytes::from(body.iter().copied().chain(*b" ").collect::<Vec<_>>());
            parts.headers.remove("content-length");
        }
    }
    Response::from_parts(parts, Body::from(body))
}

async fn markets(State(s): State<Arc<StandIn>>) -> Response {
    let markets = s.with(|i| {
        i.market_reads += 1;
        i.markets.clone()
    });
    axum::Json(markets).into_response()
}

async fn marks(State(s): State<Arc<StandIn>>, Query(query): Query<BTreeMap<String, String>>) -> Response {
    let number = |key: &str| query.get(key).and_then(|v| v.parse::<i64>().ok());
    let (from, to) = (number("from_ms").unwrap_or(i64::MIN), number("to_ms").unwrap_or(i64::MAX));
    let limit = number("limit").map_or(500, |limit| limit as usize);
    let (entries, next) = s.with(|i| {
        i.marks_reads += 1;
        let window: Vec<Value> =
            i.marks.iter().filter(|b| (from..=to).contains(&b["ts"].as_i64().unwrap())).cloned().collect();
        let page = limit.min(i.marks_page.unwrap_or(limit));
        let next = (window.len() > page).then(|| window[page - 1]["ts"].as_i64().unwrap() + 1);
        (window.into_iter().take(page).collect::<Vec<_>>(), i.marks_next.or(next))
    });
    axum::Json(json!({ "market": 1, "resolution": query.get("resolution"), "entries": entries, "next": next,
        "max_limit": 2000 }))
    .into_response()
}

/// Records a POST, settles a cancel-all or close-all when `Inner::settle` says so, and answers
/// with the next scripted status or events, after `Inner::post_delay`.
async fn record(State(s): State<Arc<StandIn>>, uri: Uri, body: Bytes) -> Response {
    let body: Value = serde_json::from_slice(&body).unwrap();
    // Recorded and settled at once, as a node sequences a request whose answer is then lost.
    let (status, reply, delay) = s.with(|i| {
        let status = i.statuses.pop_front();
        let account = body["account"].as_str().unwrap_or_default().to_lowercase();
        if let (true, None, Some(view)) = (i.settle, status, i.accounts.get_mut(&account)) {
            match uri.path() {
                "/v1/orders/cancel-all" => {
                    view["orders"] = json!({});
                    if let Some((market, position)) = i.filled_before_cancel.take() {
                        view["positions"][market] = position;
                    }
                }
                "/v1/positions/close-all" => {
                    for close in body["closes"].as_array().unwrap() {
                        let positions = view["positions"].as_object_mut().unwrap();
                        let market = close["market"].to_string();
                        let held: engine_core::Fixed = positions[&market]["qty"].as_str().unwrap().parse().unwrap();
                        match i.close_cap {
                            Some(cap) if held.abs() > cap => {
                                let left = if held.is_positive() { held - cap } else { held + cap };
                                positions[&market]["qty"] = json!(left.to_string());
                            }
                            _ => {
                                positions.remove(&market);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        let start = i.nonce;
        *i.nonces.entry(account).or_insert(start) += 1;
        i.posts.push((uri.path().to_owned(), body));
        let reply = if status.is_some() {
            json!({ "error": "refused" })
        } else {
            i.replies.pop_front().unwrap_or_else(|| json!([]))
        };
        (status, reply, i.post_delay)
    });
    if let Some(delay) = delay {
        tokio::time::sleep(delay).await;
    }
    let status = StatusCode::from_u16(status.unwrap_or(200)).unwrap();
    (status, axum::Json(reply)).into_response()
}

/// The log over a signed stream connection, from `from` or after `after`, then whatever is
/// appended, with each fault injected once.
async fn stream(State(s): State<Arc<StandIn>>, uri: Uri, ws: WebSocketUpgrade) -> Response {
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
