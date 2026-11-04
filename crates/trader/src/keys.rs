//! The account key: unlocked once at startup, held only as the signing key, which zeroes its secret
//! when dropped, and never printed. Every copy of its bytes or its password on the way in is zeroed
//! as soon as it has been read.

use std::fmt;
use std::path::Path;

use anyhow::{Context, bail};
use engine_core::eip712::{self, B256, Signature};
use engine_core::types::AccountId;
use k256::ecdsa::SigningKey;
use zeroize::Zeroizing;

use crate::config::{KeySource, PasswordSource};

/// An account's signing key and its address.
pub struct AccountKey {
    key: SigningKey,
    address: AccountId,
}

impl AccountKey {
    /// Unlocks the key `source` names, reading a password from `env` when the source names an
    /// environment variable. A keystore is decrypted on the blocking pool: its key derivation is
    /// slow by design.
    /// Error cases:
    /// - the key file or password cannot be read, or the password variable is unset
    /// - the keystore does not decrypt with the password
    /// - the bytes are not a valid secp256k1 secret key
    pub async fn load(source: &KeySource, env: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        match source {
            KeySource::Keystore { path, password } => {
                let password = read_password(password, env)?;
                let path = path.clone();
                let bytes = tokio::task::spawn_blocking(move || {
                    eth_keystore::decrypt_key(&path, password.as_bytes())
                        .map(Zeroizing::new)
                        .with_context(|| format!("decrypting the keystore {}", path.display()))
                })
                .await
                .context("the keystore task")??;
                Self::from_secret(&bytes)
            }
            KeySource::HexFile { path } => Self::from_secret(&*read_hex_key(path)?),
        }
    }

    /// The key whose secret is `bytes`.
    /// Error cases:
    /// - `bytes` is not 32 bytes of a valid secp256k1 secret key
    fn from_secret(bytes: &[u8]) -> anyhow::Result<Self> {
        let key = SigningKey::from_slice(bytes).map_err(|_| anyhow::anyhow!("not a valid secp256k1 secret key"))?;
        let address = eip712::address(key.verifying_key());
        Ok(Self { key, address })
    }

    /// The account's address.
    pub fn address(&self) -> AccountId {
        self.address
    }

    /// The account's signature over `hash`, an EIP-712 digest from `engine_core::eip712`.
    pub fn sign(&self, hash: &B256) -> Signature {
        eip712::sign(&self.key, hash)
    }
}

impl fmt::Debug for AccountKey {
    /// The address only.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AccountKey({})", self.address)
    }
}

/// The keystore password `source` names, without one trailing newline when read from a file.
/// Error cases:
/// - the file cannot be read, or is not UTF-8
/// - the variable is unset
fn read_password(source: &PasswordSource, env: impl Fn(&str) -> Option<String>) -> anyhow::Result<Zeroizing<String>> {
    let mut password = Zeroizing::new(match source {
        PasswordSource::File(path) => {
            std::fs::read_to_string(path).with_context(|| format!("reading the password file {}", path.display()))?
        }
        PasswordSource::Env(name) => env(name).with_context(|| format!("the password variable {name} is not set"))?,
    });
    if password.ends_with('\n') {
        password.pop();
        if password.ends_with('\r') {
            password.pop();
        }
    }
    Ok(password)
}

/// The 32 key bytes in `path`: 64 hex characters, with or without `0x`, around which whitespace is
/// ignored.
/// Error cases:
/// - the file cannot be read
/// - its content is not 32 bytes of hex
fn read_hex_key(path: &Path) -> anyhow::Result<Zeroizing<[u8; 32]>> {
    let text = Zeroizing::new(std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?);
    let mut bytes = Zeroizing::new([0u8; 32]);
    let hex = text.trim();
    if hex::decode_to_slice(hex.strip_prefix("0x").unwrap_or(hex), &mut *bytes).is_err() {
        bail!("{} does not hold a key as 64 hex characters", path.display());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests;
