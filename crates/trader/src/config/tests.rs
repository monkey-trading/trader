use std::path::Path;

use super::*;

/// The committed example, which the README and the dev stack point to.
const EXAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config/example.toml");

/// The example loads, every relative path resolved against its directory.
#[test]
fn the_example_loads() {
    let cfg = Config::load(Path::new(EXAMPLE)).unwrap();
    assert_eq!(cfg.node.url, NodeUrl { host: "127.0.0.1".into(), port: 18610 });
    assert_eq!(cfg.node.chain_id, 31337);
    assert_eq!(cfg.node.tls_key_sha256, Some(TlsPin([0; 32])));
    assert_eq!(cfg.node.request_timeout, Duration::from_secs(10));
    assert_eq!(cfg.strategy.market, "BTC-USD");
    assert_eq!((cfg.strategy.fast, cfg.strategy.slow), (5, 20));
    assert_eq!(cfg.strategy.qty, "0.01".parse().unwrap());
    let KeySource::Keystore { path, password: PasswordSource::Env(name) } = &cfg.account.key else {
        panic!("the example names a keystore and a password variable");
    };
    assert_eq!(path, &Path::new(EXAMPLE).parent().unwrap().join("keys/account.json"));
    assert_eq!(name, "TRADER_KEY_PASSWORD");
    assert_eq!(cfg.strategy.bounds, Some(SmaBounds { fast: [2, 20], slow: [10, 100] }));
    let console = cfg.console.unwrap();
    assert_eq!(console.bind, "0.0.0.0:8443".parse().unwrap());
    assert_eq!(console.data_dir, Path::new(EXAMPLE).parent().unwrap().join("data"));
    assert_eq!(console.refresh_interval, Duration::from_secs(5));
    assert!(console.tls.is_none());
}

/// Without a `[console]` section the bot serves none; an empty one listens on every interface on
/// port 8443, keeping its data beside the config.
#[test]
fn the_console_defaults_to_every_interface() {
    let without = example().split("\n# The console:").next().unwrap().to_owned();
    assert!(parse(&without).unwrap().console.is_none());
    let cfg = parse(&format!("{without}\n[console]\n")).unwrap();
    let console = cfg.console.unwrap();
    assert_eq!(console.bind, DEFAULT_CONSOLE_BIND);
    assert_eq!(console.bind.to_string(), "0.0.0.0:8443");
    assert_eq!((console.data_dir.as_path(), console.refresh_interval), (Path::new("data"), DEFAULT_REFRESH_INTERVAL));
}

/// The console's values and the period bounds are checked: each range low to high, admitting the
/// periods set, and reads of the node at most once a second.
#[test]
fn console_values_are_checked() {
    for (from, to) in [
        ("fast = [2, 20]", "fast = [6, 4]"),
        ("fast = [2, 20]", "fast = [6, 20]"),
        ("slow = [10, 100]", "slow = [21, 100]"),
        ("slow = [10, 100]", "slow = [10, 19]"),
        ("refresh_interval = \"5s\"", "refresh_interval = \"999ms\""),
        ("bind = \"0.0.0.0:8443\"", "bind = \"8443\""),
    ] {
        assert!(parse(&example().replacen(from, to, 1)).is_err(), "{to} was accepted");
    }
    for (from, to) in [
        ("fast = [2, 20]", "fast = [5, 5]"),
        ("slow = [10, 100]", "slow = [20, 20]"),
        ("refresh_interval = \"5s\"", "refresh_interval = \"1s\""),
        ("bind = \"0.0.0.0:8443\"", "bind = \"[::]:9443\""),
    ] {
        parse(&example().replacen(from, to, 1)).unwrap_or_else(|e| panic!("{to}: {e:#}"));
    }
}

/// The console's data directory and named certificate resolve against the config's directory.
#[test]
fn console_paths_resolve_against_the_config() {
    let text =
        example().replace("# tls = {", "tls = {").replace("data_dir = \"data\"", "data_dir = \"/var/lib/trader\"");
    let mut cfg: Config = toml::from_str(&text).unwrap();
    cfg.resolve_paths(Path::new("/etc/trader"));
    let console = cfg.console.unwrap();
    assert_eq!(console.data_dir, Path::new("/var/lib/trader"));
    let tls = console.tls.unwrap();
    assert_eq!(
        (tls.cert.as_path(), tls.key.as_path()),
        (Path::new("/etc/trader/console.crt"), Path::new("/etc/trader/console.key"))
    );
}

/// A strategy's periods are within its bounds when both are.
#[test]
fn bounds_admit_periods_within_both_ranges() {
    let bounds = SmaBounds { fast: [2, 5], slow: [10, 20] };
    for (fast, slow, admitted) in
        [(2, 10, true), (5, 20, true), (1, 10, false), (6, 10, false), (2, 9, false), (2, 21, false)]
    {
        assert_eq!(bounds.admit(SmaParams { fast, slow }), admitted, "{fast}, {slow}");
    }
}

/// An unknown key is refused in every section, the key and password tables included.
#[test]
fn unknown_keys_are_refused() {
    let edits = [
        ("\n[node]\n", "\n[node]\nbogus = 1\n"),
        ("\n[account]\n", "\n[account]\nbogus = 1\n"),
        ("\n[strategy]\n", "\n[strategy]\nbogus = 1\n"),
        ("\n[logging]\n", "\n[logging]\nbogus = 1\n"),
        ("\n[console]\n", "\n[console]\nbogus = 1\n"),
        ("\n[strategy.bounds]\n", "\n[strategy.bounds]\nbogus = [1, 2]\n"),
        ("# tls = { cert = \"console.crt\", key = \"console.key\" }", "tls = { cert = \"a\", key = \"b\", bogus = 1 }"),
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
    for key in ["signer = ", "chain_id = ", "market = ", "qty = ", "max_slippage_bps = "] {
        let text: String = example().lines().filter(|l| !l.starts_with(key)).collect::<Vec<_>>().join("\n");
        let err = parse(&text).expect_err(key);
        assert!(format!("{err:#}").contains(key.trim_end_matches(" = ")), "{key}: {err:#}");
    }
}

/// Without a pin the node's certificate must chain to a public root; timeouts default.
#[test]
fn optional_node_keys_default() {
    let text: String = example()
        .lines()
        .filter(|l| !["tls_key_sha256", "request_timeout", "stream_timeout"].iter().any(|key| l.starts_with(key)))
        .collect::<Vec<_>>()
        .join("\n");
    let cfg = parse(&text).unwrap();
    assert_eq!(cfg.node.tls_key_sha256, None);
    assert_eq!((cfg.node.request_timeout, cfg.node.stream_timeout), (DEFAULT_REQUEST_TIMEOUT, DEFAULT_STREAM_TIMEOUT));
}

/// A hex key file loads in a dev build only.
#[test]
fn a_hex_key_is_dev_only() {
    let text = example().replace(
        "key = { type = \"keystore\", path = \"keys/account.json\", password = { env = \"TRADER_KEY_PASSWORD\" } }",
        "key = { type = \"hex_file\", path = \"dev.hex\" }",
    );
    let cfg: Config = toml::from_str(&text).unwrap();
    assert!(matches!(cfg.account.key, KeySource::HexFile { .. }));
    cfg.validate(true).unwrap();
    let err = cfg.validate(false).unwrap_err();
    assert!(err.to_string().contains("only in a dev build"), "{err}");
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
        ("qty = \"0.01\"", "qty = \"0\""),
        ("qty = \"0.01\"", "qty = \"-1\""),
        ("qty = \"0.01\"", "qty = \"1000000000.000000001\""),
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
        ("qty = \"0.01\"", "qty = \"1000000000\""),
        ("max_slippage_bps = 25", "max_slippage_bps = 0"),
        ("max_slippage_bps = 25", "max_slippage_bps = 10000"),
        ("fast = 5", "fast = 19"),
    ];
    for (from, to) in accepted {
        parse(&example().replacen(from, to, 1)).unwrap_or_else(|e| panic!("{to}: {e:#}"));
    }
}

/// Relative key and password paths resolve against the config's directory; absolute ones stay.
#[test]
fn relative_paths_resolve_against_the_config() {
    let mut cfg: Config = toml::from_str(
        &example().replace("password = { env = \"TRADER_KEY_PASSWORD\" }", "password = { file = \"/run/pass\" }"),
    )
    .unwrap();
    cfg.resolve_paths(Path::new("/etc/trader"));
    let KeySource::Keystore { path, password: PasswordSource::File(file) } = &cfg.account.key else { unreachable!() };
    assert_eq!((path.as_path(), file.as_path()), (Path::new("/etc/trader/keys/account.json"), Path::new("/run/pass")));
}

/// The node URL and the pin must be well formed.
#[test]
fn node_values_are_parsed() {
    for (from, to) in [
        ("url = \"https://127.0.0.1:18610\"", "url = \"http://127.0.0.1:18610\""),
        ("tls_key_sha256 = \"00", "tls_key_sha256 = \"zz"),
        ("tls_key_sha256 = \"00", "tls_key_sha256 = \""),
        ("vault = \"0x00", "vault = \"0x0"),
    ] {
        assert!(parse(&example().replacen(from, to, 1)).is_err(), "{to} was accepted");
    }
    let cfg = parse(&example().replacen("tls_key_sha256 = \"", "tls_key_sha256 = \"0x", 1)).unwrap();
    assert_eq!(cfg.node.tls_key_sha256, Some(TlsPin([0; 32])));
}

#[test]
fn authorities_bracket_ipv6_hosts() {
    assert_eq!(NodeUrl { host: "::1".into(), port: 1 }.authority(), "[::1]:1");
    assert_eq!(NodeUrl { host: "node.example".into(), port: 443 }.authority(), "node.example:443");
}

// ── Helpers ──

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
