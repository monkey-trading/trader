//! Who may use the console, against the stand-in: the login takes only the password the console
//! printed, a few attempts at a time; every API route but the login needs the session; every
//! request that changes state, and the live feed, needs the console's own `Origin`; every answer
//! carries the security headers; no answer carries the account's key and no route moves funds; and
//! a binary without the console's files says so.

mod common;

use common::console::{Running, SAME, console_config, printed_password, seed, until_started};
use common::{ACCOUNT_SECRET, MINUTE, StandIn, T0, account};
use hyper::Method;
use serde_json::json;

/// Every API route but the login, with a body each takes.
fn api_routes() -> Vec<(Method, String, Option<serde_json::Value>)> {
    let a = account();
    let params = Some(json!({ "fast": 2, "slow": 3 }));
    vec![
        (Method::GET, "/api/accounts".into(), None),
        (Method::GET, format!("/api/accounts/{a}"), None),
        (Method::GET, format!("/api/accounts/{a}/fills"), None),
        (Method::GET, "/api/live".into(), None),
        (Method::POST, format!("/api/accounts/{a}/start"), None),
        (Method::POST, format!("/api/accounts/{a}/stop"), None),
        (Method::POST, format!("/api/accounts/{a}/flatten"), None),
        (Method::POST, format!("/api/accounts/{a}/strategies/0/start"), None),
        (Method::POST, format!("/api/accounts/{a}/strategies/0/stop"), None),
        (Method::PUT, format!("/api/accounts/{a}/strategies/0/params"), params),
        (Method::POST, "/api/logout".into(), None),
    ]
}

/// The console prints its certificate's fingerprint and, once, the password; only that password
/// logs in, into a session cookie the browser keeps from scripts and other sites; logging out ends it.
#[tokio::test]
async fn logs_in_with_the_printed_password_only() {
    let stand_in = StandIn::start().await;
    seed(&stand_in);
    let dir = tempfile::tempdir().unwrap();
    let running = Running::start(console_config(&stand_in, dir.path(), "")).await;
    assert!(running.printed.contains("SHA-256 fingerprint "), "{}", running.printed);
    let password = printed_password(&running.printed);

    let (wrong, cookie) = running.login("not-the-password").await;
    assert_eq!((wrong.status, cookie), (401, None), "{wrong:?}");
    let (empty, _) = running.login("").await;
    assert_eq!(empty.status, 401);

    let (answer, cookie) = running.login(&password).await;
    assert_eq!(answer.status, 204, "{answer:?}");
    let set_cookie = answer.headers["set-cookie"].to_str().unwrap();
    for flag in ["__Host-session=", "; Path=/", "; HttpOnly", "; Secure", "; SameSite=Strict"] {
        assert!(set_cookie.contains(flag), "{flag} missing from {set_cookie}");
    }
    let cookie = cookie.unwrap();
    assert_eq!(running.get("/api/accounts", &cookie).await.status, 200);

    let logout = running.post("/api/logout", &cookie).await;
    assert_eq!(logout.status, 204);
    assert!(logout.headers["set-cookie"].to_str().unwrap().contains("Max-Age=0"));
    assert_eq!(running.get("/api/accounts", &cookie).await.status, 401, "the session ended");
    running.stop_after(&stand_in).await.unwrap();
}

/// A client gets five attempts at once; the sixth is refused, even with the right password, and
/// told when to try again.
#[tokio::test]
async fn login_attempts_are_limited() {
    let stand_in = StandIn::start().await;
    seed(&stand_in);
    let dir = tempfile::tempdir().unwrap();
    let running = Running::start(console_config(&stand_in, dir.path(), "")).await;
    for _ in 0..5 {
        assert_eq!(running.login("wrong").await.0.status, 401);
    }
    let (limited, cookie) = running.login(&printed_password(&running.printed)).await;
    assert_eq!((limited.status, cookie), (429, None), "{limited:?}");
    let retry: u64 = limited.headers["retry-after"].to_str().unwrap().parse().unwrap();
    assert!((1..=12).contains(&retry), "{retry}");
    running.stop_after(&stand_in).await.unwrap();
}

/// Every API route but the login refuses a request without a live session, from the console's own
/// origin; with one, none refuses for want of it.
#[tokio::test]
async fn every_api_route_needs_the_session() {
    let stand_in = StandIn::start().await;
    seed(&stand_in);
    let dir = tempfile::tempdir().unwrap();
    let running = Running::start(console_config(&stand_in, dir.path(), "")).await;
    let cookie = running.session().await;
    until_started(&running, &cookie).await;
    // The log passes every order an earlier process could have signed, so a flatten acts at once.
    stand_in.applied(stand_in.tick(T0 + MINUTE)).await;
    for (method, path, body) in api_routes() {
        for stale in [None, Some("__Host-session=00"), Some("__Host-session=")] {
            let answer = running.call(method.clone(), &path, SAME, stale, body.clone()).await;
            assert_eq!(answer.status, 401, "{method} {path} with {stale:?}: {answer:?}");
            assert_eq!(answer.body, json!({ "error": "log in first" }));
        }
    }
    assert_eq!(running.live(SAME, None).await.err(), Some(401));
    for (method, path, body) in api_routes().into_iter().filter(|(_, path, _)| path != "/api/logout") {
        let answer = running.call(method.clone(), &path, SAME, Some(&cookie), body).await;
        assert_ne!(answer.status, 401, "{method} {path}: {answer:?}");
    }
    assert!(running.live(SAME, Some(&cookie)).await.is_ok());
    running.stop_after(&stand_in).await.unwrap();
}

/// A request that changes state, the login included, and the live feed are refused in a live
/// session unless their `Origin` is the console's own, and change nothing.
#[tokio::test]
async fn state_changes_need_the_consoles_origin() {
    let stand_in = StandIn::start().await;
    seed(&stand_in);
    let dir = tempfile::tempdir().unwrap();
    let running = Running::start(console_config(&stand_in, dir.path(), "")).await;
    let cookie = running.session().await;
    until_started(&running, &cookie).await;
    let foreign = [None, Some("https://evil.example"), Some("null"), Some("http://127.0.0.1")];
    let mut changing: Vec<_> = api_routes().into_iter().filter(|(method, _, _)| method != Method::GET).collect();
    changing.push((Method::POST, "/api/login".into(), Some(json!({ "password": printed_password(&running.printed) }))));
    for (method, path, body) in changing {
        for origin in foreign {
            let answer = running.call(method.clone(), &path, origin, Some(&cookie), body.clone()).await;
            assert_eq!(answer.status, 403, "{method} {path} from {origin:?}: {answer:?}");
            assert!(!answer.headers.contains_key("set-cookie"));
        }
    }
    for origin in foreign {
        assert_eq!(running.live(origin, Some(&cookie)).await.err(), Some(403), "{origin:?}");
    }
    let view = running.get(&format!("/api/accounts/{}", account()), &cookie).await;
    assert_eq!(view.body["state"], "running", "nothing stopped the account");
    assert!(stand_in.with(|i| i.posts.is_empty()), "nothing reached the node");
    running.stop_after(&stand_in).await.unwrap();
}

/// API answers, refusals and the console's page carry the security headers; API answers are never
/// cached.
#[tokio::test]
async fn answers_carry_the_security_headers() {
    let stand_in = StandIn::start().await;
    seed(&stand_in);
    let dir = tempfile::tempdir().unwrap();
    let running = Running::start(console_config(&stand_in, dir.path(), "")).await;
    let cookie = running.session().await;
    for (answer, api) in [
        (running.get("/api/accounts", &cookie).await, true),
        (running.call(Method::GET, "/api/accounts", None, None, None).await, true),
        (running.call(Method::POST, "/api/login", Some("https://evil.example"), None, None).await, false),
        (running.call(Method::GET, "/", None, None, None).await, false),
        (running.call(Method::GET, "/api/nothing", None, None, None).await, false),
    ] {
        let header = |name: &str| answer.headers.get(name).map(|v| v.to_str().unwrap().to_owned());
        let csp = header("content-security-policy").unwrap_or_default();
        for directive in ["default-src 'self'", "script-src 'self'", "connect-src 'self'", "frame-ancestors 'none'"] {
            assert!(csp.contains(directive), "{directive} missing: {answer:?}");
        }
        assert!(!csp.contains("unsafe"), "{csp}");
        assert_eq!(header("x-content-type-options").as_deref(), Some("nosniff"));
        assert_eq!(header("x-frame-options").as_deref(), Some("DENY"));
        assert_eq!(header("referrer-policy").as_deref(), Some("no-referrer"));
        assert_eq!(header("cross-origin-opener-policy").as_deref(), Some("same-origin"));
        assert!(header("permissions-policy").is_some_and(|p| p.contains("camera=()")));
        if api {
            assert_eq!(header("cache-control").as_deref(), Some("no-store"));
        }
    }
    running.stop_after(&stand_in).await.unwrap();
}

/// No answer carries the account's key, in any form, and no route moves funds or reads a key.
#[tokio::test]
async fn no_route_returns_a_key_or_moves_funds() {
    let stand_in = StandIn::start().await;
    seed(&stand_in);
    let dir = tempfile::tempdir().unwrap();
    let running = Running::start(console_config(&stand_in, dir.path(), "")).await;
    let cookie = running.session().await;
    until_started(&running, &cookie).await;
    let key = hex::encode(ACCOUNT_SECRET);
    for (method, path, _) in
        api_routes().into_iter().filter(|(method, path, _)| method == Method::GET && path != "/api/live")
    {
        let answer = running.call(method, &path, None, Some(&cookie), None).await;
        assert_eq!(answer.status, 200, "{path}");
        assert!(!answer.text.to_lowercase().contains(&key), "{path} answered with the key");
    }
    let a = account();
    for path in [
        format!("/api/accounts/{a}/withdraw"),
        format!("/api/accounts/{a}/withdrawals"),
        format!("/api/accounts/{a}/deposit"),
        format!("/api/accounts/{a}/claim"),
        format!("/api/accounts/{a}/transfer"),
        format!("/api/accounts/{a}/key"),
        "/api/keys".into(),
        "/api/accounts/export".into(),
    ] {
        for method in [Method::GET, Method::POST] {
            let answer = running.call(method.clone(), &path, SAME, Some(&cookie), Some(json!({}))).await;
            assert!([404, 405].contains(&answer.status), "{method} {path}: {answer:?}");
            assert!(!answer.text.to_lowercase().contains(&key));
        }
    }
    assert!(stand_in.with(|i| i.posts.is_empty()), "nothing reached the node");
    running.stop_after(&stand_in).await.unwrap();
}

/// A binary built without the console's files serves a page saying so at every page path; one
/// built with them serves their `index.html`.
#[tokio::test]
async fn serves_the_console_or_says_it_is_not_built() {
    let stand_in = StandIn::start().await;
    seed(&stand_in);
    let dir = tempfile::tempdir().unwrap();
    let running = Running::start(console_config(&stand_in, dir.path(), "")).await;
    let index = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../console/dist/index.html")).ok();
    for path in ["/", "/accounts/0xabc"] {
        let page = running.call(Method::GET, path, None, None, None).await;
        assert_eq!(page.status, 200);
        assert!(page.headers["content-type"].to_str().unwrap().starts_with("text/html"));
        match &index {
            Some(index) => assert_eq!(&page.text, index),
            None => assert!(page.text.contains("The console is not built into this binary"), "{}", page.text),
        }
    }
    running.stop_after(&stand_in).await.unwrap();
}
