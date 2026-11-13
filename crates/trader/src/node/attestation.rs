//! The node's attestation, checked once before any other call: the enclave address it names is the
//! signer every answer and stream frame must carry, and the TLS key it names the only one the bot's
//! connections to the node accept from then on. `engine_verify` does the verifying, as
//! `engine verify-attestation` does: the quote's certificate chain to a vendor root pinned in that
//! crate, and the release it matches among the manifests the config names. The chain comes from the
//! node's relay (`/v1/attestation/collateral`), which adds no trust, since it is checked against
//! the pinned roots. A mock attestation proves nothing; only a dev build's config takes one.

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use engine_core::types::AccountId;
use engine_verify::{
    Attestation, RelayedCollateral, ReleaseManifest, SnpReport, verify_attestation, verify_mock_attestation,
};
use tracing::{error, info, warn};

use crate::config::{AttestationSection, NodeSection};
use crate::node::{Tls, fetch_unsigned};

/// The node's identity, as its attestation proves it.
pub struct Attested {
    /// The enclave address that signs every answer and stream frame.
    pub signer: [u8; 20],
    /// Connections to the node that accept only the attested TLS key.
    pub tls: Tls,
}

/// Attests the node `cfg` names, as `attest` does, with dates checked at this machine's time. A
/// failure raises the Critical alert `node_attestation_failed`.
/// Error cases:
/// - the system clock is before 1970, or the TLS configuration cannot be built
/// - see `attest`
pub async fn attest_node(cfg: &NodeSection) -> anyhow::Result<Attested> {
    let clock = SystemTime::now().duration_since(UNIX_EPOCH).context("the system clock is before 1970")?;
    let tls = Tls::new(cfg.url.clone(), cfg.tls_key_sha256)?;
    attest(&tls, &cfg.attestation, cfg.request_timeout, clock.as_secs().cast_signed())
        .await
        .inspect_err(|e| error!(alert = "node_attestation_failed", "{e:#}"))
}

/// Fetches the attestation of the node `tls` connects to and checks it as `section` says, with
/// dates checked at `now` in Unix seconds: against the release manifests, or, for a dev node, as
/// the mock it is.
/// Error cases:
/// - the attestation or its collateral cannot be fetched or parsed, or a manifest or the root read
/// - the attestation does not verify (see `engine_verify::verify_attestation`), or is not the mock
///   it must be
/// - the TLS key it names is not the pinned one
pub async fn attest(tls: &Tls, section: &AttestationSection, timeout: Duration, now: i64) -> anyhow::Result<Attested> {
    let served: Attestation = serde_json::from_slice(&fetch_unsigned(tls, "/v1/attestation", timeout).await?)
        .context("the node's attestation is not an attestation")?;
    let (signer, tls_key) = match section {
        AttestationSection::Mock {} => {
            let bundle = verify_mock_attestation(&served).context("the node's attestation is not a mock")?;
            let signer = AccountId(bundle.enclave_address);
            warn!(%signer, "the node's attestation is a mock: nothing proves its signer runs in an enclave");
            (bundle.enclave_address, bundle.tls_public_key_sha256)
        }
        AttestationSection::SevSnp { releases, root } => {
            let releases = releases.iter().map(|path| read_release(path)).collect::<anyhow::Result<Vec<_>>>()?;
            let root = root.as_deref().map(read_root).transpose()?;
            // Only an SEV-SNP report has collateral to fetch; a dev node's mock has none.
            let quote = hex::decode(served.quote.strip_prefix("0x").unwrap_or(&served.quote))
                .context("the node's quote is not hex")?;
            SnpReport::parse(&quote)
                .context("the node's quote is not an SEV-SNP attestation report; a mock quote never verifies")?;
            let collateral = fetch_unsigned(tls, "/v1/attestation/collateral", timeout).await?;
            let collateral = RelayedCollateral::from_json(&collateral).context("the node's attestation collateral")?;
            let verified = verify_attestation(&served, &releases, &collateral, root.as_deref(), now)
                .context("the node's attestation does not verify")?;
            info!(
                signer = %AccountId(verified.enclave_address),
                release = verified.release_name,
                config = verified.config_name,
                "the node's attestation verified"
            );
            (verified.enclave_address, verified.tls_public_key_sha256)
        }
    };
    Ok(Attested { signer, tls: tls.attested(tls_key)? })
}

/// The release manifest at `path`.
/// Error cases:
/// - the file cannot be read, or is not a valid manifest
fn read_release(path: &Path) -> anyhow::Result<ReleaseManifest> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    ReleaseManifest::from_toml(&text).with_context(|| format!("the release manifest {}", path.display()))
}

/// The PEM root certificate at `path`.
/// Error cases:
/// - the file cannot be read
fn read_root(path: &Path) -> anyhow::Result<String> {
    std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
}
