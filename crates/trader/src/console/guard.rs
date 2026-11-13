//! What console requests pass through: a log span for each, the security headers on every answer,
//! the `Origin` check on every request that changes state or opens the live feed, a limit on the
//! time and size of each request's body, and the session check on every API route but the login.

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::header::{HOST, ORIGIN};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use tokio::time::{Instant, timeout};
use tracing::{Instrument, debug, info_span, warn};

use crate::console::Server;
use crate::console::live::LIVE_PATH;
use crate::console::session;

/// The headers on every answer: the page loads nothing but its own files and talks only to the
/// bot, no other site may frame it, and the browser guesses no content type.
const SECURITY_HEADERS: [(&str, &str); 6] = [
    (
        "content-security-policy",
        "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; font-src 'self'; \
         connect-src 'self'; object-src 'none'; base-uri 'self'; form-action 'self'; frame-ancestors 'none'",
    ),
    ("x-content-type-options", "nosniff"),
    ("referrer-policy", "no-referrer"),
    ("x-frame-options", "DENY"),
    ("cross-origin-opener-policy", "same-origin"),
    ("permissions-policy", "camera=(), microphone=(), geolocation=(), payment=(), usb=(), browsing-topics=()"),
];

/// The longest a client may take to send a request's body, so a client that stops halfway holds
/// none of the console's connections for long.
pub const BODY_TIMEOUT: Duration = Duration::from_secs(10);
/// The largest request body: a login or a strategy's parameters.
pub const MAX_BODY: usize = 4096;

/// A console request's id, which the requests it sends an account carry.
#[derive(Clone, Copy, Debug)]
pub struct RequestId(pub u64);

/// Runs the request in a span carrying its id, method and path, and gives the id to its handler.
pub async fn span(State(server): State<Arc<Server>>, mut request: Request, next: Next) -> Response {
    let id = server.next_request_id();
    request.extensions_mut().insert(RequestId(id));
    let span = info_span!("console_request", id, method = %request.method(), path = %request.uri().path());
    async move {
        let response = next.run(request).await;
        debug!(status = response.status().as_u16(), "answered");
        response
    }
    .instrument(span)
    .await
}

/// Adds the security headers to the answer.
pub async fn headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    for (name, value) in SECURITY_HEADERS {
        headers.insert(HeaderName::from_static(name), HeaderValue::from_static(value));
    }
    response
}

/// Reads the request's whole body, at most `MAX_BODY` bytes, within `BODY_TIMEOUT`, before the
/// handler sees it; refuses one too slow (408) or too large (413).
pub async fn read_body(request: Request, next: Next) -> Response {
    let (parts, body) = request.into_parts();
    let bytes = match timeout(BODY_TIMEOUT, to_bytes(body, MAX_BODY)).await {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(_)) => {
            return refusal(StatusCode::PAYLOAD_TOO_LARGE, "body_too_large", "the request's body is too large");
        }
        Err(_) => {
            return refusal(StatusCode::REQUEST_TIMEOUT, "body_timeout", "the request's body did not arrive in time");
        }
    };
    next.run(Request::from_parts(parts, Body::from(bytes))).await
}

/// Refuses a request that changes state, or opens the live feed, unless its `Origin` is the
/// console's own: `https://` and the host it asked for. A browser always sends `Origin` on such a
/// request, and no page may set it, so another site's page cannot act through the user's session.
pub async fn check_origin(request: Request, next: Next) -> Response {
    let reads = matches!(*request.method(), Method::GET | Method::HEAD) && request.uri().path() != LIVE_PATH;
    if !reads && !same_origin(request.headers()) {
        warn!(origin = ?request.headers().get(ORIGIN), "refused: the request's origin is not the console");
        return refusal(StatusCode::FORBIDDEN, "foreign_origin", "the request's origin is not the console");
    }
    next.run(request).await
}

/// Refuses a request without a live session.
pub async fn require_session(State(server): State<Arc<Server>>, request: Request, next: Next) -> Response {
    let live = session::token(request.headers()).is_some_and(|token| server.session_live(token, Instant::now()));
    if !live {
        debug!("refused: no live session");
        return refusal(StatusCode::UNAUTHORIZED, "session_required", "log in first");
    }
    next.run(request).await
}

/// A JSON refusal with `status`: `{"error": message, "code": code}`, where a client matches on the
/// code and shows the message.
pub fn refusal(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({ "error": message, "code": code }))).into_response()
}

/// Whether the request's `Origin` is `https://` and its `Host`.
fn same_origin(headers: &HeaderMap) -> bool {
    let text = |name| headers.get(name).and_then(|value: &HeaderValue| value.to_str().ok());
    match (text(ORIGIN), text(HOST)) {
        (Some(origin), Some(host)) => {
            origin.strip_prefix("https://").is_some_and(|origin| origin.eq_ignore_ascii_case(host))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only the console's own HTTPS origin passes.
    #[test]
    fn origins_must_be_the_consoles() {
        let check = |origin: Option<&'static str>, host: Option<&'static str>| {
            let mut headers = HeaderMap::new();
            if let Some(origin) = origin {
                headers.insert(ORIGIN, HeaderValue::from_static(origin));
            }
            if let Some(host) = host {
                headers.insert(HOST, HeaderValue::from_static(host));
            }
            same_origin(&headers)
        };
        assert!(check(Some("https://192.168.1.5:8443"), Some("192.168.1.5:8443")));
        assert!(check(Some("https://Bot.Local:8443"), Some("bot.local:8443")));
        assert!(!check(Some("http://192.168.1.5:8443"), Some("192.168.1.5:8443")));
        assert!(!check(Some("https://evil.example"), Some("192.168.1.5:8443")));
        assert!(!check(Some("https://192.168.1.5:8443.evil.example"), Some("192.168.1.5:8443")));
        assert!(!check(Some("https://192.168.1.5"), Some("192.168.1.5:8443")));
        assert!(!check(Some("null"), Some("192.168.1.5:8443")));
        assert!(!check(None, Some("192.168.1.5:8443")));
        assert!(!check(Some("https://192.168.1.5:8443"), None));
    }
}
