//! The stand-in's TLS keys: the one its attestation names; another it holds, as a node another
//! enclave took over would; and another's certificate it cannot sign for, as a party between the bot
//! and the node replaying one would. Neither of the last two holds the attested key's sessions, so
//! while either is presented nothing resumes.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use rcgen::PublicKeyData;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::server::{ClientHello, ResolvesServerCert, ServerSessionMemoryCache, StoresServerSessions};
use rustls::sign::CertifiedKey;
use sha2::{Digest, Sha256};

/// What a stand-in presents on a new connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Presented {
    /// The attested key.
    Attested,
    /// Another key, which it holds.
    Other,
    /// The other key's certificate, with the handshake signed by the attested key.
    Replayed,
}

/// The keys a stand-in presents, and the attested key's sessions.
#[derive(Debug)]
pub struct Keys {
    attested: Arc<CertifiedKey>,
    other: Arc<CertifiedKey>,
    replayed: Arc<CertifiedKey>,
    sessions: Arc<ServerSessionMemoryCache>,
    presented: AtomicU8,
}

impl Keys {
    /// Two self-signed keys for `localhost`, and the SHA-256 of the first's SubjectPublicKeyInfo.
    pub fn new() -> (Arc<Self>, [u8; 32]) {
        let (attested, pin) = certified();
        let (other, _) = certified();
        let replayed = Arc::new(CertifiedKey::new(other.cert.clone(), Arc::clone(&attested.key)));
        let sessions = ServerSessionMemoryCache::new(256);
        let presented = AtomicU8::new(Presented::Attested as u8);
        (Arc::new(Self { attested, other, replayed, sessions, presented }), pin)
    }

    /// Presents `presented` on every new connection from now on.
    pub fn present(&self, presented: Presented) {
        self.presented.store(presented as u8, Ordering::SeqCst);
    }

    fn attested(&self) -> bool {
        self.presented.load(Ordering::SeqCst) == Presented::Attested as u8
    }
}

impl ResolvesServerCert for Keys {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let presented = self.presented.load(Ordering::SeqCst);
        let key = [&self.attested, &self.other, &self.replayed][usize::from(presented)];
        Some(Arc::clone(key))
    }
}

impl StoresServerSessions for Keys {
    fn put(&self, key: Vec<u8>, value: Vec<u8>) -> bool {
        self.attested() && self.sessions.put(key, value)
    }

    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        if self.attested() { self.sessions.get(key) } else { None }
    }

    fn take(&self, key: &[u8]) -> Option<Vec<u8>> {
        if self.attested() { self.sessions.take(key) } else { None }
    }

    fn can_cache(&self) -> bool {
        true
    }
}

/// A self-signed certificate for `localhost` with its key, and the key's hash.
fn certified() -> (Arc<CertifiedKey>, [u8; 32]) {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let pin: [u8; 32] = Sha256::digest(cert.signing_key.subject_public_key_info()).into();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der()));
    let signing = rustls::crypto::ring::sign::any_supported_type(&key).unwrap();
    (Arc::new(CertifiedKey::new(vec![CertificateDer::from(cert.cert.der().to_vec())], signing)), pin)
}
