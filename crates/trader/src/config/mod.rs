//! The bot's config: one TOML file, loaded and checked once at startup. Every section refuses keys
//! it does not know, so a typo fails the load instead of leaving a default in place. Relative paths
//! in it are read from the config file's directory.

mod validation;

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use engine_core::Fixed;
use engine_core::types::AccountId;
use serde::Deserialize;

/// The default time a call to the node may take, connection included.
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// The default longest silence on the stream; the node sends a heartbeat every few seconds.
const DEFAULT_STREAM_TIMEOUT: Duration = Duration::from_secs(30);

/// The whole config file.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub node: NodeSection,
    pub account: AccountSection,
    pub strategy: StrategySection,
    #[serde(default)]
    pub logging: LoggingSection,
}

/// The node the bot trades against, and what its answers are checked against.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeSection {
    /// The API's `https://host:port`.
    pub url: NodeUrl,
    /// The custody chain and vault every signature names.
    pub chain_id: u64,
    pub vault: AccountId,
    /// The enclave key's address, which must sign every answer and stream frame.
    pub signer: AccountId,
    /// The SHA-256 of the node's TLS key (its DER SubjectPublicKeyInfo), as the node's attestation
    /// names it, for a node whose certificate no public CA signs, as a dev node's. Without it the
    /// certificate must chain to a public root.
    #[serde(default)]
    pub tls_key_sha256: Option<TlsPin>,
    #[serde(default = "default_request_timeout", with = "humantime_serde")]
    pub request_timeout: Duration,
    #[serde(default = "default_stream_timeout", with = "humantime_serde")]
    pub stream_timeout: Duration,
}

/// The node's API address: a host and a port, over TLS.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeUrl {
    pub host: String,
    pub port: u16,
}

/// A TLS key hash: 32 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TlsPin(pub [u8; 32]);

/// The account the bot trades.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountSection {
    pub key: KeySource,
    /// Whether stopping the bot also closes the strategy's position.
    #[serde(default)]
    pub close_on_stop: bool,
}

/// Where the account key comes from.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum KeySource {
    /// An encrypted Web3 Secret Storage keystore, as wallets export it.
    Keystore { path: PathBuf, password: PasswordSource },
    /// A file holding the key as 64 hex characters. Dev builds only.
    HexFile { path: PathBuf },
}

/// Where a keystore's password comes from.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum PasswordSource {
    /// A file holding the password; one trailing newline is not part of it.
    File(PathBuf),
    /// An environment variable holding the password.
    Env(String),
}

/// The SMA crossover and the orders it places.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrategySection {
    /// The market's symbol, such as `BTC-USD`.
    pub market: String,
    /// The bar length: whole minutes, from one minute to one day.
    #[serde(with = "humantime_serde")]
    pub bar: Duration,
    /// The fast and slow averages' periods, in bars.
    pub fast: usize,
    pub slow: usize,
    /// The position size in the market's units, long or short.
    pub qty: Fixed,
    /// How far, in bps, a market order may fill from the mark it was quoted at.
    pub max_slippage_bps: i64,
}

/// Log output.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoggingSection {
    /// A tracing filter directive, such as `info` or `trader=debug`.
    pub level: String,
}

impl Default for LoggingSection {
    /// Logs at `info`.
    fn default() -> Self {
        Self { level: "info".into() }
    }
}

impl NodeUrl {
    /// `host:port`, with an IPv6 host in brackets.
    pub fn authority(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

impl Config {
    /// Reads, parses, and checks the config at `path`, and resolves its relative paths against the
    /// file's directory.
    /// Error cases:
    /// - the file cannot be read, or is not valid TOML of this shape, with no unknown key
    /// - a value fails its check (see `validate`)
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut cfg: Config = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        cfg.validate(cfg!(debug_assertions)).with_context(|| format!("checking {}", path.display()))?;
        cfg.resolve_paths(path.parent().unwrap_or(Path::new(".")));
        Ok(cfg)
    }

    /// Makes the key's relative paths relative to `dir`.
    fn resolve_paths(&mut self, dir: &Path) {
        let resolve = |path: &mut PathBuf| {
            if path.is_relative() {
                *path = dir.join(&*path);
            }
        };
        match &mut self.account.key {
            KeySource::Keystore { path, password } => {
                resolve(path);
                if let PasswordSource::File(file) = password {
                    resolve(file);
                }
            }
            KeySource::HexFile { path } => resolve(path),
        }
    }
}

/// `DEFAULT_REQUEST_TIMEOUT`, for serde.
fn default_request_timeout() -> Duration {
    DEFAULT_REQUEST_TIMEOUT
}

/// `DEFAULT_STREAM_TIMEOUT`, for serde.
fn default_stream_timeout() -> Duration {
    DEFAULT_STREAM_TIMEOUT
}

#[cfg(test)]
mod tests;
