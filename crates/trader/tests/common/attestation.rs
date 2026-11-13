//! The attestations the stand-in serves: a dev node's mock, and an SEV-SNP quote signed by the
//! engine's stand-in vendor chain (`engine_verify::fixtures`), with the release manifest and root a
//! config needs to verify it.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::http::header::CONTENT_TYPE;
use axum::response::{IntoResponse, Response};
use engine_verify::fixtures::{
    LAUNCH_DIGEST, REPORTED_TCB, attestation, collateral_of, crl_revoking, enclave, forged_report, pem, release_toml,
    test_chain,
};
use engine_verify::{AttestationBundle, MOCK_QUOTE_MARKER};
use serde_json::{Value, json};

use super::StandIn;

/// The configuration hash the fixture's release manifest lists.
const CONFIG_SHA256: [u8; 32] = [0xc0; 32];

/// A test-signed SEV-SNP attestation and what verifying it takes.
pub struct SevSnp {
    /// What `/v1/attestation` serves.
    pub attestation: Value,
    /// What `/v1/attestation/collateral` serves.
    pub collateral: Vec<u8>,
    /// The release manifest the quote matches.
    pub release: String,
    /// The stand-in vendor chain's root, as PEM.
    pub root: String,
}

/// A dev node's mock attestation, naming `signer` and the TLS key hash `tls`.
pub fn mock_attestation(signer: [u8; 20], tls: [u8; 32]) -> Value {
    let bundle = AttestationBundle { enclave_address: signer, config_sha256: [0; 32], tls_public_key_sha256: tls };
    let quote = [MOCK_QUOTE_MARKER, &bundle.report_data()[..]].concat();
    let mut served = serde_json::to_value(attestation(&bundle, &quote)).unwrap();
    served["provider"] = "mock".into();
    served
}

/// An SEV-SNP attestation of an enclave whose address is `signer` and whose TLS key hashes to
/// `tls`, from a chip of a fresh stand-in vendor chain, at the fixture release's launch digest.
pub fn sev_snp(signer: [u8; 20], tls: [u8; 32]) -> SevSnp {
    let chip_id = enclave().chip_id;
    let chain = test_chain(chip_id, REPORTED_TCB);
    let bundle =
        AttestationBundle { enclave_address: signer, config_sha256: CONFIG_SHA256, tls_public_key_sha256: tls };
    let quote = forged_report(&chain, LAUNCH_DIGEST, bundle.report_data(), chip_id);
    SevSnp {
        attestation: serde_json::to_value(attestation(&bundle, &quote)).unwrap(),
        collateral: collateral_of(&chain, &crl_revoking(&chain, &[])).to_json(),
        release: release_toml(),
        root: pem(&chain.ark),
    }
}

/// `GET /v1/attestation`.
pub(super) async fn attestation_route(State(s): State<Arc<StandIn>>) -> Response {
    axum::Json(s.with(|i| i.attestation.clone())).into_response()
}

/// `GET /v1/attestation/collateral`: 404 without collateral, as a mock node answers.
pub(super) async fn collateral_route(State(s): State<Arc<StandIn>>) -> Response {
    match s.with(|i| i.collateral.clone()) {
        Some(collateral) => ([(CONTENT_TYPE, "application/json")], collateral).into_response(),
        None => (StatusCode::NOT_FOUND, axum::Json(json!({ "error": "not found" }))).into_response(),
    }
}
