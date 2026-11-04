//! The console: an HTTPS server on the host's address that serves the console app's files and an
//! API for everything it shows and controls, behind a login. Its certificate and the login's hash
//! live in its data directory, both generated on first run; every start prints the certificate's
//! SHA-256 fingerprint, and the first prints the password, once. Accounts are reached through their
//! links (`bot::link`); a refresher reads their figures from the node.

mod assets;
mod board;
mod guard;
mod live;
mod login;
mod refresh;
mod routes;
mod server;
mod session;
mod tls;

use std::io::Write;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, anyhow};
use axum::Router;
use axum::extract::ConnectInfo;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::service::TowerToHyperService;
use rustls::ServerConfig;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::timeout;
use tokio_rustls::TlsAcceptor;
use tracing::{debug, info, warn};

use crate::bot::link::AccountLink;
use crate::config::ConsoleSection;
use crate::console::refresh::Refresher;
use crate::console::server::Server;
use crate::console::tls::{Identity, fingerprint_text};
use crate::files::create_private_dir;
use crate::node::Node;

/// The most connections open at once.
const CONNECTIONS: usize = 64;
/// How long a client may take over its TLS handshake, and over a request's headers.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the accept loop rests after the system refuses a connection, such as when it is out of
/// file descriptors.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

/// The console's address, identity and login, ready to serve.
pub struct Prepared {
    listener: std::net::TcpListener,
    identity: Identity,
    password_hash: String,
}

/// The console's tasks: its server and its refresher.
pub struct Console {
    tasks: JoinSet<anyhow::Result<()>>,
}

/// Binds the console's address; creates the data directory, and the certificate and login when
/// they are missing; prints the certificate's fingerprint, and a new password, to `out`. Runs
/// before anything trades, off the async runtime: it reads and writes files and hashes the
/// password.
/// Error cases:
/// - the address cannot be bound
/// - the data directory, the certificate, or the login cannot be read or created (see `tls`,
///   `login`)
/// - `out` cannot be written
pub fn prepare(cfg: &ConsoleSection, out: &mut dyn Write) -> anyhow::Result<Prepared> {
    let listener =
        std::net::TcpListener::bind(cfg.bind).with_context(|| format!("binding the console to {}", cfg.bind))?;
    create_private_dir(&cfg.data_dir)?;
    let (identity, generated) = tls::identity(cfg)?;
    let fingerprint = fingerprint_text(&identity.fingerprint);
    let made = if generated { "generated now, " } else { "" };
    writeln!(out, "console certificate ({made}compare it with the browser's): SHA-256 fingerprint {fingerprint}")
        .context("printing the certificate's fingerprint")?;
    let password_hash = login::password_hash(&cfg.data_dir, out)?;
    Ok(Prepared { listener, identity, password_hash })
}

impl Prepared {
    /// The address the console listens on.
    /// Error cases:
    /// - the system cannot say
    pub fn local_addr(&self) -> anyhow::Result<SocketAddr> {
        self.listener.local_addr().context("the console's address")
    }
}

impl Console {
    /// Starts serving `accounts` on the prepared address, with figures `node` gives.
    /// Error cases:
    /// - the prepared listener cannot be handed to the async runtime
    pub async fn start(
        cfg: &ConsoleSection,
        prepared: Prepared,
        node: Node,
        accounts: Vec<AccountLink>,
    ) -> anyhow::Result<Self> {
        let local_addr = prepared.local_addr()?;
        prepared.listener.set_nonblocking(true).context("the console's listener")?;
        let listener = TcpListener::from_std(prepared.listener).context("the console's listener")?;
        let fingerprint = fingerprint_text(&prepared.identity.fingerprint);
        info!(address = %local_addr, fingerprint, "console listening");
        let addresses: Vec<_> = accounts.iter().map(|account| account.address).collect();
        let (refresher, views) = Refresher::new(node, cfg.refresh_interval, &addresses);
        let router = routes::router(Arc::new(Server::new(accounts, views, prepared.password_hash)));
        let mut tasks = JoinSet::new();
        tasks.spawn(serve(listener, prepared.identity.config, router));
        tasks.spawn(async move {
            refresher.run().await;
            Ok(())
        });
        Ok(Self { tasks })
    }

    /// Waits until one of the console's tasks ends, which it does only on a failure, and says why.
    pub async fn failed(&mut self) -> anyhow::Error {
        match self.tasks.join_next().await {
            Some(Ok(Err(e))) => e,
            Some(Ok(Ok(()))) => anyhow!("a console task ended"),
            Some(Err(e)) => anyhow!("a console task panicked: {e}"),
            None => anyhow!("the console has no tasks"),
        }
    }
}

/// Accepts connections on `listener` and serves `router` over TLS on each, up to `CONNECTIONS` at
/// once; a connection past that is closed at once. Returns only when a connection's task panics.
/// Error cases:
/// - a connection's task panicked: a bug, which stops the console rather than leave it serving
///   half-working
async fn serve(listener: TcpListener, tls: Arc<ServerConfig>, router: Router) -> anyhow::Result<()> {
    let acceptor = TlsAcceptor::from(tls);
    let open = Arc::new(Semaphore::new(CONNECTIONS));
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            Some(ended) = connections.join_next() => {
                if let Err(e) = ended
                    && e.is_panic()
                {
                    return Err(anyhow!("a console connection panicked: {e}"));
                }
            }
            accepted = listener.accept() => {
                let (tcp, client) = match accepted {
                    Ok(accepted) => accepted,
                    Err(e) => {
                        warn!("the console could not accept a connection: {e}");
                        tokio::time::sleep(ACCEPT_BACKOFF).await;
                        continue;
                    }
                };
                let Ok(permit) = open.clone().try_acquire_owned() else {
                    debug!(%client, "too many console connections; closed");
                    continue;
                };
                let (acceptor, router) = (acceptor.clone(), router.clone());
                connections.spawn(async move {
                    let _permit = permit;
                    let tls = match timeout(HANDSHAKE_TIMEOUT, acceptor.accept(tcp)).await {
                        Ok(Ok(tls)) => tls,
                        Ok(Err(e)) => return debug!(%client, "TLS handshake failed: {e}"),
                        Err(_) => return debug!(%client, "TLS handshake timed out"),
                    };
                    let service = TowerToHyperService::new(router.layer(axum::Extension(ConnectInfo(client))));
                    let served = hyper::server::conn::http1::Builder::new()
                        .timer(TokioTimer::new())
                        .header_read_timeout(HEADER_TIMEOUT)
                        .serve_connection(TokioIo::new(tls), service)
                        .with_upgrades()
                        .await;
                    if let Err(e) = served {
                        debug!(%client, "connection ended: {e}");
                    }
                });
            }
        }
    }
}
