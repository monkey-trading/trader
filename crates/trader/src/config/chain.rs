//! The sections for the custody chain: its RPC endpoints, the optional funding key, and the
//! optional automatic MONKEY claims.

use std::fmt;
use std::time::Duration;

use alloy_primitives::U256;
use alloy_primitives::utils::parse_units;
use anyhow::{bail, ensure};
use serde::de::{SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use zeroize::Zeroizing;

use crate::config::{Config, KeySource};

/// MONKEY's decimals: the engine counts entitlements in base units of 10^-18.
pub const MONKEY_DECIMALS: u8 = 18;
/// Wei in a gwei, as decimals.
const GWEI_DECIMALS: u8 = 9;
/// The default time one RPC request may take.
const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_secs(10);
/// The default time a sent transaction may take to be mined.
const DEFAULT_RECEIPT_TIMEOUT: Duration = Duration::from_secs(300);
/// The default time the bot waits on the node: a deposit's credit, a withdrawal's approval and its
/// payout's authorization.
pub(crate) const DEFAULT_SETTLE_TIMEOUT: Duration = Duration::from_secs(1800);
/// The default time between two polls of the chain or the node while the bot waits.
pub(crate) const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);
/// The default highest fee per gas a transaction may offer, in gwei: far above Arbitrum's usual
/// hundredth of a gwei, and far below what an endpoint misquoting it could otherwise cost.
const DEFAULT_MAX_FEE_GWEI: u128 = 1;

/// The custody chain the vault is on: Arbitrum in production.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChainSection {
    /// JSON-RPC endpoints, tried in order. A read moves to the next when one fails, and every
    /// transaction is sent to each.
    #[serde(deserialize_with = "rpc_urls")]
    pub rpc: Vec<RpcUrl>,
    #[serde(default = "default_rpc_timeout", with = "humantime_serde")]
    pub rpc_timeout: Duration,
    #[serde(default = "default_receipt_timeout", with = "humantime_serde")]
    pub receipt_timeout: Duration,
    #[serde(default = "default_settle_timeout", with = "humantime_serde")]
    pub settle_timeout: Duration,
    #[serde(default = "default_poll_interval", with = "humantime_serde")]
    pub poll_interval: Duration,
    /// The highest fee per gas a transaction may offer, in gwei; one the chain's price would need
    /// higher is not sent.
    #[serde(default = "default_max_fee")]
    pub max_fee_gwei: Gwei,
}

/// A fee per gas in wei, written in gwei as a decimal string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gwei(pub u128);

/// A JSON-RPC endpoint's URL. Providers put an API key in the path or the query, so the URL is
/// never printed, and its copy here is zeroed when dropped.
pub struct RpcUrl(Zeroizing<String>);

/// A key that pays the gas of every account's claims and withdrawals, and deposits USDC from its
/// own wallet into each account. It never receives a payout: the vault pays the account.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FundingSection {
    pub key: KeySource,
}

/// Claiming each account's MONKEY while the bot runs.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimsSection {
    /// Claims once an account's unclaimed MONKEY reaches this much, in MONKEY.
    pub threshold: MonkeyAmount,
    /// How often the bot checks.
    #[serde(with = "humantime_serde")]
    pub interval: Duration,
}

/// An amount of MONKEY in base units, written in MONKEY as a decimal string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MonkeyAmount(pub U256);

impl RpcUrl {
    /// The URL, for the client that connects to it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for RpcUrl {
    /// Never the URL.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RpcUrl(..)")
    }
}

impl<'de> Deserialize<'de> for RpcUrl {
    /// From an `https://` URL, or an `http://` one on this machine, as a local chain's.
    /// Error cases:
    /// - not a URL, or another scheme, or plain HTTP to another machine
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = Zeroizing::new(String::deserialize(d)?);
        // The error never quotes the URL, which may carry a credential.
        let url = reqwest::Url::parse(&text).map_err(|_| serde::de::Error::custom("chain.rpc: not a URL"))?;
        let local = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
        if url.scheme() != "https" && !(url.scheme() == "http" && local) {
            return Err(serde::de::Error::custom("chain.rpc: https://, or http:// on this machine"));
        }
        Ok(Self(text))
    }
}

impl<'de> Deserialize<'de> for MonkeyAmount {
    /// From a decimal string of at most 18 decimals, such as `"250"` or `"0.5"`.
    /// Error cases:
    /// - not a non-negative decimal, or more decimals than MONKEY has
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        let units = base_units(&text, MONKEY_DECIMALS)
            .ok_or_else(|| serde::de::Error::custom(format!("{text:?} is not an amount of MONKEY")))?;
        Ok(Self(units))
    }
}

impl<'de> Deserialize<'de> for Gwei {
    /// From a decimal string of gwei, such as `"0.1"`.
    /// Error cases:
    /// - not a decimal above zero with at most nine decimals, or past a `u128` of wei
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        let wei = base_units(&text, GWEI_DECIMALS)
            .and_then(|wei| u128::try_from(wei).ok())
            .filter(|wei| *wei > 0)
            .ok_or_else(|| serde::de::Error::custom(format!("{text:?} is not a fee in gwei above zero")))?;
        Ok(Self(wei))
    }
}

impl Config {
    /// Checks the chain sections: at least one endpoint and non-zero durations, a funding key that
    /// is a keystore outside a dev build (`dev_build`), and claims only with a chain to claim on.
    /// Error cases:
    /// - `[chain]` with no endpoint, or a zero duration
    /// - a hex funding key outside a dev build
    /// - `[claims]` without `[chain]`, or with a zero interval
    pub(super) fn validate_chain(&self, dev_build: bool) -> anyhow::Result<()> {
        if let Some(chain) = &self.chain {
            ensure!(!chain.rpc.is_empty(), "chain.rpc: at least one endpoint");
            for (name, duration) in [
                ("rpc_timeout", chain.rpc_timeout),
                ("receipt_timeout", chain.receipt_timeout),
                ("settle_timeout", chain.settle_timeout),
                ("poll_interval", chain.poll_interval),
            ] {
                ensure!(!duration.is_zero(), "chain.{name}: above zero");
            }
        }
        if let Some(funding) = &self.funding
            && matches!(funding.key, KeySource::HexFile { .. })
            && !dev_build
        {
            bail!("funding.key: a plain hex key is accepted only in a dev build; use a keystore");
        }
        if let Some(claims) = &self.claims {
            ensure!(self.chain.is_some(), "[claims] needs a [chain] section to claim on");
            ensure!(!claims.interval.is_zero(), "claims.interval: above zero");
        }
        Ok(())
    }
}

/// A list of RPC URLs. Anything else is refused without its value, which may be a URL with a
/// credential: serde's own type errors quote it.
/// Error cases:
/// - not a list of URLs `RpcUrl` takes
fn rpc_urls<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<RpcUrl>, D::Error> {
    /// Visits a list, and refuses any other value unquoted.
    struct List;
    impl<'de> Visitor<'de> for List {
        type Value = Vec<RpcUrl>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a list of URLs")
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut urls = Vec::new();
            while let Some(url) = seq.next_element()? {
                urls.push(url);
            }
            Ok(urls)
        }

        fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<Self::Value, E> {
            Err(E::custom("chain.rpc: a list of URLs, such as [\"https://...\"]"))
        }
    }
    d.deserialize_seq(List)
}

/// The base units of a non-negative decimal `text` of a unit with `decimals`, or `None` when it is
/// not one or has more decimals than the unit, which would lose some.
fn base_units(text: &str, decimals: u8) -> Option<U256> {
    if text.split_once('.').is_some_and(|(_, fraction)| fraction.len() > usize::from(decimals)) {
        return None;
    }
    let units = parse_units(text, decimals).ok()?;
    (!units.is_negative()).then(|| units.get_absolute())
}

/// `DEFAULT_RPC_TIMEOUT`, for serde.
fn default_rpc_timeout() -> Duration {
    DEFAULT_RPC_TIMEOUT
}

/// `DEFAULT_RECEIPT_TIMEOUT`, for serde.
fn default_receipt_timeout() -> Duration {
    DEFAULT_RECEIPT_TIMEOUT
}

/// `DEFAULT_SETTLE_TIMEOUT`, for serde.
fn default_settle_timeout() -> Duration {
    DEFAULT_SETTLE_TIMEOUT
}

/// `DEFAULT_POLL_INTERVAL`, for serde.
fn default_poll_interval() -> Duration {
    DEFAULT_POLL_INTERVAL
}

/// `DEFAULT_MAX_FEE_GWEI`, for serde.
fn default_max_fee() -> Gwei {
    Gwei(DEFAULT_MAX_FEE_GWEI * 1_000_000_000)
}
