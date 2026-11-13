//! What a production build refuses at config load, the metrics address, and the release manifests.

use super::*;

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

/// A production build reaches the chain only over TLS: a plain `http://` endpoint, which only a local
/// dev chain serves, is refused without its URL.
#[test]
fn a_production_build_takes_only_https_chain_endpoints() {
    let local = "rpc = [\"https://arbitrum.example/rpc\", \"http://127.0.0.1:8546/k3y\"]";
    let text = example().replacen("rpc = [\"https://arbitrum.example/rpc\"]", local, 1);
    assert_ne!(text, example(), "the example names one endpoint");
    let cfg: Config = toml::from_str(&text).unwrap();
    cfg.validate(true).unwrap();
    let err = cfg.validate(false).expect_err("http:// in a production build");
    assert!(err.to_string().starts_with("chain.rpc: "), "{err}");
    assert!(!err.to_string().contains("k3y"), "{err}");
}
