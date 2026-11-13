//! The account key: unlocked once at startup, held only as the signing key, which zeroes its secret
//! when dropped, and never printed. Every copy of its bytes or its password on the way in is zeroed
//! as soon as it has been read.

use std::fmt;
use std::path::Path;

use anyhow::{Context, anyhow, bail};
use engine_core::eip712::{self, B256, Signature};
use engine_core::types::AccountId;
use k256::ecdsa::SigningKey;
use tokio::task::spawn_blocking;
use zeroize::Zeroizing;

use crate::PRODUCTION_BUILD;
use crate::config::{KeySource, PasswordSource};

/// The accounts whose private keys this repository publishes, for its dev scenario and its tests.
/// Anyone can sign for them, so only a dev build takes them.
const PUBLISHED_ACCOUNTS: [&str; 16] = [
    // scripts/dev-trade-one.sh: Anvil's account 1, and scripts/dev-strategies.sh: Anvil's accounts 1 to 7;
    // scripts/dev-lifecycle.sh: Anvil's accounts 2, 3 and 4, and 5, its funding key.
    "0x70997970c51812dc3a010c7d01b50e0d17dc79c8",
    "0x3c44cdddb6a900fa2b585dd299e03d12fa4293bc",
    "0x90f79bf6eb2c4f870365e785982e1f101e93b906",
    "0x15d34aaf54267db7d7c367839aaf71a00a2c6a65",
    "0x9965507d1a55bcc2695c58ba16fb37d819b0a4dc",
    "0x976ea74026e726554db657fa54763abd0c3a0aa9",
    "0x14dc79964da2c08b23698b3d3cc7ca32193d9955",
    // scripts/dev-stack.sh --chain: Anvil's account 0, which deploys the contracts.
    "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266",
    // scripts/dev-stack.sh: the engine's dev operator key, 32 bytes of 0x42.
    "0x17c5185167401ed00cf5f5b2fc97d9bbfdb7d025",
    // The tests' keys: 32 bytes of 0x11, 0x21, 0x51 and 0x52, the secrets 1 and 0x123456789abc, and
    // the Web3 Secret Storage test vectors' key.
    "0x19e7e376e7c213b7e7e7e46cc70a5dd086daff2a",
    "0x2bd0c9fe079c8fca0e3352eb3d02839c371e5c41",
    "0xfa17c5b66a985f44bc249c8e8fabd64767f414b1",
    "0x83279fae0994aa1a563377e889cc2a1d96adb3b1",
    "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf",
    "0xa13d31fec618d56e347aa2b5e83258010548b03e",
    "0x008aeeda4d805471df9b2a5b0f38a0c3bcba786b",
];

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
    /// - in a production build, the key is one this repository publishes
    pub async fn load(source: &KeySource, env: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let secret: Zeroizing<Vec<u8>> = match source {
            KeySource::Keystore { path, password } => {
                let password = read_password(password, env)?;
                let path = path.clone();
                spawn_blocking(move || {
                    eth_keystore::decrypt_key(&path, password.as_bytes())
                        .map(Zeroizing::new)
                        .with_context(|| format!("decrypting the keystore {}", path.display()))
                })
                .await
                .context("the keystore task")??
            }
            KeySource::HexFile { path } => Zeroizing::new(read_hex_key(path)?.to_vec()),
        };
        refuse_guessable(&secret, !PRODUCTION_BUILD)?;
        let key = Self::from_secret(&secret)?;
        refuse_published(key.address, !PRODUCTION_BUILD)?;
        Ok(key)
    }

    /// The key whose secret is `bytes`. Unlike `load`, it does not refuse the keys this repository
    /// publishes: tests and benchmarks build many keys from counters.
    /// Error cases:
    /// - `bytes` is not 32 bytes of a valid secp256k1 secret key
    #[doc(hidden)]
    pub fn from_secret(bytes: &[u8]) -> anyhow::Result<Self> {
        let key = SigningKey::from_slice(bytes).map_err(|_| anyhow!("not a valid secp256k1 secret key"))?;
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

/// Refuses `secret` outside a dev build (`dev_build`) when anyone could guess it: a counter (the
/// secret fits in eight bytes), as this repository's tests and dev scenarios derive their keys, or one
/// byte repeated. Their addresses are not listed in `PUBLISHED_ACCOUNTS` one by one; the class is.
/// Error cases:
/// - `secret` is such a key and `dev_build` is false
fn refuse_guessable(secret: &[u8], dev_build: bool) -> anyhow::Result<()> {
    let counter = secret.len() > 8 && secret[..secret.len() - 8].iter().all(|b| *b == 0);
    let repeated = secret.windows(2).all(|pair| pair[0] == pair[1]);
    if !dev_build && (counter || repeated) {
        bail!("the key is a counter or a repeated byte, which anyone can guess; only a dev build uses it");
    }
    Ok(())
}

/// Refuses `account` outside a dev build (`dev_build`) when this repository publishes its key.
/// Error cases:
/// - `account` is in `PUBLISHED_ACCOUNTS` and `dev_build` is false
fn refuse_published(account: AccountId, dev_build: bool) -> anyhow::Result<()> {
    if !dev_build && PUBLISHED_ACCOUNTS.contains(&account.to_string().as_str()) {
        bail!("{account}'s private key is published for development; only a dev build uses it");
    }
    Ok(())
}

/// The keystore password `source` names: a file's without one trailing newline, a variable's as
/// it is.
/// Error cases:
/// - the file cannot be read, or is not UTF-8
/// - the variable is unset
fn read_password(source: &PasswordSource, env: impl Fn(&str) -> Option<String>) -> anyhow::Result<Zeroizing<String>> {
    match source {
        PasswordSource::File(path) => {
            let mut password = Zeroizing::new(
                std::fs::read_to_string(path)
                    .with_context(|| format!("reading the password file {}", path.display()))?,
            );
            if password.ends_with('\n') {
                password.pop();
                if password.ends_with('\r') {
                    password.pop();
                }
            }
            Ok(password)
        }
        PasswordSource::Env(name) => {
            Ok(Zeroizing::new(env(name).with_context(|| format!("the password variable {name} is not set"))?))
        }
    }
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
