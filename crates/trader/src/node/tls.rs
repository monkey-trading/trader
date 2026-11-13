//! The TLS connections the bot makes to the node: TLS 1.3, accepted when the server's key hashes to
//! the configured pin (the SHA-256 of its DER SubjectPublicKeyInfo, which the node's attestation
//! names), or, with no pin, when its certificate chains to a public root. Once the node's
//! attestation is verified, its connections must also present the TLS key the attestation names: a
//! node that presents another is not the attested enclave (`KeyMismatch`), whatever signs its answers.
//! The key is compared once the handshake is done, when the server has signed it with that key: a
//! certificate is public, so one presented by a server that cannot sign with its key proves nothing,
//! and fails the handshake as any outage does.

use std::sync::Arc;

use anyhow::{Context, ensure};
use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::ring::default_provider;
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::version::TLS13;
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use sha2::{Digest, Sha256};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use webpki::EndEntityCert;

use crate::config::{NodeUrl, TlsPin};

/// The node completed a TLS handshake with another key than the pinned or attested one, so it holds
/// that key: another enclave, or a host in its place. A connection that fails so is not an outage to
/// retry.
#[derive(Debug, thiserror::Error)]
#[error("the node's TLS key is not the pinned or attested one")]
pub struct KeyMismatch;

/// Opens TLS connections to one node.
#[derive(Clone)]
pub struct Tls {
    url: NodeUrl,
    connector: TlsConnector,
    /// The key the node must present: the configured pin, or the attested key.
    key: Option<[u8; 32]>,
}

/// Accepts a server certificate that chains to a public root for the node's name when `public` is
/// set. Its key is not checked here, before the server has signed the handshake with it: `Tls::connect`
/// compares it once the handshake is done, and a `Tls` with no `public` always has a key to compare.
#[derive(Debug)]
struct NodeCert {
    public: Option<Arc<WebPkiServerVerifier>>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for NodeCert {
    /// Accepts the certificate when it chains to a public root for the node's name, if required.
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match &self.public {
            Some(public) => public.verify_server_cert(end_entity, intermediates, server_name, ocsp, now),
            None => Ok(ServerCertVerified::assertion()),
        }
    }

    /// Checks the handshake signature with the certificate's key.
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    /// Checks the handshake signature with the certificate's key.
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    /// Every scheme the provider verifies.
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

impl Tls {
    /// Connections to `url`, pinned to `pin` when given, else checked against the public roots.
    /// Error cases:
    /// - the TLS 1.3 configuration cannot be built
    pub fn new(url: NodeUrl, pin: Option<TlsPin>) -> anyhow::Result<Self> {
        Self::build(url, pin.map(|TlsPin(pin)| pin), pin.is_none())
    }

    /// The same connections, accepted only when the node presents `key`, the TLS key its verified
    /// attestation names: pinned connections to a pin that is that key, or connections checked
    /// against the public roots that must now present it too.
    /// Error cases:
    /// - the pin is another key
    /// - the TLS 1.3 configuration cannot be built
    pub fn attested(&self, key: [u8; 32]) -> anyhow::Result<Self> {
        match self.key {
            Some(pin) => {
                ensure!(
                    pin == key,
                    "the node's attestation names the TLS key {}, not the pinned {}",
                    hex::encode(key),
                    hex::encode(pin)
                );
                Ok(self.clone())
            }
            None => Self::build(self.url.clone(), Some(key), true),
        }
    }

    /// Connections to `url` that must present `key` when given, and whose certificate must chain to
    /// a public root when `public` is set.
    /// Error cases:
    /// - the TLS 1.3 configuration cannot be built
    fn build(url: NodeUrl, key: Option<[u8; 32]>, public: bool) -> anyhow::Result<Self> {
        let provider = Arc::new(default_provider());
        let roots = Arc::new(RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned()));
        let public = public
            .then(|| WebPkiServerVerifier::builder_with_provider(roots, provider.clone()).build())
            .transpose()
            .context("the public roots")?;
        ensure!(public.is_some() || key.is_some(), "a node connection needs a public root or a key");
        let verifier = NodeCert { public, provider: provider.clone() };
        // Resumption stays on: a resumed session proves the secret of a handshake that presented the
        // key, which a node holding another key never had, so it takes a full handshake and fails.
        let config = ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&TLS13])
            .context("TLS 1.3")?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier))
            .with_no_client_auth();
        Ok(Self { url, connector: TlsConnector::from(Arc::new(config)), key })
    }

    /// The node's address.
    pub fn url(&self) -> &NodeUrl {
        &self.url
    }

    /// A new connection to the node.
    /// Error cases:
    /// - the host is not a host name or an IP address
    /// - the connection fails, or the handshake does
    /// - the node completes the handshake with another key than the pinned or attested one: `KeyMismatch`
    pub async fn connect(&self) -> anyhow::Result<TlsStream<TcpStream>> {
        let NodeUrl { host, port } = &self.url;
        let name = ServerName::try_from(host.clone()).with_context(|| format!("{host} is not a host name"))?;
        let tcp = TcpStream::connect((host.as_str(), *port)).await?;
        tcp.set_nodelay(true)?;
        let stream = self.connector.connect(name, tcp).await?;
        // Nothing has been sent yet: the handshake is done, and the server signed it with this key.
        if let Some(key) = self.key {
            presents(stream.get_ref().1.peer_certificates(), key)?;
        }
        Ok(stream)
    }
}

/// Checks that `certificates`, a completed handshake's, start with one whose key hashes to `key`.
/// Error cases:
/// - another key: `KeyMismatch`
/// - no certificate, or one that cannot be read
fn presents(certificates: Option<&[CertificateDer<'_>]>, key: [u8; 32]) -> anyhow::Result<()> {
    let end_entity = certificates.and_then(<[_]>::first).context("the node presented no certificate")?;
    let cert = EndEntityCert::try_from(end_entity).map_err(|e| anyhow::anyhow!("the node's certificate: {e}"))?;
    let hash: [u8; 32] = Sha256::digest(cert.subject_public_key_info()).into();
    ensure!(hash == key, KeyMismatch);
    Ok(())
}

/// Whether `error`, or any error under it, is a `KeyMismatch`.
pub fn is_mismatch(error: &anyhow::Error) -> bool {
    error.chain().any(|e| e.is::<KeyMismatch>())
}

#[cfg(test)]
mod tests {
    use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair, PublicKeyData};

    use super::*;

    /// A certificate for `localhost` and its key's hash, issued by `issuer` or self-signed.
    fn leaf(issuer: Option<(&CertificateParams, &KeyPair)>) -> (CertificateDer<'static>, [u8; 32]) {
        let key = KeyPair::generate().unwrap();
        let params = CertificateParams::new(vec!["localhost".into()]).unwrap();
        let cert = match issuer {
            Some((params_of, key_of)) => params.signed_by(&key, &Issuer::from_params(params_of, key_of)).unwrap(),
            None => params.self_signed(&key).unwrap(),
        };
        (cert.der().clone(), Sha256::digest(key.subject_public_key_info()).into())
    }

    /// The verifier requires the chain when it has roots, and leaves the key to the completed
    /// handshake; that requires the key it is given, and any certificate otherwise.
    #[test]
    fn a_node_certificate_needs_its_chain_and_its_key() {
        let provider = Arc::new(default_provider());
        let root_key = KeyPair::generate().unwrap();
        let mut root_params = CertificateParams::new(Vec::new()).unwrap();
        root_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let root = root_params.self_signed(&root_key).unwrap();
        let mut roots = RootCertStore::empty();
        roots.add(root.der().clone()).unwrap();
        let public = WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider.clone()).build().unwrap();
        let (chained, chained_key) = leaf(Some((&root_params, &root_key)));
        let (unchained, unchained_key) = leaf(None);
        let name = ServerName::try_from("localhost").unwrap();
        let chains = |public: Option<&Arc<WebPkiServerVerifier>>, cert: &CertificateDer<'_>| {
            let verifier = NodeCert { public: public.cloned(), provider: provider.clone() };
            verifier.verify_server_cert(cert, &[], &name, &[], UnixTime::now()).is_ok()
        };
        assert!(chains(Some(&public), &chained));
        assert!(!chains(Some(&public), &unchained));
        assert!(chains(None, &unchained));
        presents(Some(&[chained.clone(), unchained.clone()]), chained_key).unwrap();
        let other = presents(Some(&[chained]), unchained_key).unwrap_err();
        assert!(is_mismatch(&other), "{other}");
        let none = presents(None, unchained_key).unwrap_err();
        assert!(!is_mismatch(&none), "{none}");
    }

    /// Pinned connections take only an attested key that is the pin; publicly rooted ones take any,
    /// and require it from then on.
    #[test]
    fn attested_connections_present_the_attested_key() {
        let url = NodeUrl { host: "localhost".into(), port: 1 };
        let pinned = Tls::new(url.clone(), Some(TlsPin([1; 32]))).unwrap();
        assert_eq!(pinned.attested([1; 32]).unwrap().key, Some([1; 32]));
        let err = pinned.attested([2; 32]).err().unwrap();
        assert!(err.to_string().contains(&format!("names the TLS key {}", "02".repeat(32))), "{err}");
        let public = Tls::new(url, None).unwrap();
        assert_eq!(public.key, None);
        assert_eq!(public.attested([2; 32]).unwrap().key, Some([2; 32]));
    }
}
