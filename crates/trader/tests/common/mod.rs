//! A local stand-in for the node: its API over TLS with a self-signed certificate, signing every
//! answer and stream frame as the node does (`engine_wire`), with the faults a test injects.

#![allow(dead_code)]

use std::collections::{BTreeMap, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::{Body, Bytes, to_bytes};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Request, State};
use axum::http::{HeaderValue, StatusCode, Uri};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use engine_core::eip712::{self, B256};
use engine_core::types::AccountId;
use engine_wire::{Connection, Deployment, FRAME_VERSION, SignedFrame, frame_digest, parse_challenge, response_digest};
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
    pub time_ms: u64,
    /// The time stream frames are signed with, when it is not `time_ms`.
    pub frame_time: Option<u64>,
    pub head_seq: u64,
    pub markets: Value,
    /// Account views by lower-case address; an absent one answers 404.
    pub accounts: BTreeMap<String, Value>,
    pub nonce: u64,
    pub marks: Vec<Value>,
    /// Every POST, by path, with its body.
    pub posts: Vec<(String, Value)>,
    /// The events each POST answers with, in turn; `[]` once none are left.
    pub replies: VecDeque<Value>,
    /// The log: offsets with their messages.
    pub log: Vec<((u64, u32), String)>,
    /// Faults by the connection's place among those opened, from 0, and the message's offset.
    pub faults: BTreeMap<(usize, (u64, u32)), Fault>,
    /// The request target of each stream connection opened.
    pub connections: Vec<String>,
    /// How many times an account, and its nonce, were read.
    pub account_reads: usize,
    pub nonce_reads: usize,
    /// How many times the markets were read.
    pub market_reads: usize,
    /// Whether a cancel-all clears the account's resting orders and a close-all its positions in
    /// the markets it names, as the engine's do when they succeed.
    pub settle: bool,
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
                ..Inner::default()
            }),
            appended: Notify::new(),
            signer: SigningKey::from_slice(&SIGNER_SECRET).unwrap(),
            other: SigningKey::from_slice(&OTHER_SECRET).unwrap(),
            addr,
            pin,
        });
        let router = Router::new()
            .route("/v1/markets", get(markets))
            .route("/v1/markets/{id}/marks", get(marks))
            .route("/v1/accounts/{address}", get(account_view_route))
            .route("/v1/accounts/{address}/nonce", get(nonce))
            .route("/v1/orders", post(record))
            .route("/v1/orders/cancel-all", post(record))
            .route("/v1/positions/close-all", post(record))
            .route("/v1/stream", get(stream))
            .layer(from_fn_with_state(stand_in.clone(), sign_answers))
            .with_state(stand_in.clone());
        let acceptor = TlsAcceptor::from(Arc::new(config));
        tokio::spawn(async move {
            loop {
                let Ok((tcp, _)) = listener.accept().await else { return };
                let (acceptor, router) = (acceptor.clone(), router.clone());
                tokio::spawn(async move {
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

/// A command message of the log, as the node streams it.
pub fn command(seq: u64, ts: i64, cmd: Value, events: Value) -> Value {
    json!({ "offset": format!("{seq}:0"), "type": "command", "seq": seq, "ts": ts, "cmd": cmd, "events": events })
}

/// A `Prices` command for `market` quoting `mid` from three venues stamped `ts`.
pub fn prices(market: u32, mid: &str, ts: i64) -> Value {
    let quote = |source: &str| json!({ "source": source, "mid": mid, "ts": ts });
    json!({ "type": "prices", "market": market, "quotes": [quote("a"), quote("b"), quote("c")] })
}

/// BTC-USD as the dev config lists it, with its mark.
pub fn btc_usd(mark: Option<&str>) -> Value {
    json!({
        "id": 1, "symbol": "BTC-USD", "asset_class": "crypto", "schedule": { "kind": "always_open" },
        "margin_tiers": [{ "bps": 100 }], "spread_tiers": [{ "bps": 0 }],
        "price_rules": { "max_age_ms": 1000, "max_divergence_bps": 25, "min_sources": 2 },
        "max_slippage_bps": 25, "financing": { "benchmark": "SOFR", "markup_bps": 500, "day_basis": 365 },
        "house_cap_bps": 3000, "house_cap_floor": "300000",
        "open": true, "reduce_only": false, "mark": mark, "fx_mark": null, "stress_level": 0
    })
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
    let (answers, time_ms, head_seq) = s.with(|i| (i.answers.unwrap(), i.time_ms, i.head_seq));
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
        let signature = s.sign(response_digest(&s.deployment(), &response), answers == Answers::OtherSigner);
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

async fn marks(State(s): State<Arc<StandIn>>) -> Response {
    let entries = s.with(|i| i.marks.clone());
    axum::Json(json!({ "market": 1, "resolution": "1m", "entries": entries, "next": null, "max_limit": 2000 }))
        .into_response()
}

async fn account_view_route(State(s): State<Arc<StandIn>>, Path(address): Path<String>) -> Response {
    let view = s.with(|i| {
        i.account_reads += 1;
        i.accounts.get(&address.to_lowercase()).cloned()
    });
    match view {
        Some(view) => axum::Json(view).into_response(),
        None => (StatusCode::NOT_FOUND, axum::Json(json!({ "error": "not found" }))).into_response(),
    }
}

async fn nonce(State(s): State<Arc<StandIn>>) -> Response {
    let nonce = s.with(|i| {
        i.nonce_reads += 1;
        i.nonce
    });
    axum::Json(json!({ "next_nonce": nonce })).into_response()
}

/// Records a POST, settles a cancel-all or close-all when `Inner::settle` says so, and answers
/// with the next scripted events.
async fn record(State(s): State<Arc<StandIn>>, uri: Uri, body: Bytes) -> Response {
    let body: Value = serde_json::from_slice(&body).unwrap();
    let reply = s.with(|i| {
        let account = body["account"].as_str().unwrap_or_default().to_lowercase();
        if let (true, Some(view)) = (i.settle, i.accounts.get_mut(&account)) {
            match uri.path() {
                "/v1/orders/cancel-all" => view["orders"] = json!({}),
                "/v1/positions/close-all" => {
                    for close in body["closes"].as_array().unwrap() {
                        view["positions"].as_object_mut().unwrap().remove(&close["market"].to_string());
                    }
                }
                _ => {}
            }
        }
        i.posts.push((uri.path().to_owned(), body));
        i.nonce += 1;
        i.replies.pop_front().unwrap_or_else(|| json!([]))
    });
    axum::Json(reply).into_response()
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
/// connection at `place`.
async fn serve_log(s: Arc<StandIn>, mut socket: WebSocket, connection: Connection, start: (u64, u32), place: usize) {
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
        let Some((_, msg, fault, time)) = entry else {
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
    }
}

// ── The bot against the stand-in ──

/// The stand-in's time at start: a minute boundary.
pub const T0: i64 = 1_800_000_000_000;
pub const MINUTE: i64 = 60_000;

/// The test account's address.
pub fn account() -> AccountId {
    eip712::address(SigningKey::from_slice(&ACCOUNT_SECRET).unwrap().verifying_key())
}

/// The domain the stand-in's deployment signs under.
pub fn domain() -> eip712::Eip712Domain {
    eip712::domain(CHAIN_ID, AccountId(VAULT))
}

/// A config for the stand-in, with the test account's key in a hex file in `dir`: BTC-USD, `bar`
/// bars, a one-bar average over a two-bar one, one unit a side, closing on stop.
pub fn config(stand_in: &StandIn, dir: &std::path::Path, bar: &str) -> trader::config::Config {
    std::fs::write(dir.join("account.hex"), hex::encode(ACCOUNT_SECRET)).unwrap();
    let text = format!(
        r#"
[node]
url = "https://127.0.0.1:{port}"
chain_id = {CHAIN_ID}
vault = "0x{vault}"
signer = "0x{signer}"
tls_key_sha256 = "{pin}"
stream_timeout = "30s"

[account]
key = {{ type = "hex_file", path = "account.hex" }}
close_on_stop = true

[strategy]
market = "BTC-USD"
bar = "{bar}"
fast = 1
slow = 2
qty = "1"
max_slippage_bps = 25
"#,
        port = stand_in.addr.port(),
        vault = hex::encode(VAULT),
        signer = hex::encode(stand_in.signer()),
        pin = hex::encode(stand_in.pin),
    );
    let path = dir.join("bot.toml");
    std::fs::write(&path, text).unwrap();
    trader::config::Config::load(&path).unwrap()
}

/// Runs the bot until `stop`.
pub async fn run(cfg: trader::config::Config, stop: impl Future<Output = ()> + Send + 'static) -> anyhow::Result<()> {
    trader::bot::run(cfg, |_| None, stop).await
}

/// The `n`th POST, from 1, once the stand-in has it.
pub async fn posted(stand_in: &StandIn, n: usize) -> (String, Value) {
    for _ in 0..200 {
        if let Some(post) = stand_in.with(|i| i.posts.get(n - 1).cloned()) {
            return post;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("no POST {n}: {:?}", stand_in.with(|i| i.posts.clone()));
}

/// An account view with `balance`, positions and resting orders.
pub fn account_view(balance: &str, positions: Value, orders: Value) -> Value {
    json!({ "balance": balance, "positions": positions, "orders": orders })
}

/// A resting limit order, as an account view lists it.
pub fn resting_order(order_id: u64) -> Value {
    json!({ "order_id": order_id, "client_id": 1, "market": 1, "side": "buy", "qty": "1",
        "kind": { "kind": "limit", "price": "50" }, "reduce_only": false })
}
