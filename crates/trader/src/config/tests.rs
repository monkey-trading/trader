use std::path::Path;

use trader_core::risk::LossLimit;

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
    assert_eq!(cfg.node.limits, LimitsSection::default());
    assert_eq!(cfg.plans.len(), 1);
    let strategy = &cfg.plans[0].strategies[0];
    assert_eq!(strategy.market, "BTC-USD");
    assert_eq!(strategy.params, StrategyParams::SmaCrossover { fast: 5, slow: 20 });
    let sizing = Sizing::FixedNotional { notional: "1000".parse().unwrap(), max_leverage: "5".parse().unwrap() };
    assert_eq!((&cfg.sizing, &cfg.plans[0].sizing), (&sizing, &sizing));
    assert_eq!(cfg.risk.max_open_positions, 1);
    assert_eq!(cfg.risk.daily_loss, LossLimit::Usd("100".parse().unwrap()));
    assert_eq!(cfg.plans[0].risk, cfg.risk);
    assert_eq!(cfg.risk.min_hold, Duration::from_secs(3600));
    assert_eq!(cfg.state.dir, Path::new(EXAMPLE).parent().unwrap().join("state"));
    let KeySource::Keystore { path, password: PasswordSource::Env(name) } = &cfg.plans[0].key else {
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
        ("\n[[account]]\n", "\n[[account]]\nbogus = 1\n"),
        ("\n[[account.strategy]]\n", "\n[[account.strategy]]\nbogus = 1\n"),
        ("\n[node.limits]\n", "\n[node.limits]\nbogus = 1\n"),
        ("\n[state]\n", "\n[state]\nbogus = 1\n"),
        ("per_ip = { per_second = 50,", "per_ip = { bogus = 1, per_second = 50,"),
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
    let keys = ["signer = ", "chain_id = ", "market = ", "max_slippage_bps = ", "kind = ", "by = ", "notional = "];
    for key in
        keys.into_iter().chain(["max_leverage = ", "max_open_positions = ", "max_total_notional = ", "daily_loss = "])
    {
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
    assert!(matches!(cfg.account[0].key, KeySource::HexFile { .. }));
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
        ("per_ip = { per_second = 50,", "per_ip = { per_second = 0,"),
        ("burst = 20 }", "burst = 0 }"),
        ("connections = 4", "connections = 0"),
        ("connections = 4", "connections = 17"),
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

/// Relative key and password paths resolve against the config's directory; absolute ones stay.
#[test]
fn relative_paths_resolve_against_the_config() {
    let mut cfg: Config = toml::from_str(
        &example().replace("password = { env = \"TRADER_KEY_PASSWORD\" }", "password = { file = \"/run/pass\" }"),
    )
    .unwrap();
    cfg.resolve_paths(Path::new("/etc/trader"));
    let KeySource::Keystore { path, password: PasswordSource::File(file) } = &cfg.account[0].key else {
        unreachable!()
    };
    assert_eq!((path.as_path(), file.as_path()), (Path::new("/etc/trader/keys/account.json"), Path::new("/run/pass")));
}

/// An account trades each market once, at most `MAX_STRATEGIES` markets, and the config names an
/// account.
#[test]
fn accounts_and_their_strategies_are_checked() {
    let strategy = "\n[[account.strategy]]\nmarket = \"BTC-USD\"\nbar = \"1m\"\nkind = \"sma_crossover\"\nfast = 5\nslow = 20\nmax_slippage_bps = 25\n";
    let twice = format!("{}{strategy}", example());
    let err = parse(&twice).unwrap_err();
    assert!(format!("{err:#}").contains("twice"), "{err:#}");
    let other = strategy.replace("BTC-USD", "ETH-USD");
    parse(&format!("{}{other}", example())).unwrap();
    let many: String = (0..16).map(|i| strategy.replace("BTC-USD", &format!("M{i}-USD"))).collect();
    assert!(parse(&format!("{}{many}", example())).is_err(), "17 markets");
    let head: Vec<_> =
        example().lines().take_while(|l| !l.starts_with("[[account.strategy]]")).map(str::to_owned).collect();
    let err = parse(&format!("{}\nstrategy = []\n", head.join("\n"))).unwrap_err();
    assert!(err.to_string().contains("from 1 to 16"), "{err}");
    let nothing: String =
        example().lines().take_while(|l| !l.starts_with("[[account]]")).collect::<Vec<_>>().join("\n");
    let err = parse(&nothing).unwrap_err();
    assert!(err.to_string().contains("no [[account]]"), "{err}");
}

/// A directory of keystores makes one account for each `.json` file, by file name, with the
/// directory's password and strategies; the accounts listed one by one come first.
#[test]
fn a_directory_of_keystores_is_many_accounts() {
    let dir = tempfile::tempdir().unwrap();
    let keys = dir.path().join("keys");
    std::fs::create_dir(&keys).unwrap();
    for name in ["b.json", "a.json", "notes.txt"] {
        std::fs::write(keys.join(name), "{}").unwrap();
    }
    std::fs::create_dir(keys.join("nested.json")).unwrap();
    let text = format!(
        "{}\n[[account_dir]]\npath = \"keys\"\npassword = {{ file = \"pass\" }}\nclose_on_stop = true\n\
         [[account_dir.strategy]]\nmarket = \"ETH-USD\"\nbar = \"5m\"\nkind = \"sma_crossover\"\nfast = 3\nslow = 9\nmax_slippage_bps = 10\n",
        example()
    );
    let path = dir.path().join("bot.toml");
    std::fs::write(&path, text).unwrap();
    let cfg = Config::load(&path).unwrap();
    let files: Vec<_> = cfg
        .plans
        .iter()
        .map(|plan| match &plan.key {
            KeySource::Keystore { path, password } => {
                (path.file_name().unwrap().to_str().unwrap().to_owned(), matches!(password, PasswordSource::File(_)))
            }
            KeySource::HexFile { .. } => unreachable!(),
        })
        .collect();
    assert_eq!(files, [("account.json".into(), false), ("a.json".into(), true), ("b.json".into(), true)]);
    assert!(cfg.plans[1..].iter().all(|plan| plan.close_on_stop && plan.strategies[0].market == "ETH-USD"));
    let PasswordSource::File(file) = (match &cfg.plans[1].key {
        KeySource::Keystore { password, .. } => password,
        KeySource::HexFile { .. } => unreachable!(),
    }) else {
        unreachable!()
    };
    assert_eq!(file, &dir.path().join("pass"), "the password file is read from the config's directory");
}

/// A directory with no keystore, or that cannot be read, fails the load.
#[test]
fn an_empty_or_missing_directory_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("empty")).unwrap();
    for (name, expected) in [("empty", "no .json keystore"), ("missing", "reading")] {
        let text = format!(
            "{}\n[[account_dir]]\npath = \"{name}\"\npassword = {{ env = \"P\" }}\n[[account_dir.strategy]]\nmarket = \"ETH-USD\"\n\
             bar = \"5m\"\nkind = \"sma_crossover\"\nfast = 3\nslow = 9\nmax_slippage_bps = 10\n",
            example()
        );
        let path = dir.path().join("bot.toml");
        std::fs::write(&path, text).unwrap();
        let err = format!("{:#}", Config::load(&path).unwrap_err());
        assert!(err.contains(expected), "{name}: {err}");
    }
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
