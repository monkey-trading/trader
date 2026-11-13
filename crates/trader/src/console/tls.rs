//! The console's TLS identity: the certificate and key the config names, or a self-signed pair the
//! bot generates on first run and keeps in its data directory. A browser cannot check a
//! self-signed certificate, so the bot prints its SHA-256 fingerprint at every start for the user
//! to compare with the one the browser shows.

use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, bail};
use rustls::ServerConfig;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use sha2::{Digest, Sha256};

use crate::config::ConsoleSection;
use crate::files::write_private;

/// The generated certificate and its key, in one PEM file, so neither is ever written without the
/// other.
pub const GENERATED: &str = "console.pem";

/// What the console serves TLS with.
pub struct Identity {
    pub config: Arc<ServerConfig>,
    /// The SHA-256 of the leaf certificate's DER.
    pub fingerprint: [u8; 32],
}

/// The identity `cfg` names, or the generated one in its data directory, generated now when there
/// is none. Returns whether it was generated now.
/// Error cases:
/// - a named or generated file cannot be read, or holds no certificate or no key
/// - the certificate cannot be generated or written
/// - rustls refuses the certificate and key
pub fn identity(cfg: &ConsoleSection) -> anyhow::Result<(Identity, bool)> {
    let (chain, key, generated) = match &cfg.tls {
        Some(tls) => (read_chain(&tls.cert)?, read_key(&tls.key)?, false),
        None => {
            let path = cfg.data_dir.join(GENERATED);
            let generated = !path.exists();
            if generated {
                generate(&path, cfg.bind.ip())?;
            }
            (read_chain(&path)?, read_key(&path)?, generated)
        }
    };
    let fingerprint = Sha256::digest(&chain[0]).into();
    let mut config = ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .context("TLS versions")?
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .context("the console's certificate and key")?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok((Identity { config: Arc::new(config), fingerprint }, generated))
}

/// A fingerprint as browsers show it: upper-case hex bytes joined by colons.
pub fn fingerprint_text(fingerprint: &[u8; 32]) -> String {
    fingerprint.iter().map(|byte| format!("{byte:02X}")).collect::<Vec<_>>().join(":")
}

/// Writes a self-signed certificate for `localhost`, the loopback addresses and `bind`'s address
/// when it names one, with its new ECDSA P-256 key, to `path`.
/// Error cases:
/// - rcgen cannot generate it, or the file cannot be written
fn generate(path: &Path, bind: IpAddr) -> anyhow::Result<()> {
    let mut names = vec!["localhost".to_owned(), "127.0.0.1".to_owned(), "::1".to_owned()];
    if !bind.is_unspecified() && !bind.is_loopback() {
        names.push(bind.to_string());
    }
    let certified = rcgen::generate_simple_self_signed(names).context("generating the console's certificate")?;
    let pem = format!("{}{}", certified.cert.pem(), certified.signing_key.serialize_pem());
    write_private(path, pem.as_bytes())
}

/// The certificates in the PEM file at `path`, leaf first.
/// Error cases:
/// - the file cannot be read, or holds no certificate
fn read_chain(path: &Path) -> anyhow::Result<Vec<CertificateDer<'static>>> {
    let chain = CertificateDer::pem_file_iter(path)
        .and_then(|certs| certs.collect::<Result<Vec<_>, _>>())
        .with_context(|| format!("reading the certificates in {}", path.display()))?;
    if chain.is_empty() {
        bail!("{} holds no certificate", path.display());
    }
    Ok(chain)
}

/// The private key in the PEM file at `path`.
/// Error cases:
/// - the file cannot be read, or holds no private key
fn read_key(path: &Path) -> anyhow::Result<PrivateKeyDer<'static>> {
    PrivateKeyDer::from_pem_file(path).with_context(|| format!("reading the private key in {}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::config::ConsoleTls;

    /// The first run generates the pair, readable by the bot's user alone; later runs serve the same
    /// one, with the same fingerprint, the SHA-256 of the certificate's DER.
    #[test]
    fn generates_once_then_keeps() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = section(dir.path(), None);
        let (first, generated) = identity(&cfg).unwrap();
        assert!(generated);
        let path = dir.path().join(GENERATED);
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let der = read_chain(&path).unwrap().remove(0);
        assert_eq!(first.fingerprint, <[u8; 32]>::from(Sha256::digest(&der)));
        let (second, generated) = identity(&cfg).unwrap();
        assert!(!generated);
        assert_eq!(second.fingerprint, first.fingerprint);
    }

    /// Named files are served instead, and nothing is generated.
    #[test]
    fn serves_named_files() {
        let dir = tempfile::tempdir().unwrap();
        let certified = rcgen::generate_simple_self_signed(vec!["console.example".into()]).unwrap();
        std::fs::write(dir.path().join("c.pem"), certified.cert.pem()).unwrap();
        std::fs::write(dir.path().join("k.pem"), certified.signing_key.serialize_pem()).unwrap();
        let tls = ConsoleTls { cert: dir.path().join("c.pem"), key: dir.path().join("k.pem") };
        let (identity, generated) = identity(&section(dir.path(), Some(tls))).unwrap();
        assert!(!generated);
        assert_eq!(identity.fingerprint, <[u8; 32]>::from(Sha256::digest(certified.cert.der())));
        assert!(!dir.path().join(GENERATED).exists());
    }

    /// A file without a certificate, or without a key, is refused rather than replaced.
    #[test]
    fn refuses_a_damaged_file() {
        let dir = tempfile::tempdir().unwrap();
        let certified = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let path = dir.path().join(GENERATED);
        for pem in [certified.cert.pem(), certified.signing_key.serialize_pem()] {
            std::fs::write(&path, &pem).unwrap();
            assert!(identity(&section(dir.path(), None)).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), pem);
        }
    }

    #[test]
    fn fingerprints_read_as_browsers_show_them() {
        let mut fingerprint = [0u8; 32];
        fingerprint[0] = 0xab;
        fingerprint[31] = 0x01;
        let text = fingerprint_text(&fingerprint);
        assert!(text.starts_with("AB:00:") && text.ends_with(":00:01"), "{text}");
        assert_eq!(text.len(), 32 * 3 - 1);
    }

    // ── Helpers ──

    /// A console section keeping its data in `dir`.
    fn section(dir: &Path, tls: Option<ConsoleTls>) -> ConsoleSection {
        ConsoleSection {
            bind: "127.0.0.1:0".parse().unwrap(),
            data_dir: dir.to_owned(),
            tls,
            refresh_interval: std::time::Duration::from_secs(5),
        }
    }
}
