use std::path::Path;

use trader_core::risk::LossLimit;

use super::*;

/// The committed example, which the README and the dev stack point to.
const EXAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config/example.toml");

/// The example loads, every relative path resolved against its directory.
#[test]
fn the_example_loads() {
    let cfg = Config::load(Path::new(EXAMPLE)).unwrap();
    assert_eq!(cfg.node.url, NodeUrl { host: "node.monkeytrading.example".into(), port: 443 });
    assert_eq!(cfg.node.chain_id, 42161);
    assert_eq!(cfg.node.tls_key_sha256, None);
    assert_eq!(cfg.node.request_timeout, Duration::from_secs(10));
    let AttestationSection::SevSnp { releases, root: None } = &cfg.node.attestation else {
        panic!("the example verifies an SEV-SNP attestation against the vendor's roots alone");
    };
    assert_eq!(releases, &[Path::new(EXAMPLE).parent().unwrap().join("engine-release.toml")]);
    assert_eq!(cfg.metrics.map(|m| m.listen), Some("127.0.0.1:9466".parse().unwrap()));
    assert_eq!(cfg.strategy.market, "BTC-USD");
    assert_eq!(cfg.strategy.params, StrategyParams::SmaCrossover { fast: 5, slow: 20 });
    assert_eq!(
        cfg.sizing,
        Sizing::FixedNotional { notional: "1000".parse().unwrap(), max_leverage: "5".parse().unwrap() }
    );
    assert_eq!(cfg.risk.max_open_positions, 1);
    assert_eq!(cfg.risk.daily_loss, LossLimit::Usd("100".parse().unwrap()));
    assert_eq!(cfg.risk.min_hold, Duration::from_secs(3600));
    assert_eq!(cfg.account.state_dir, Path::new(EXAMPLE).parent().unwrap().join("state"));
    let KeySource::Keystore { path, password: PasswordSource::Env(name) } = &cfg.account.key else {
        panic!("the example names a keystore and a password variable");
    };
    assert_eq!(path, &Path::new(EXAMPLE).parent().unwrap().join("keys/account.json"));
    assert_eq!(name, "TRADER_KEY_PASSWORD");
}

/// An unknown key is refused in every section, the key and password tables included.
#[test]
fn unknown_keys_are_refused() {
    let edits = [
        ("\n[node]\n", "\n[node]\nbogus = 1\n"),
        ("\n[node.attestation]\n", "\n[node.attestation]\nbogus = 1\n"),
        ("\n[metrics]\n", "\n[metrics]\nbogus = 1\n"),
        ("\n[account]\n", "\n[account]\nbogus = 1\n"),
        ("\n[strategy]\n", "\n[strategy]\nbogus = 1\n"),
        ("\n[sizing]\n", "\n[sizing]\nbogus = 1\n"),
        ("\n[risk]\n", "\n[risk]\nbogus = 1\n"),
        ("\n[logging]\n", "\n[logging]\nbogus = 1\n"),
        ("\n[logging]\n", "\n[bogus]\n[logging]\n"),
        ("path = \"keys/account.json\", password", "path = \"keys/account.json\", bogus = 1, password"),
        ("{ env = \"TRADER_KEY_PASSWORD\" }", "{ env = \"TRADER_KEY_PASSWORD\", bogus = 1 }"),
    ];
    for (from, to) in edits {
        let text = example();
        assert!(text.contains(from), "{from:?}");
        let err = parse(&text.replacen(from, to, 1)).expect_err(to);
        // serde names an unknown field, or, in the one-key password table, a second key.
        let err = format!("{err:#}");
        assert!(err.contains("unknown") || err.contains("single key"), "{to}: {err}");
    }
}

/// A missing required key fails the load, naming it.
#[test]
fn required_keys_are_required() {
    let keys = ["chain_id = ", "releases = ", "listen = ", "market = ", "max_slippage_bps = ", "kind = ", "by = "];
    for key in keys.into_iter().chain([
        "notional = ",
        "max_leverage = ",
        "max_open_positions = ",
        "max_total_notional = ",
        "daily_loss = ",
    ]) {
        let text: String = example().lines().filter(|l| !l.starts_with(key)).collect::<Vec<_>>().join("\n");
        let err = parse(&text).expect_err(key);
        assert!(format!("{err:#}").contains(key.trim_end_matches(" = ")), "{key}: {err:#}");
    }
}

/// Timeouts default, and the metrics section may be left out.
#[test]
fn optional_keys_default() {
    let text: String = example()
        .lines()
        .filter(|l| !["request_timeout", "stream_timeout", "[metrics]", "listen"].iter().any(|key| l.starts_with(key)))
        .collect::<Vec<_>>()
        .join("\n");
    let cfg = parse(&text).unwrap();
    assert_eq!((cfg.node.request_timeout, cfg.node.stream_timeout), (DEFAULT_REQUEST_TIMEOUT, DEFAULT_STREAM_TIMEOUT));
    assert!(cfg.metrics.is_none());
}

/// A production build refuses each value only a dev build takes, naming it, and takes the example
/// without them.
#[test]
fn dev_only_values_are_refused_in_a_production_build() {
    let dev_only = [
        (KEYSTORE, "key = { type = \"hex_file\", path = \"dev.hex\" }", "account.key"),
        (SEV_SNP, "type = \"mock\"", "node.attestation"),
        (SEV_SNP, &format!("{SEV_SNP}\nroot = \"test-root.pem\""), "node.attestation.root"),
        ("vault = ", &format!("tls_key_sha256 = \"{}\"\nvault = ", "00".repeat(32)), "node.tls_key_sha256"),
    ];
    for (from, to, key) in dev_only {
        let text = example();
        assert!(text.contains(from), "{from}");
        let cfg: Config = toml::from_str(&text.replacen(from, to, 1)).unwrap();
        cfg.validate(true).unwrap_or_else(|e| panic!("{key}: {e:#}"));
        let err = cfg.validate(false).expect_err(key);
        assert!(err.to_string().starts_with(&format!("{key}: ")), "{key}: {err}");
    }
    let cfg: Config = toml::from_str(&example()).unwrap();
    cfg.validate(false).unwrap();
}

/// An SEV-SNP attestation needs a release manifest to match.
#[test]
fn an_sev_snp_attestation_needs_a_release() {
    let err = parse(&example().replace("releases = [\"engine-release.toml\"]", "releases = []")).unwrap_err();
    assert!(err.to_string().contains("node.attestation.releases"), "{err}");
}

/// The metrics are served to this machine alone.
#[test]
fn the_metrics_listen_on_a_loopback_address() {
    for listen in ["127.0.0.1:9466", "127.0.0.2:1", "[::1]:9466"] {
        parse(&example().replace("127.0.0.1:9466", listen)).unwrap_or_else(|e| panic!("{listen}: {e:#}"));
    }
    for listen in ["0.0.0.0:9466", "192.168.1.2:9466", "[::]:9466"] {
        let err = parse(&example().replace("127.0.0.1:9466", listen)).expect_err(listen);
        assert!(err.to_string().contains("metrics.listen"), "{listen}: {err}");
    }
}

/// Each strategy value outside its bounds is refused, and each bound itself is accepted.
#[test]
fn strategy_values_are_checked() {
    let refused = [
        ("fast = 5", "fast = 0"),
        ("fast = 5", "fast = 20"),
        ("slow = 20", "slow = 10001"),
        ("bar = \"1m\"", "bar = \"59s\""),
        ("bar = \"1m\"", "bar = \"90s\""),
        ("bar = \"1m\"", "bar = \"1d 1m\""),
        ("notional = \"1000\"", "notional = \"0\""),
        ("notional = \"1000\"", "notional = \"-1\""),
        ("notional = \"1000\"", "notional = \"1000000000.000000001\""),
        ("max_leverage = \"5\"", "max_leverage = \"0\""),
        ("max_leverage = \"5\"", "max_leverage = \"1001\""),
        ("max_open_positions = 1", "max_open_positions = 0"),
        ("max_total_notional = \"5000\"", "max_total_notional = \"0\""),
        ("daily_loss = { usd = \"100\" }", "daily_loss = { usd = \"0\" }"),
        ("daily_loss = { usd = \"100\" }", "daily_loss = { equity_bps = 10001 }"),
        ("margin_buffer_bps = 10000", "margin_buffer_bps = -1"),
        ("min_hold = \"1h\"", "min_hold = \"169h\""),
        ("kind = \"sma_crossover\"", "kind = \"sma_crosover\""),
        ("max_slippage_bps = 25", "max_slippage_bps = -1"),
        ("max_slippage_bps = 25", "max_slippage_bps = 10001"),
        ("level = \"info\"", "level = \"[\""),
        ("request_timeout = \"10s\"", "request_timeout = \"0s\""),
        ("stream_timeout = \"30s\"", "stream_timeout = \"0s\""),
    ];
    for (from, to) in refused {
        assert!(parse(&example().replacen(from, to, 1)).is_err(), "{to} was accepted");
    }
    let accepted = [
        ("bar = \"1m\"", "bar = \"1d\""),
        ("bar = \"1m\"", "bar = \"15m\""),
        ("notional = \"1000\"", "notional = \"1000000000\""),
        ("max_leverage = \"5\"", "max_leverage = \"1000\""),
        ("daily_loss = { usd = \"100\" }", "daily_loss = { equity_bps = 10000 }"),
        ("min_hold = \"1h\"", "min_hold = \"0s\""),
        ("min_hold = \"1h\"", "min_hold = \"7d\""),
        ("max_slippage_bps = 25", "max_slippage_bps = 0"),
        ("max_slippage_bps = 25", "max_slippage_bps = 10000"),
        ("fast = 5", "fast = 19"),
    ];
    for (from, to) in accepted {
        parse(&example().replacen(from, to, 1)).unwrap_or_else(|e| panic!("{to}: {e:#}"));
    }
}

/// The strategy kinds the example's comment lists each load with their own parameters, and a key one kind
/// does not take is refused.
#[test]
fn every_strategy_kind_loads() {
    let kinds = [
        "kind = \"ema_crossover\"\nfast = 3\nslow = 9",
        "kind = \"macd\"",
        "kind = \"rsi_reversion\"\nperiod = 7\noversold = 25",
        "kind = \"bollinger_reversion\"\nwidth = \"1.5\"",
        "kind = \"donchian_breakout\"\nentry = 30",
    ];
    for kind in kinds {
        let text = example().replacen("kind = \"sma_crossover\"\nfast = 5\nslow = 20", kind, 1);
        parse(&text).unwrap_or_else(|e| panic!("{kind}: {e:#}"));
    }
    let typo = example().replacen("fast = 5", "fasst = 5", 1);
    assert!(parse(&typo).is_err(), "a key the strategy does not take was accepted");
    let other = example().replacen("kind = \"sma_crossover\"\nfast = 5\nslow = 20", "kind = \"macd\"\nfast = 5", 1);
    assert!(parse(&other).is_ok());
    let wrong = example().replacen(
        "kind = \"sma_crossover\"\nfast = 5\nslow = 20",
        "kind = \"donchian_breakout\"\nfast = 5",
        1,
    );
    assert!(parse(&wrong).is_err(), "a key another kind takes was accepted");
}

/// Relative key, password, release and root paths resolve against the config's directory; absolute
/// ones stay.
#[test]
fn relative_paths_resolve_against_the_config() {
    let text = example()
        .replace("password = { env = \"TRADER_KEY_PASSWORD\" }", "password = { file = \"/run/pass\" }")
        .replace(
            SEV_SNP,
            "type = \"sev_snp\"\nreleases = [\"engine-release.toml\", \"/srv/next.toml\"]\nroot = \"roots/test.pem\"",
        );
    let mut cfg: Config = toml::from_str(&text).unwrap();
    cfg.resolve_paths(Path::new("/etc/trader"));
    let KeySource::Keystore { path, password: PasswordSource::File(file) } = &cfg.account.key else { unreachable!() };
    assert_eq!((path.as_path(), file.as_path()), (Path::new("/etc/trader/keys/account.json"), Path::new("/run/pass")));
    let AttestationSection::SevSnp { releases, root: Some(root) } = &cfg.node.attestation else { unreachable!() };
    assert_eq!(releases, &[Path::new("/etc/trader/engine-release.toml"), Path::new("/srv/next.toml")]);
    assert_eq!(root, Path::new("/etc/trader/roots/test.pem"));
}

/// The node URL and the pin must be well formed.
#[test]
fn node_values_are_parsed() {
    let pinned = example().replacen("vault = ", &format!("tls_key_sha256 = \"{}\"\nvault = ", "00".repeat(32)), 1);
    for (from, to) in [
        ("url = \"https://node.monkeytrading.example:443\"", "url = \"http://node.monkeytrading.example:443\""),
        ("tls_key_sha256 = \"00", "tls_key_sha256 = \"zz"),
        ("tls_key_sha256 = \"00", "tls_key_sha256 = \""),
        ("vault = \"0x00", "vault = \"0x0"),
    ] {
        assert!(parse(&pinned.replacen(from, to, 1)).is_err(), "{to} was accepted");
    }
    let cfg = parse(&pinned.replacen("tls_key_sha256 = \"", "tls_key_sha256 = \"0x", 1)).unwrap();
    assert_eq!(cfg.node.tls_key_sha256, Some(TlsPin([0; 32])));
}

/// A production build refuses a dev config at load, with what only a dev build takes.
#[cfg(feature = "production")]
#[test]
fn a_production_build_refuses_a_dev_config_at_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dev.toml");
    std::fs::write(&path, example().replace(SEV_SNP, "type = \"mock\"")).unwrap();
    let err = Config::load(&path).unwrap_err();
    assert!(format!("{err:#}").contains("a mock attestation proves nothing"), "{err:#}");
    Config::load(Path::new(EXAMPLE)).unwrap();
}

#[test]
fn authorities_bracket_ipv6_hosts() {
    assert_eq!(NodeUrl { host: "::1".into(), port: 1 }.authority(), "[::1]:1");
    assert_eq!(NodeUrl { host: "node.example".into(), port: 443 }.authority(), "node.example:443");
}

// ── Helpers ──

/// The example's key and attestation lines, which tests replace.
const KEYSTORE: &str =
    "key = { type = \"keystore\", path = \"keys/account.json\", password = { env = \"TRADER_KEY_PASSWORD\" } }";
const SEV_SNP: &str = "type = \"sev_snp\"\nreleases = [\"engine-release.toml\"]";

/// The example's text.
fn example() -> String {
    std::fs::read_to_string(EXAMPLE).unwrap()
}

/// Parses and checks `text` as a dev build's config.
fn parse(text: &str) -> anyhow::Result<Config> {
    let cfg: Config = toml::from_str(text)?;
    cfg.validate(true)?;
    Ok(cfg)
}
