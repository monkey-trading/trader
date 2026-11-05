//! Parsing of the config's own value types, and the checks a parsed config must pass.

use std::time::Duration;

use anyhow::{anyhow, bail, ensure};
use serde::{Deserialize, Deserializer};
use tracing_subscriber::EnvFilter;

use crate::config::{Config, KeySource, NodeUrl, TlsPin};

/// The shortest bar: the marks history the bot warms up from is in one-minute buckets.
const MIN_BAR: Duration = Duration::from_secs(60);
/// The longest bar.
const MAX_BAR: Duration = Duration::from_secs(24 * 60 * 60);
/// The widest slippage bound an order may name: the whole price.
const MAX_SLIPPAGE_BPS: i64 = 10_000;

impl Config {
    /// Checks the values serde cannot: the strategy's parameters, bar length and slippage bound, the
    /// risk limits, the log filter, and that a plain hex key appears only in a dev build (`dev_build`).
    /// Error cases:
    /// - a hex key file outside a dev build
    /// - a strategy parameter its strategy refuses, or a risk limit or sizing figure out of range
    /// - a bar that is not whole minutes from one minute to one day
    /// - a slippage bound below zero or above 10,000 bps
    /// - a log level that is not a tracing filter directive
    pub(super) fn validate(&self, dev_build: bool) -> anyhow::Result<()> {
        if matches!(self.account.key, KeySource::HexFile { .. }) && !dev_build {
            bail!("account.key: a plain hex key is accepted only in a dev build; use a keystore");
        }
        let strategy = &self.strategy;
        strategy.params.build().map_err(|e| anyhow!("strategy: {e}"))?;
        self.risk.validate().map_err(|e| anyhow!("risk: {e}"))?;
        self.sizing.check().map_err(|e| anyhow!("sizing: {e}"))?;
        let bar = strategy.bar;
        ensure!(
            (MIN_BAR..=MAX_BAR).contains(&bar) && bar.as_millis().is_multiple_of(MIN_BAR.as_millis()),
            "strategy.bar: whole minutes from 1m to 1d, not {bar:?}"
        );
        ensure!(
            (0..=MAX_SLIPPAGE_BPS).contains(&strategy.max_slippage_bps),
            "strategy.max_slippage_bps: from 0 to {MAX_SLIPPAGE_BPS}"
        );
        EnvFilter::try_new(&self.logging.level).map_err(|e| anyhow!("logging.level: {e}"))?;
        ensure!(!self.node.request_timeout.is_zero(), "node.request_timeout: above zero");
        ensure!(!self.node.stream_timeout.is_zero(), "node.stream_timeout: above zero");
        Ok(())
    }
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
