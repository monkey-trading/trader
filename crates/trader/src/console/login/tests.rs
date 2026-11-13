use std::os::unix::fs::PermissionsExt;

use super::*;

/// A client has `BURST` attempts at once, then one each `REFILL`, and is told how long to wait.
#[tokio::test(start_paused = true)]
async fn attempts_refill_at_the_rate() {
    let mut limiter = Limiter::default();
    let client: IpAddr = "192.0.2.7".parse().unwrap();
    let start = Instant::now();
    for _ in 0..BURST {
        limiter.take(client, start).unwrap();
    }
    assert_eq!(limiter.take(client, start), Err(REFILL));
    let later = start + Duration::from_secs(5);
    assert_eq!(limiter.take(client, later), Err(REFILL - Duration::from_secs(5)));
    limiter.take(client, start + REFILL).unwrap();
    assert!(limiter.take(client, start + REFILL).is_err());
    // A long wait refills to the burst and no further.
    let idle = start + REFILL * 100;
    for _ in 0..BURST {
        limiter.take(client, idle).unwrap();
    }
    assert!(limiter.take(client, idle).is_err());
}

/// Clients are limited apart, an IPv6 client by its /64, an IPv4-mapped address as its IPv4.
#[tokio::test(start_paused = true)]
async fn clients_are_limited_apart() {
    let mut limiter = Limiter::default();
    let now = Instant::now();
    let exhaust = |limiter: &mut Limiter, client: &str| {
        for _ in 0..BURST {
            limiter.take(client.parse().unwrap(), now).unwrap();
        }
    };
    exhaust(&mut limiter, "2001:db8:1:2::1");
    assert!(limiter.take("2001:db8:1:2:ffff::9".parse().unwrap(), now).is_err(), "the same /64");
    limiter.take("2001:db8:1:3::1".parse().unwrap(), now).unwrap();
    exhaust(&mut limiter, "192.0.2.1");
    assert!(limiter.take("::ffff:192.0.2.1".parse().unwrap(), now).is_err(), "the same IPv4 client");
    limiter.take("192.0.2.2".parse().unwrap(), now).unwrap();
}

/// Past `CLIENTS` tracked clients a new one waits, unless an idle client's slot can go.
#[tokio::test(start_paused = true)]
async fn the_client_table_is_bounded() {
    let mut limiter = Limiter::default();
    let now = Instant::now();
    let client = |n: usize| IpAddr::from([10, (n >> 16) as u8, (n >> 8) as u8, n as u8]);
    for n in 0..CLIENTS {
        limiter.take(client(n), now).unwrap();
    }
    assert_eq!(limiter.take(client(CLIENTS), now), Err(REFILL));
    assert_eq!(limiter.buckets.len(), CLIENTS);
    // Once the others' buckets refill, their slots go to new clients.
    limiter.take(client(CLIENTS), now + REFILL).unwrap();
    assert_eq!(limiter.buckets.len(), 1);
}

/// The first call generates a password, prints it once, and keeps only its hash, readable by the
/// bot's user alone; later calls read the hash and print nothing.
#[test]
fn the_password_is_printed_once_and_kept_as_a_hash() {
    let dir = tempfile::tempdir().unwrap();
    let mut out = Vec::new();
    let hash = password_hash(dir.path(), &mut out).unwrap();
    let printed = String::from_utf8(out).unwrap();
    let password = printed.trim_end().rsplit(": ").next().unwrap().to_owned();
    assert_eq!(password.len(), PASSWORD_LEN, "{printed}");
    assert!(password.bytes().all(|b| ALPHABET.contains(&b)), "{password}");
    let path = dir.path().join(HASH_FILE);
    let stored = std::fs::read_to_string(&path).unwrap();
    assert_eq!(stored, hash);
    assert!(hash.starts_with("$argon2id$"), "{hash}");
    assert!(!stored.contains(&password));
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    assert!(verify(&hash, &password));
    assert!(!verify(&hash, &format!("{password}x")));
    assert!(!verify(&hash, ""));

    let mut again = Vec::new();
    assert_eq!(password_hash(dir.path(), &mut again).unwrap(), hash);
    assert!(again.is_empty(), "printed again: {}", String::from_utf8_lossy(&again));
}

/// Two first runs make different passwords.
#[test]
fn passwords_are_random() {
    assert_ne!(*generate(), *generate());
}

/// A hash file that holds no Argon2id hash is refused, and left for the user.
#[test]
fn a_damaged_hash_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(HASH_FILE);
    for text in ["", "not a hash", "$argon2i$v=19$m=16,t=2,p=1$c2FsdHNhbHQ$aGFzaGhhc2hoYXNoaGFzaA"] {
        std::fs::write(&path, text).unwrap();
        let err = password_hash(dir.path(), &mut Vec::new()).unwrap_err();
        assert!(err.to_string().contains("no Argon2id hash"), "{text:?}: {err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }
}
