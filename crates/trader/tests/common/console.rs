//! The console under test: the bot and its console against the stand-in, and a client that speaks
//! HTTPS to it pinned to its key, with or without the session's cookie, under any `Origin`.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use bytes::Bytes;
use futures_util::StreamExt;
use http_body_util::{BodyExt, Full};
use hyper::header::{CONTENT_TYPE, COOKIE, HOST, ORIGIN};
use hyper::{HeaderMap, Method, Request};
use hyper_util::rt::TokioIo;
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use trader::config::{Config, NodeUrl, TlsPin};
use trader::node::Tls;

use super::{StandIn, config};

/// The console's own origin, as a browser on it sends it.
pub const SAME: Option<&str> = Some("same");
/// The longest a call to the console may take; a flatten's waits for the log are far shorter here.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// The console's period bounds in the configs these tests write, a line of the strategy's table.
pub const BOUNDS: &str = "bounds = { fast = [1, 5], slow = [2, 50] }\n";

/// A bot running with its console.
pub struct Running {
    pub addr: SocketAddr,
    /// What the console printed while it was prepared.
    pub printed: String,
    tls: Tls,
    stop: Option<oneshot::Sender<()>>,
    pub task: JoinHandle<anyhow::Result<()>>,
}

/// An answer from the console.
#[derive(Debug)]
pub struct Answer {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Value,
    pub text: String,
}

/// A config for the stand-in with a console on a free loopback port keeping its data in `dir`,
/// reading the node every second, with `strategy` added to the account's strategy.
pub fn console_config(stand_in: &StandIn, dir: &Path, strategy: &str) -> Config {
    Config::load(&write_config(stand_in, dir, strategy)).unwrap()
}

/// Writes that config's file, and its path.
fn write_config(stand_in: &StandIn, dir: &Path, strategy: &str) -> PathBuf {
    config(stand_in, dir, "1m");
    let path = dir.join("bot.toml");
    let mut text = std::fs::read_to_string(&path).unwrap();
    let end = text.find("max_slippage_bps = 25\n").expect("the strategy's table") + "max_slippage_bps = 25\n".len();
    text.insert_str(end, strategy);
    text.push_str("\n[console]\nbind = \"127.0.0.1:0\"\ndata_dir = \"data\"\nrefresh_interval = \"1s\"\n");
    std::fs::write(&path, text).unwrap();
    path
}

/// `console_config` with each `(from, to)` replacing the first `from` in the config's text.
pub fn console_config_edited(stand_in: &StandIn, dir: &Path, strategy: &str, edits: &[(&str, &str)]) -> Config {
    let path = write_config(stand_in, dir, strategy);
    let text = edits.iter().fold(std::fs::read_to_string(&path).unwrap(), |text, (from, to)| {
        assert!(text.contains(from), "{from:?} is not in the config");
        text.replacen(from, to, 1)
    });
    std::fs::write(&path, text).unwrap();
    Config::load(&path).unwrap()
}

/// `console_config`, with a strategy in ETH-USD too, for an account that holds a position there.
pub fn console_config_eth(stand_in: &StandIn, dir: &Path, strategy: &str) -> Config {
    let path = write_config(stand_in, dir, strategy);
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str(
        "\n[[account.strategy]]\nmarket = \"ETH-USD\"\nbar = \"1m\"\nkind = \"sma_crossover\"\nfast = 1\nslow = 2\n\
         max_slippage_bps = 25\n",
    );
    std::fs::write(&path, text).unwrap();
    Config::load(&path).unwrap()
}

/// The password the console printed on its first run.
pub fn printed_password(printed: &str) -> String {
    let line = printed.lines().find(|line| line.starts_with("console password")).expect("a password was printed");
    line.rsplit(": ").next().unwrap().to_owned()
}

impl Running {
    /// Prepares the console for `cfg` and runs the bot with it until `stop`.
    pub async fn start(cfg: Config) -> Self {
        let section = cfg.console.as_ref().unwrap();
        let data_dir: PathBuf = section.data_dir.clone();
        let mut out = Vec::new();
        let prepared = trader::console::prepare(section, &mut out).unwrap();
        let addr = prepared.local_addr().unwrap();
        let der = CertificateDer::pem_file_iter(data_dir.join("console.pem")).unwrap().next().unwrap().unwrap();
        let spki = webpki::EndEntityCert::try_from(&der).unwrap().subject_public_key_info().to_vec();
        let pin = TlsPin(Sha256::digest(&spki).into());
        let tls = Tls::new(NodeUrl { host: "127.0.0.1".into(), port: addr.port() }, Some(pin)).unwrap();
        let (stop, stopped) = oneshot::channel::<()>();
        let task = tokio::spawn(trader::serve(
            cfg,
            |_| None,
            async {
                _ = stopped.await;
            },
            Some(prepared),
        ));
        Self { addr, printed: String::from_utf8(out).unwrap(), tls, stop: Some(stop), task }
    }

    /// Moves the log past every order the bot could have signed, which its stop waits for, then
    /// stops it.
    pub async fn stop_after(self, stand_in: &StandIn) -> anyhow::Result<()> {
        stand_in.tick(super::T0 + 10 * super::MINUTE);
        self.stop().await
    }

    /// Asks the bot to stop, without waiting for it.
    pub fn signal_stop(&mut self) {
        self.stop.take().unwrap().send(()).unwrap();
    }

    /// Stops the bot and waits for it.
    pub async fn stop(mut self) -> anyhow::Result<()> {
        self.stop.take().unwrap().send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(60), self.task).await.unwrap().unwrap()
    }

    /// Logs in with `password`; the session's cookie on success.
    pub async fn login(&self, password: &str) -> (Answer, Option<String>) {
        let body = serde_json::json!({ "password": password });
        let answer = self.call(Method::POST, "/api/login", SAME, None, Some(body)).await;
        let cookie =
            answer.headers.get("set-cookie").map(|v| v.to_str().unwrap().split(';').next().unwrap().to_owned());
        (answer, cookie)
    }

    /// Logs in with the password it printed; the session's cookie.
    pub async fn session(&self) -> String {
        let (answer, cookie) = self.login(&printed_password(&self.printed)).await;
        assert_eq!(answer.status, 204, "{answer:?}");
        cookie.unwrap()
    }

    /// `method path` with `origin` (`SAME` for the console's own) and `cookie`, and a JSON body.
    pub async fn call(
        &self,
        method: Method,
        path: &str,
        origin: Option<&str>,
        cookie: Option<&str>,
        body: Option<Value>,
    ) -> Answer {
        let host = format!("127.0.0.1:{}", self.addr.port());
        let mut request = Request::builder().method(method).uri(path).header(HOST, &host);
        if let Some(origin) = origin {
            let origin = if Some(origin) == SAME { format!("https://{host}") } else { origin.to_owned() };
            request = request.header(ORIGIN, origin);
        }
        if let Some(cookie) = cookie {
            request = request.header(COOKIE, cookie);
        }
        let body = match body {
            Some(body) => {
                request = request.header(CONTENT_TYPE, "application/json");
                Bytes::from(body.to_string())
            }
            None => Bytes::new(),
        };
        let exchange = async {
            let stream = self.tls.connect().await.unwrap();
            let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream)).await.unwrap();
            tokio::spawn(connection);
            let response = sender.send_request(request.body(Full::new(body)).unwrap()).await.unwrap();
            let (parts, body) = response.into_parts();
            (parts, body.collect().await.unwrap().to_bytes())
        };
        let (parts, bytes) = tokio::time::timeout(CALL_TIMEOUT, exchange).await.expect("the console answers in time");
        let text = String::from_utf8_lossy(&bytes).into_owned();
        Answer {
            status: parts.status.as_u16(),
            headers: parts.headers,
            body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            text,
        }
    }

    /// Sends a login's headers and half its body, then nothing more: the answer as text, and how
    /// long it took.
    pub async fn stall_body(&self) -> (String, Duration) {
        let host = format!("127.0.0.1:{}", self.addr.port());
        let mut stream = self.tls.connect().await.unwrap();
        let head = format!(
            "POST /api/login HTTP/1.1\r\nhost: {host}\r\norigin: https://{host}\r\n\
             content-type: application/json\r\ncontent-length: 40\r\n\r\n{{\"password\":"
        );
        stream.write_all(head.as_bytes()).await.unwrap();
        stream.flush().await.unwrap();
        let sent = tokio::time::Instant::now();
        let mut answer = Vec::new();
        // The console closes the connection after the refusal.
        let read = tokio::time::timeout(CALL_TIMEOUT, stream.read_to_end(&mut answer)).await.expect("an answer");
        assert!(read.is_ok() || !answer.is_empty(), "{read:?}");
        (String::from_utf8_lossy(&answer).into_owned(), sent.elapsed())
    }

    /// A GET of `path` in the session `cookie`.
    pub async fn get(&self, path: &str, cookie: &str) -> Answer {
        self.call(Method::GET, path, None, Some(cookie), None).await
    }

    /// A POST of `path` in the session `cookie`, from the console's own origin.
    pub async fn post(&self, path: &str, cookie: &str) -> Answer {
        self.call(Method::POST, path, SAME, Some(cookie), None).await
    }

    /// Opens the live feed with `origin` and `cookie`; its messages, or the refusal's status.
    pub async fn live(&self, origin: Option<&str>, cookie: Option<&str>) -> Result<Feed, u16> {
        let host = format!("127.0.0.1:{}", self.addr.port());
        let mut request = format!("wss://{host}/api/live").into_client_request().unwrap();
        if let Some(origin) = origin {
            let origin = if Some(origin) == SAME { format!("https://{host}") } else { origin.to_owned() };
            request.headers_mut().insert(ORIGIN, origin.parse().unwrap());
        }
        if let Some(cookie) = cookie {
            request.headers_mut().insert(COOKIE, cookie.parse().unwrap());
        }
        let opening = async { tokio_tungstenite::client_async(request, self.tls.connect().await.unwrap()).await };
        match tokio::time::timeout(CALL_TIMEOUT, opening).await.expect("the live feed answers in time") {
            Ok((socket, _)) => Ok(Feed { socket }),
            Err(tokio_tungstenite::tungstenite::Error::Http(response)) => Err(response.status().as_u16()),
            Err(e) => panic!("the live feed failed: {e}"),
        }
    }
}

/// An open live feed.
pub struct Feed {
    socket: tokio_tungstenite::WebSocketStream<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>,
}

impl Feed {
    /// The next JSON message, or `None` once the feed closes; within ten seconds.
    pub async fn next(&mut self) -> Option<Value> {
        loop {
            let message = tokio::time::timeout(Duration::from_secs(10), self.socket.next()).await.expect("a message");
            match message {
                Some(Ok(Message::Text(text))) => return Some(serde_json::from_str(&text).unwrap()),
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return None,
                Some(Ok(_)) => {}
            }
        }
    }

    /// The next message of `kind`, skipping others.
    pub async fn next_of(&mut self, kind: &str) -> Value {
        loop {
            let message = self.next().await.unwrap_or_else(|| panic!("the feed closed before a {kind} message"));
            if message["type"] == kind {
                return message;
            }
        }
    }
}

/// Waits until `check` holds for the account's view, reading it every 100 ms for up to ten seconds.
pub async fn until_account(running: &Running, cookie: &str, address: &str, check: impl Fn(&Value) -> bool) -> Value {
    for _ in 0..100 {
        let answer = running.get(&format!("/api/accounts/{address}"), cookie).await;
        if answer.status == 200 && check(&answer.body) {
            return answer.body;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the account's view never matched: {:?}", running.get(&format!("/api/accounts/{address}"), cookie).await);
}

/// Seeds the stand-in for a console test: BTC-USD marked at 99, the test account holding 1000 USD
/// and nothing else, the log's head at 10, and the account's next nonce 3.
pub fn seed(stand_in: &StandIn) {
    stand_in.with(|i| {
        i.markets = serde_json::json!([super::btc_usd(Some("99"))]);
        i.accounts.insert(super::account().to_string(), figures_view("1000", "1000", "0", serde_json::json!({})));
        i.head_seq = 10;
        i.nonce = 3;
    });
}

/// An account view with the figures the console reads: balance, equity, margin, and positions.
pub fn figures_view(balance: &str, equity: &str, margin: &str, positions: Value) -> Value {
    serde_json::json!({ "balance": balance, "equity": equity, "margin": margin, "positions": positions, "orders": {} })
}

/// Waits until the bot has started following the log: its view says it is running or stopped.
pub async fn until_started(running: &Running, cookie: &str) -> Value {
    let address = super::account().to_string();
    until_account(running, cookie, &address, |view| view["state"] == "running" || view["state"] == "stopped").await
}

/// `future`'s output, within `CALL_TIMEOUT`.
pub async fn within<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(CALL_TIMEOUT, future).await.expect("done in time")
}
