//! The TLS connections the bot makes to the node: TLS 1.3, accepted when the server's key hashes to
//! the configured pin (the SHA-256 of its DER SubjectPublicKeyInfo, which the node's attestation
//! names), or, with no pin, when its certificate chains to a public root.

use std::sync::Arc;

use anyhow::Context;
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

/// Opens TLS connections to one node.
#[derive(Clone)]
pub struct Tls {
    url: NodeUrl,
    connector: TlsConnector,
}

/// Accepts a server certificate only when its key hashes to `pin`.
#[derive(Debug)]
struct PinnedKey {
    pin: [u8; 32],
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for PinnedKey {
    /// Accepts the certificate when the SHA-256 of its SubjectPublicKeyInfo is the pin.
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let cert = EndEntityCert::try_from(end_entity).map_err(|e| rustls::Error::General(e.to_string()))?;
        let hash: [u8; 32] = Sha256::digest(cert.subject_public_key_info()).into();
        if hash != self.pin {
            return Err(rustls::Error::General("the node's TLS key is not the pinned one".into()));
        }
        Ok(ServerCertVerified::assertion())
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
        let provider = Arc::new(default_provider());
        let builder = ClientConfig::builder_with_provider(provider.clone())
            .with_protocol_versions(&[&TLS13])
            .context("TLS 1.3")?;
        let config = match pin {
            Some(TlsPin(pin)) => {
                builder.dangerous().with_custom_certificate_verifier(Arc::new(PinnedKey { pin, provider }))
            }
            None => {
                builder.with_root_certificates(RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned()))
            }
        }
        .with_no_client_auth();
        Ok(Self { url, connector: TlsConnector::from(Arc::new(config)) })
    }

    /// The node's address.
    pub fn url(&self) -> &NodeUrl {
        &self.url
    }

    /// A new connection to the node.
    /// Error cases:
    /// - the host is not a host name or an IP address
    /// - the connection fails, or the handshake does, including when the key is not the pinned one
    pub async fn connect(&self) -> anyhow::Result<TlsStream<TcpStream>> {
        let NodeUrl { host, port } = &self.url;
        let name = ServerName::try_from(host.clone()).with_context(|| format!("{host} is not a host name"))?;
        let tcp = TcpStream::connect((host.as_str(), *port)).await?;
        tcp.set_nodelay(true)?;
        Ok(self.connector.connect(name, tcp).await?)
    }
}
