//! The console's login: one password, generated on first run and printed once, kept only as its
//! Argon2id hash; and the rate limit on attempts to log in.

use std::collections::HashMap;
use std::io::Write;
use std::net::{IpAddr, Ipv6Addr};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, bail};
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use argon2::{Algorithm, Argon2};
use rand::Rng;
use rand::rngs::OsRng;
use tokio::time::Instant;
use zeroize::Zeroizing;

use crate::files::write_private;

/// The file that holds the password's hash, in the console's data directory.
pub const HASH_FILE: &str = "login.argon2";
/// The password's characters: lower-case letters and digits, with none that read alike.
const ALPHABET: &[u8] = b"abcdefghijkmnpqrstuvwxyz23456789";
/// The password's length: 26 characters of 32 kinds, 130 bits.
const PASSWORD_LEN: usize = 26;
/// Attempts a client may make at once, and how often it earns another.
const BURST: u32 = 5;
const REFILL: Duration = Duration::from_secs(12);
/// The most clients the limiter tracks; past it, a new client waits for a slot.
const CLIENTS: usize = 1024;

/// Login attempts per client: a bucket of `BURST` attempts, refilled one every `REFILL`. A client
/// is an IPv4 address, or an IPv6 address's /64, which one host usually holds whole.
#[derive(Default)]
pub struct Limiter {
    buckets: HashMap<IpAddr, Bucket>,
}

/// A client's attempts left, as of `at`.
struct Bucket {
    left: u32,
    at: Instant,
}

impl Limiter {
    /// Takes one attempt for `client` at `now`.
    /// Error cases:
    /// - the client has none left, or no slot is free: the time until one is
    pub fn take(&mut self, client: IpAddr, now: Instant) -> Result<(), Duration> {
        let client = client_key(client);
        if !self.buckets.contains_key(&client) && self.buckets.len() >= CLIENTS {
            // A full bucket is the same as none.
            self.buckets.retain(|_, bucket| bucket.refill(now) < BURST);
            if self.buckets.len() >= CLIENTS {
                return Err(REFILL);
            }
        }
        let bucket = self.buckets.entry(client).or_insert(Bucket { left: BURST, at: now });
        if bucket.refill(now) == 0 {
            return Err(REFILL.saturating_sub(now - bucket.at));
        }
        bucket.left -= 1;
        Ok(())
    }
}

impl Bucket {
    /// Adds the attempts earned since `at`, up to `BURST`; how many are left.
    fn refill(&mut self, now: Instant) -> u32 {
        let earned = u32::try_from((now - self.at).as_nanos() / REFILL.as_nanos()).unwrap_or(u32::MAX);
        if self.left.saturating_add(earned) >= BURST {
            (self.left, self.at) = (BURST, now);
        } else {
            self.left += earned;
            self.at += REFILL * earned;
        }
        self.left
    }
}

/// The password's hash from `data_dir`, generating a password when there is none: its hash is
/// written and the password printed to `out`, once, never logged.
/// Error cases:
/// - the hash cannot be read, or is not an Argon2id hash
/// - a new hash cannot be written, or the password printed
pub fn password_hash(data_dir: &Path, out: &mut dyn Write) -> anyhow::Result<String> {
    let path = data_dir.join(HASH_FILE);
    if path.exists() {
        let hash = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let hash = hash.trim().to_owned();
        match PasswordHash::new(&hash) {
            Ok(parsed) if parsed.algorithm == Algorithm::Argon2id.ident() => return Ok(hash),
            _ => bail!("{} holds no Argon2id hash; delete it for a new password", path.display()),
        }
    }
    let password = generate();
    let hash = Argon2::default().hash_password(password.as_bytes()).context("hashing the password")?.to_string();
    write_private(&path, hash.as_bytes())?;
    writeln!(out, "console password, shown once and kept only as a hash in {}: {}", path.display(), *password)
        .context("printing the password")?;
    Ok(hash)
}

/// Whether `password` is the one `hash` holds; the comparison takes the same time either way.
/// Slow by design: call it off the async runtime.
pub fn verify(hash: &str, password: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|hash| Argon2::default().verify_password(password.as_bytes(), &hash).is_ok())
}

/// A new password from the operating system's random source.
fn generate() -> Zeroizing<String> {
    let mut rng = OsRng;
    Zeroizing::new((0..PASSWORD_LEN).map(|_| char::from(ALPHABET[rng.gen_range(0..ALPHABET.len())])).collect())
}

/// The address a client is limited by: an IPv4 address whole, an IPv6 address by its /64.
fn client_key(client: IpAddr) -> IpAddr {
    match client {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(Ipv6Addr::from(v6.to_bits() & !u128::from(u64::MAX))),
        },
        v4 => v4,
    }
}

#[cfg(test)]
mod tests;
