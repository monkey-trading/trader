//! The bot's config: one TOML file, loaded and checked once at startup. Every section refuses keys
//! it does not know, so a typo fails the load instead of leaving a default in place. Relative paths
//! in it are read from the config file's directory.

mod chain;
pub use chain::*;
pub mod bounds;
mod validation;

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, anyhow};
use engine_core::types::AccountId;
use serde::Deserialize;
use trader_core::risk::RiskParams;
use trader_core::sizing::Sizing;
use trader_core::strategies::StrategyParams;

pub use bounds::ParamBounds;

/// The default time a call to the node may take, connection included.
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// The default longest silence on the stream; the node sends a heartbeat every few seconds.
const DEFAULT_STREAM_TIMEOUT: Duration = Duration::from_secs(30);
/// The console's default address: every interface, so it is reachable at the host's IP.
const DEFAULT_CONSOLE_BIND: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::UNSPECIFIED), 8443);
/// How often, by default, the console reads each account from the node.
const DEFAULT_REFRESH_INTERVAL: Duration = Duration::from_secs(5);

/// The whole config file.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub node: NodeSection,
    /// How large a new position is, for every account that does not set its own.
    pub sizing: Sizing,
    /// The risk limits of every account that does not set its own.
    pub risk: RiskParams,
    /// Where the bot keeps what it must not lose across a restart.
    #[serde(default)]
    pub state: StateSection,
    /// Accounts listed one by one.
    #[serde(default)]
    pub account: Vec<AccountSection>,
    /// Directories of keystores, one account per keystore.
    #[serde(default)]
    pub account_dir: Vec<AccountDirSection>,
    /// The custody chain, for deposits, MONKEY claims and withdrawals.
    #[serde(default)]
    pub chain: Option<ChainSection>,
    #[serde(default)]
    pub funding: Option<FundingSection>,
    #[serde(default)]
    pub claims: Option<ClaimsSection>,
    /// The console; without the section the bot serves none.
    #[serde(default)]
    pub console: Option<ConsoleSection>,
    #[serde(default)]
    pub logging: LoggingSection,
    /// Every account the config names, directories expanded; filled by `load`.
    #[serde(skip)]
    pub plans: Vec<AccountPlan>,
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
    /// The node's request limits and the connections the bot may hold open.
    #[serde(default)]
    pub limits: LimitsSection,
}

/// A token bucket as the node's `[api]` section sets it: `burst` requests at once, refilled at
/// `per_second`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RateSection {
    pub per_second: u32,
    pub burst: u32,
}

/// A token bucket counted by the minute, as the node's `[api.fresh_accounts]` sets it: `burst`
/// requests at once, refilled at `per_minute`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MinuteRateSection {
    pub per_minute: u32,
    pub burst: u32,
}

/// The node's limits on this bot, which the bot paces its requests under. Set them to the node's
/// `api.per_ip`, `api.per_account` and `api.fresh_accounts.per_client`; the defaults are the
/// engine's development config.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LimitsSection {
    /// Every request the bot makes counts against this bucket.
    #[serde(default = "default_per_ip")]
    pub per_ip: RateSection,
    /// Orders and cancels count against their account's bucket.
    #[serde(default = "default_per_account")]
    pub per_account: RateSection,
    /// Declarations count against this bucket as well: the node's budget for one client's setup
    /// requests, which holds every declaration it takes from the bot's address.
    #[serde(default = "default_fresh_accounts")]
    pub fresh_accounts: MinuteRateSection,
    /// The most HTTP connections open at once, the stream's not counted. The node caps connections
    /// per address (`api.max_connections_per_ip`), the stream's included.
    #[serde(default = "default_connections")]
    pub connections: usize,
}

impl Default for LimitsSection {
    /// The engine's development limits.
    fn default() -> Self {
        Self {
            per_ip: default_per_ip(),
            per_account: default_per_account(),
            fresh_accounts: default_fresh_accounts(),
            connections: default_connections(),
        }
    }
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

/// One account the bot trades, with the markets it trades and the strategy in each.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountSection {
    pub key: KeySource,
    /// Whether stopping the bot also closes the account's positions in its markets.
    #[serde(default)]
    pub close_on_stop: bool,
    /// This account's own sizing, in place of the config's.
    pub sizing: Option<Sizing>,
    /// This account's own risk limits, in place of the config's.
    pub risk: Option<RiskParams>,
    pub strategy: Vec<StrategySection>,
}

/// A directory of keystores: every `.json` file in it is an account, all with this password source
/// and these strategies.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountDirSection {
    pub path: PathBuf,
    pub password: PasswordSource,
    #[serde(default)]
    pub close_on_stop: bool,
    pub sizing: Option<Sizing>,
    pub risk: Option<RiskParams>,
    pub strategy: Vec<StrategySection>,
}

/// One account to trade: where its key is, and what it trades.
#[derive(Clone, Debug)]
pub struct AccountPlan {
    pub key: KeySource,
    pub close_on_stop: bool,
    pub sizing: Sizing,
    pub risk: RiskParams,
    pub strategies: Vec<StrategySection>,
}

/// Where the account key comes from.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum KeySource {
    /// An encrypted Web3 Secret Storage keystore, as wallets export it.
    Keystore { path: PathBuf, password: PasswordSource },
    /// A file holding the key as 64 hex characters. Dev builds only.
    HexFile { path: PathBuf },
}

/// Where a keystore's password comes from.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum PasswordSource {
    /// A file holding the password; one trailing newline is not part of it.
    File(PathBuf),
    /// An environment variable holding the password.
    Env(String),
}

/// The strategy in one market, the bars it reads, and the orders it places.
#[derive(Clone, Debug, Deserialize)]
pub struct StrategySection {
    /// The market's symbol, such as `BTC-USD`.
    pub market: String,
    /// The bar length: whole minutes, from one minute to one day.
    #[serde(with = "humantime_serde")]
    pub bar: Duration,
    /// How far, in bps, a market order may fill from the mark it was quoted at.
    pub max_slippage_bps: i64,
    /// For each parameter the console may set, its lowest and highest value; a parameter without
    /// bounds is the config's alone.
    #[serde(default)]
    pub bounds: Option<ParamBounds>,
    /// The strategy by `kind`, with its parameters. Not `deny_unknown_fields`, which serde cannot
    /// combine with `flatten`: the strategy's own section refuses a key it does not know.
    #[serde(flatten)]
    pub params: StrategyParams,
}

/// The console: an HTTPS server on the host's address, behind a login.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsoleSection {
    #[serde(default = "default_console_bind")]
    pub bind: SocketAddr,
    /// Where the console keeps its generated certificate, its login, and the controls it set.
    #[serde(default = "default_console_data_dir")]
    pub data_dir: PathBuf,
    /// A certificate and key to serve instead of the generated ones.
    #[serde(default)]
    pub tls: Option<ConsoleTls>,
    /// How often the console reads each account from the node.
    #[serde(default = "default_refresh_interval", with = "humantime_serde")]
    pub refresh_interval: Duration,
}

/// PEM files: a certificate chain, and its private key.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsoleTls {
    pub cert: PathBuf,
    pub key: PathBuf,
}

/// Where the bot keeps its state.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateSection {
    /// The day's loss and the positions' opening times, across restarts: one file for each account,
    /// named for its address.
    #[serde(default = "default_state_dir")]
    pub dir: PathBuf,
}

impl Default for StateSection {
    /// A `state` directory beside the config.
    fn default() -> Self {
        Self { dir: default_state_dir() }
    }
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
        let mut cfg = parse(&text).with_context(|| format!("parsing {}", path.display()))?;
        cfg.validate(cfg!(debug_assertions)).with_context(|| format!("checking {}", path.display()))?;
        cfg.resolve_paths(path.parent().unwrap_or(Path::new(".")));
        cfg.plans = cfg.expand().with_context(|| format!("reading the accounts of {}", path.display()))?;
        Ok(cfg)
    }

    /// Makes every relative path relative to `dir`.
    fn resolve_paths(&mut self, dir: &Path) {
        if self.state.dir.is_relative() {
            self.state.dir = dir.join(&self.state.dir);
        }
        let resolve = |path: &mut PathBuf| {
            if path.is_relative() {
                *path = dir.join(&*path);
            }
        };
        let resolve_password = |password: &mut PasswordSource| {
            if let PasswordSource::File(file) = password {
                resolve(file);
            }
        };
        let funding = self.funding.as_mut().map(|funding| &mut funding.key);
        for key in self.account.iter_mut().map(|account| &mut account.key).chain(funding) {
            match key {
                KeySource::Keystore { path, password } => {
                    resolve(path);
                    resolve_password(password);
                }
                KeySource::HexFile { path } => resolve(path),
            }
        }
        for account_dir in &mut self.account_dir {
            resolve(&mut account_dir.path);
            resolve_password(&mut account_dir.password);
        }
        if let Some(console) = &mut self.console {
            resolve(&mut console.data_dir);
            if let Some(tls) = &mut console.tls {
                resolve(&mut tls.cert);
                resolve(&mut tls.key);
            }
        }
    }

    /// The accounts in config order: the listed ones, then each directory's keystores by file name.
    /// Error cases:
    /// - a directory cannot be read or holds no `.json` keystore
    fn expand(&self) -> anyhow::Result<Vec<AccountPlan>> {
        let mut plans: Vec<AccountPlan> = self
            .account
            .iter()
            .map(|a| AccountPlan {
                key: a.key.clone(),
                close_on_stop: a.close_on_stop,
                sizing: a.sizing.clone().unwrap_or_else(|| self.sizing.clone()),
                risk: a.risk.clone().unwrap_or_else(|| self.risk.clone()),
                strategies: a.strategy.clone(),
            })
            .collect();
        for dir in &self.account_dir {
            let mut files: Vec<PathBuf> = std::fs::read_dir(&dir.path)
                .with_context(|| format!("reading {}", dir.path.display()))?
                .map(|entry| entry.map(|e| e.path()))
                .collect::<Result<_, _>>()
                .with_context(|| format!("reading {}", dir.path.display()))?;
            files.retain(|file| file.extension().is_some_and(|ext| ext == "json") && file.is_file());
            files.sort();
            anyhow::ensure!(!files.is_empty(), "{} holds no .json keystore", dir.path.display());
            plans.extend(files.into_iter().map(|path| AccountPlan {
                key: KeySource::Keystore { path, password: dir.password.clone() },
                close_on_stop: dir.close_on_stop,
                sizing: dir.sizing.clone().unwrap_or_else(|| self.sizing.clone()),
                risk: dir.risk.clone().unwrap_or_else(|| self.risk.clone()),
                strategies: dir.strategy.clone(),
            }));
        }
        Ok(plans)
    }
}

/// `DEFAULT_CONSOLE_BIND`, for serde.
fn default_console_bind() -> SocketAddr {
    DEFAULT_CONSOLE_BIND
}

/// `data`, beside the config file, for serde.
fn default_console_data_dir() -> PathBuf {
    PathBuf::from("data")
}

/// `DEFAULT_REFRESH_INTERVAL`, for serde.
fn default_refresh_interval() -> Duration {
    DEFAULT_REFRESH_INTERVAL
}

/// The directory of the saved state, beside the config.
fn default_state_dir() -> PathBuf {
    PathBuf::from("state")
}

/// The engine's development `api.per_ip`, for serde.
fn default_per_ip() -> RateSection {
    RateSection { per_second: 50, burst: 100 }
}

/// The engine's development `api.per_account`, for serde.
fn default_per_account() -> RateSection {
    RateSection { per_second: 10, burst: 20 }
}

/// The engine's development `api.fresh_accounts.per_client`, for serde.
fn default_fresh_accounts() -> MinuteRateSection {
    MinuteRateSection { per_minute: 600, burst: 20 }
}

/// Four connections, for serde.
fn default_connections() -> usize {
    4
}

/// The config in `text`. A parse error names its line and what is wrong there, but never quotes
/// the text, which may hold an RPC endpoint's credential.
/// Error cases:
/// - `text` is not valid TOML of the config's shape, with no unknown key
fn parse(text: &str) -> anyhow::Result<Config> {
    toml::from_str(text).map_err(|e| {
        let line = e.span().map_or(0, |span| text[..span.start.min(text.len())].matches('\n').count() + 1);
        anyhow!("line {line}: {}", e.message())
    })
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
