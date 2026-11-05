//! The node's attestation against the stand-in: a quote the engine's stand-in vendor chain signed
//! verifies against the release manifest the config names, and the bot trades against the signer
//! it names; a mock, a quote no release or trusted root admits, an attestation of another TLS key,
//! and one of another signer each stop it before it reads anything else or sends anything.

mod common;

use std::path::Path;
use std::time::Duration;

use common::{
    MINUTE, MOCK_ATTESTATION, SevSnp, StandIn, T0, account, account_view, btc_usd, bucket, check_order, command,
    config_with, order, prices, run, sev_snp,
};
use serde_json::json;
use tokio::sync::oneshot;
use trader::config::Config;

/// The SEV-SNP attestation of a config beside the release manifest and root `load` writes.
const VERIFIED: &str = r#"{ type = "sev_snp", releases = ["release.toml"], root = "root.pem" }"#;

/// With the engine's verifier fixtures, the stand-in's test-signed attestation verifies against the
/// fixture release, and the bot trades against the signer it names, as against any node.
#[tokio::test]
async fn a_verified_attestation_names_the_signer_the_bot_trades_against() {
    let stand_in = StandIn::start().await;
    let quote = sev_snp(stand_in.signer(), stand_in.pin);
    serve(&stand_in, &quote);
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99"))]);
        i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({})));
        i.head_seq = 10;
        i.nonce = 3;
        i.marks = vec![bucket(T0, "100")];
    });
    let dir = tempfile::tempdir().unwrap();
    let cfg = load(&stand_in, dir.path(), &quote, VERIFIED);
    let (stop, stopped) = oneshot::channel::<()>();
    let bot = tokio::spawn(run(cfg, async {
        _ = stopped.await;
    }));

    // Minute 0 closes at 100 and minute 1 at 101, so the target is long and the bot buys.
    stand_in.append((11, 0), command(11, T0, prices(1, "100", T0), json!([])));
    stand_in.append((12, 0), command(12, T0 + 1_001, prices(1, "100", T0 + 1_001), json!([])));
    stand_in.append((13, 0), command(13, T0 + MINUTE, prices(1, "101", T0 + MINUTE), json!([])));
    stand_in.append((14, 0), command(14, T0 + 2 * MINUTE, prices(1, "102", T0 + 2 * MINUTE), json!([])));
    check_order(&order(&stand_in, 1).await, T0 + MINUTE, "buy", "0.980392156", "102", false, 3);

    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap();
}

/// Each attestation the config does not admit stops the bot at startup with its reason, before it
/// reads the market, the account or the nonce.
#[tokio::test]
async fn an_attestation_the_config_does_not_admit_stops_the_bot_first() {
    let stand_in = StandIn::start().await;
    let (signer, pin) = (stand_in.signer(), stand_in.pin);
    let mock = stand_in.with(|i| i.attestation.clone());
    let quote = || sev_snp(signer, pin);
    let other_release = SevSnp { release: quote().release.replace(&"1d".repeat(48), &"2e".repeat(48)), ..quote() };
    let cases = [
        // A dev node's mock where a quote must verify.
        (None, VERIFIED, "a mock quote never verifies"),
        // A quote where the config takes a mock.
        (Some(quote()), r#"{ type = "mock" }"#, "not a mock"),
        // A quote whose chain ends at no root the bot trusts.
        (Some(quote()), r#"{ type = "sev_snp", releases = ["release.toml"] }"#, "does not verify"),
        // A quote of another release.
        (Some(other_release), VERIFIED, "matches no release launch digest"),
        // A verified quote of another TLS key than the node's.
        (Some(sev_snp(signer, [7; 32])), VERIFIED, "names the TLS key"),
    ];
    for (served, attestation, reason) in cases {
        match &served {
            Some(served) => serve(&stand_in, served),
            None => stand_in.with(|i| (i.attestation, i.collateral) = (mock.clone(), None)),
        }
        let dir = tempfile::tempdir().unwrap();
        let cfg = load(&stand_in, dir.path(), &served.unwrap_or_else(quote), attestation);
        let err = within(run(cfg, async {})).await.expect_err(reason);
        assert!(format!("{err:#}").contains(reason), "{reason}: {err:#}");
        assert_eq!(stand_in.with(|i| (i.market_reads, i.account_reads, i.nonce_reads)), (0, 0, 0), "{reason}");
    }
}

/// An attestation that verifies but names another signer than the one signing the node's answers
/// stops the bot at its first verified read, before it sends anything.
#[tokio::test]
async fn an_attestation_of_another_signer_stops_the_bot_before_it_sends() {
    let stand_in = StandIn::start().await;
    let quote = sev_snp(stand_in.other_signer(), stand_in.pin);
    serve(&stand_in, &quote);
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("100"))]);
        i.accounts.insert(account().to_string(), account_view("5", json!({}), json!({})));
    });
    let dir = tempfile::tempdir().unwrap();
    let err = within(run(load(&stand_in, dir.path(), &quote, VERIFIED), async {})).await.unwrap_err();
    assert!(format!("{err:#}").contains("not signed by the attested signer"), "{err:#}");
    assert!(stand_in.with(|i| i.posts.is_empty()));
}

// ── Helpers ──

/// `run`'s result, or a panic after twenty seconds.
async fn within(run: impl Future<Output = anyhow::Result<()>>) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(20), run).await.expect("the bot stopped within twenty seconds")
}

/// Serves `quote` from the stand-in.
fn serve(stand_in: &StandIn, quote: &SevSnp) {
    stand_in.with(|i| (i.attestation, i.collateral) = (quote.attestation.clone(), Some(quote.collateral.clone())));
}

/// The stand-in's config in `dir` with `attestation`, beside `quote`'s release manifest and root.
fn load(stand_in: &StandIn, dir: &Path, quote: &SevSnp, attestation: &str) -> Config {
    std::fs::write(dir.join("release.toml"), &quote.release).unwrap();
    std::fs::write(dir.join("root.pem"), &quote.root).unwrap();
    config_with(stand_in, dir, "1m", &[(MOCK_ATTESTATION, &format!("attestation = {attestation}"))])
}
