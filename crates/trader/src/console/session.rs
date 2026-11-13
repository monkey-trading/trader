//! Console sessions: a random token in an `HttpOnly`, `Secure`, `SameSite=Strict` cookie, held by
//! the bot only as its SHA-256, for a fixed lifetime. Sessions live in memory: a restart logs
//! everyone out.

use std::collections::HashMap;
use std::time::Duration;

use axum::http::HeaderMap;
use axum::http::header::COOKIE;
use rand::RngCore;
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};
use tokio::time::Instant;
use zeroize::Zeroizing;

/// The cookie's name. `__Host-` makes a browser refuse it unless it is `Secure`, for `/`, and set
/// by this host alone.
pub const COOKIE_NAME: &str = "__Host-session";
/// How long a session lasts from its login.
pub const LIFETIME: Duration = Duration::from_secs(12 * 60 * 60);
/// The most sessions held at once; a new one past it replaces the oldest.
const SESSIONS: usize = 32;

/// The live sessions, by the SHA-256 of their tokens, with when each ends.
#[derive(Default)]
pub struct Sessions {
    ends: HashMap<[u8; 32], Instant>,
}

impl Sessions {
    /// A new session's token, valid from `now` for `LIFETIME`.
    pub fn create(&mut self, now: Instant) -> Zeroizing<String> {
        self.ends.retain(|_, end| *end > now);
        if self.ends.len() >= SESSIONS
            && let Some(oldest) = self.ends.iter().min_by_key(|(_, end)| **end).map(|(hash, _)| *hash)
        {
            self.ends.remove(&oldest);
        }
        let mut bytes = Zeroizing::new([0u8; 32]);
        OsRng.fill_bytes(bytes.as_mut());
        let token = Zeroizing::new(hex::encode(bytes.as_ref()));
        self.ends.insert(digest(&token), now + LIFETIME);
        token
    }

    /// Whether `token` is a live session's at `now`.
    pub fn is_live(&self, token: &str, now: Instant) -> bool {
        self.ends.get(&digest(token)).is_some_and(|end| *end > now)
    }

    /// Ends `token`'s session.
    pub fn end(&mut self, token: &str) {
        self.ends.remove(&digest(token));
    }
}

/// The `Set-Cookie` value that holds `token` for the session's lifetime.
pub fn cookie(token: &str) -> String {
    format!("{COOKIE_NAME}={token}; Path=/; Max-Age={}; HttpOnly; Secure; SameSite=Strict", LIFETIME.as_secs())
}

/// The `Set-Cookie` value that clears the session's cookie.
pub fn cleared_cookie() -> String {
    format!("{COOKIE_NAME}=; Path=/; Max-Age=0; HttpOnly; Secure; SameSite=Strict")
}

/// The session token in a request's cookies, if it carries one.
pub fn token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|pair| pair.trim().strip_prefix(COOKIE_NAME)?.strip_prefix('='))
}

/// The SHA-256 of a token: what the store keys sessions by, so it holds no token itself.
fn digest(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    /// A session lives for its lifetime and no longer, and ends when ended.
    #[tokio::test(start_paused = true)]
    async fn sessions_end() {
        let mut sessions = Sessions::default();
        let start = Instant::now();
        let token = sessions.create(start);
        assert_eq!(token.len(), 64);
        assert!(sessions.is_live(&token, start));
        assert!(sessions.is_live(&token, start + LIFETIME - Duration::from_millis(1)));
        assert!(!sessions.is_live(&token, start + LIFETIME));
        let other = sessions.create(start);
        assert_ne!(*token, *other);
        sessions.end(&other);
        assert!(!sessions.is_live(&other, start));
        assert!(!sessions.is_live("", start));
        assert!(!sessions.is_live(&"0".repeat(64), start));
    }

    /// Past the cap a new session replaces the oldest, and no other.
    #[tokio::test(start_paused = true)]
    async fn the_oldest_session_goes_first() {
        let mut sessions = Sessions::default();
        let start = Instant::now();
        let tokens: Vec<_> = (0..SESSIONS).map(|n| sessions.create(start + Duration::from_secs(n as u64))).collect();
        let now = start + Duration::from_secs(SESSIONS as u64);
        let newest = sessions.create(now);
        assert!(!sessions.is_live(&tokens[0], now));
        assert!(tokens[1..].iter().all(|token| sessions.is_live(token, now)));
        assert!(sessions.is_live(&newest, now));
        assert_eq!(sessions.ends.len(), SESSIONS);
    }

    /// The cookie carries every flag the session needs, and its token reads back from a request.
    #[test]
    fn cookies_round_trip() {
        assert_eq!(cookie("abc"), "__Host-session=abc; Path=/; Max-Age=43200; HttpOnly; Secure; SameSite=Strict");
        assert!(cleared_cookie().contains("Max-Age=0"));
        let mut headers = HeaderMap::new();
        assert_eq!(token(&headers), None);
        headers.append(COOKIE, HeaderValue::from_static("theme=dark; __Host-sessionx=no"));
        assert_eq!(token(&headers), None);
        headers.append(COOKIE, HeaderValue::from_static("a=1;  __Host-session=abc ; b=2"));
        assert_eq!(token(&headers), Some("abc"));
    }
}
