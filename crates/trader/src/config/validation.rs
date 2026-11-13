//! Parsing of the config's own value types, and the checks a parsed config must pass.

use std::time::Duration;

use anyhow::{anyhow, bail, ensure};
use serde::{Deserialize, Deserializer};
use tracing_subscriber::EnvFilter;

use trader_core::risk::RiskParams;
use trader_core::sizing::Sizing;

use crate::config::{Config, KeySource, NodeUrl, StrategySection, TlsPin};

/// The shortest bar: the marks history the bot warms up from is in one-minute buckets.
const MIN_BAR: Duration = Duration::from_secs(60);
/// The longest bar.
const MAX_BAR: Duration = Duration::from_secs(24 * 60 * 60);
/// The most markets one account trades.
const MAX_STRATEGIES: usize = 16;
/// The most HTTP connections the bot may hold open: well inside the node's per-address cap.
const MAX_CONNECTIONS: usize = 16;
/// The widest slippage bound an order may name: the whole price.
const MAX_SLIPPAGE_BPS: i64 = 10_000;

impl Config {
    /// Checks the values serde cannot: that some account is named, each account's strategies, the
    /// request limits, the log filter, and that a plain hex key appears only in a dev build
    /// (`dev_build`).
    /// Error cases:
    /// - no account, or a hex key file outside a dev build
    /// - a strategy that `validate_strategies` refuses
    /// - a limit of zero, or connections outside 1 to `MAX_CONNECTIONS`
    /// - a log level that is not a tracing filter directive, or a zero timeout
    /// - a chain section that fails its checks (see `validate_chain`)
    pub(super) fn validate(&self, dev_build: bool) -> anyhow::Result<()> {
        ensure!(!self.account.is_empty() || !self.account_dir.is_empty(), "no [[account]] or [[account_dir]]");
        check_limits(&self.sizing, &self.risk)?;
        for account in &self.account {
            check_limits(account.sizing.as_ref().unwrap_or(&self.sizing), account.risk.as_ref().unwrap_or(&self.risk))?;
            if matches!(account.key, KeySource::HexFile { .. }) && !dev_build {
                bail!("account.key: a plain hex key is accepted only in a dev build; use a keystore");
            }
            validate_strategies(&account.strategy)?;
        }
        for dir in &self.account_dir {
            check_limits(dir.sizing.as_ref().unwrap_or(&self.sizing), dir.risk.as_ref().unwrap_or(&self.risk))?;
            validate_strategies(&dir.strategy)?;
        }
        let limits = &self.node.limits;
        for (name, rate) in [("per_ip", limits.per_ip), ("per_account", limits.per_account)] {
            ensure!(rate.per_second > 0 && rate.burst > 0, "node.limits.{name}: per_second and burst above zero");
        }
        let fresh = limits.fresh_accounts;
        ensure!(fresh.per_minute > 0 && fresh.burst > 0, "node.limits.fresh_accounts: per_minute and burst above zero");
        ensure!(
            (1..=MAX_CONNECTIONS).contains(&limits.connections),
            "node.limits.connections: from 1 to {MAX_CONNECTIONS}"
        );
        EnvFilter::try_new(&self.logging.level).map_err(|e| anyhow!("logging.level: {e}"))?;
        ensure!(!self.node.request_timeout.is_zero(), "node.request_timeout: above zero");
        ensure!(!self.node.stream_timeout.is_zero(), "node.stream_timeout: above zero");
        self.validate_chain(dev_build)
    }
}

/// Checks the sizing and the risk limits.
/// Error cases:
/// - a risk limit or a sizing figure out of range
fn check_limits(sizing: &Sizing, risk: &RiskParams) -> anyhow::Result<()> {
    risk.validate().map_err(|e| anyhow!("risk: {e}"))?;
    sizing.check().map_err(|e| anyhow!("sizing: {e}"))
}

/// Checks one account's strategies: how many, and each one's values.
/// Error cases:
/// - none, more than `MAX_STRATEGIES`, or one market twice
/// - fast and slow periods `SmaCrossover` refuses
/// - a bar that is not whole minutes from one minute to one day
/// - a size that is not above zero and within the engine's input bound
/// - a slippage bound below zero or above 10,000 bps
fn validate_strategies(strategies: &[StrategySection]) -> anyhow::Result<()> {
    ensure!((1..=MAX_STRATEGIES).contains(&strategies.len()), "strategy: from 1 to {MAX_STRATEGIES} per account");
    for (i, strategy) in strategies.iter().enumerate() {
        ensure!(
            strategies[..i].iter().all(|other| other.market != strategy.market),
            "strategy.market: {} twice for one account",
            strategy.market
        );
        strategy.params.build().map_err(|e| anyhow!("strategy: {e}"))?;
        let bar = strategy.bar;
        ensure!(
            (MIN_BAR..=MAX_BAR).contains(&bar) && bar.as_millis().is_multiple_of(MIN_BAR.as_millis()),
            "strategy.bar: whole minutes from 1m to 1d, not {bar:?}"
        );
        ensure!(
            (0..=MAX_SLIPPAGE_BPS).contains(&strategy.max_slippage_bps),
            "strategy.max_slippage_bps: from 0 to {MAX_SLIPPAGE_BPS}"
        );
    }
    Ok(())
}

impl<'de> Deserialize<'de> for NodeUrl {
    /// From `https://host:port`, with no path, query, or user.
    /// Error cases:
    /// - another scheme, no port, or a path, query, or user part
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        parse_url(&text).ok_or_else(|| serde::de::Error::custom(format!("{text:?} is not https://host:port")))
    }
}

impl<'de> Deserialize<'de> for TlsPin {
    /// From 64 hex characters, with or without `0x`.
    /// Error cases:
    /// - not 32 bytes of hex
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        let mut pin = [0u8; 32];
        hex::decode_to_slice(text.trim_start_matches("0x"), &mut pin)
            .map_err(|_| serde::de::Error::custom("a TLS key hash is 32 bytes of hex"))?;
        Ok(TlsPin(pin))
    }
}

/// The host and port of `https://host:port`, where the host is a name, an IPv4 address, or an IPv6
/// address in brackets; `None` for anything else.
fn parse_url(text: &str) -> Option<NodeUrl> {
    let rest = text.strip_prefix("https://")?;
    let authority = rest.strip_suffix('/').unwrap_or(rest);
    if authority.contains(['/', '?', '#', '@']) {
        return None;
    }
    let (host, port) = authority.rsplit_once(':')?;
    let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    if host.is_empty() || (host.contains(':') && !authority.starts_with('[')) {
        return None;
    }
    Some(NodeUrl { host: host.to_owned(), port: port.parse().ok()? })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_https_host_and_port_only() {
        let url = |host: &str, port| Some(NodeUrl { host: host.into(), port });
        assert_eq!(parse_url("https://127.0.0.1:8443"), url("127.0.0.1", 8443));
        assert_eq!(parse_url("https://node.example:443/"), url("node.example", 443));
        assert_eq!(parse_url("https://[::1]:9000"), url("::1", 9000));
        for bad in [
            "http://127.0.0.1:8443",
            "https://127.0.0.1",
            "https://127.0.0.1:8443/v1",
            "https://127.0.0.1:8443?x=1",
            "https://user@host:1",
            "https://:1",
            "https://::1:9000",
            "https://host:99999",
        ] {
            assert_eq!(parse_url(bad), None, "{bad}");
        }
    }
}
