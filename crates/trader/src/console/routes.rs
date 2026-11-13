//! The console's routes. Every one but the login and the static files needs a live session, and
//! every one that changes state checks `Origin` (`guard`). Reads answer from what the accounts and
//! the refresher last published; controls ask the bot's runner, which alone changes the accounts'
//! controls. No route moves funds or returns a key: the controls start, stop and flatten an account
//! and set a strategy's parameters within the config's bounds, and nothing else.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{ConnectInfo, DefaultBodyLimit, Extension, FromRequest, FromRequestParts, Path, Request, State};
use axum::http::header::{CACHE_CONTROL, SET_COOKIE};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{from_fn, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::json;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::oneshot;
use tokio::task::spawn_blocking;
use tokio::time::{Instant, timeout};
use tracing::{info, warn};
use zeroize::Zeroizing;

use crate::bot::link::{AccountRequest, Control, Refusal};
use crate::console::guard::{self, MAX_BODY, RequestId, refusal};
use crate::console::live::{self, LIVE_PATH};
use crate::console::{Server, assets, login, session};

/// The longest password the login hashes; the generated one is far shorter.
const MAX_PASSWORD: usize = 128;
/// How long a control waits for the account to answer: a flatten may first wait for the log to
/// pass the account's last order, then makes several calls to the node.
const CONTROL_WAIT: Duration = Duration::from_secs(10);
const FLATTEN_WAIT: Duration = Duration::from_secs(120);

/// A login's body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginBody {
    password: String,
}

/// Every route, its guards, and the static files.
pub fn router(server: Arc<Server>) -> Router {
    let api = Router::new()
        .route("/api/accounts", get(accounts))
        .route("/api/accounts/{address}", get(account))
        .route("/api/accounts/{address}/fills", get(fills))
        .route("/api/accounts/{address}/start", post(start))
        .route("/api/accounts/{address}/stop", post(stop))
        .route("/api/accounts/{address}/flatten", post(flatten))
        .route("/api/accounts/{address}/strategies/{id}/start", post(start_strategy))
        .route("/api/accounts/{address}/strategies/{id}/stop", post(stop_strategy))
        .route("/api/accounts/{address}/strategies/{id}/params", put(set_params))
        .route("/api/session", get(session))
        .route("/api/logout", post(logout))
        .route(LIVE_PATH, get(live::live))
        .route_layer(from_fn_with_state(server.clone(), guard::require_session))
        .layer(from_fn(no_store));
    Router::new()
        .route("/api/login", post(login))
        .merge(api)
        .method_not_allowed_fallback(not_allowed)
        .fallback(assets::serve)
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .layer(from_fn(guard::read_body))
        .layer(from_fn(guard::check_origin))
        .layer(from_fn(guard::headers))
        .layer(from_fn_with_state(server.clone(), guard::span))
        .with_state(server)
}

/// Logs in with the password: a new session in its cookie, or a refusal. Each client has a few
/// attempts at once and earns one every few seconds; one password check runs at a time.
async fn login(
    State(server): State<Arc<Server>>,
    ConnectInfo(client): ConnectInfo<SocketAddr>,
    Body(body): Body<LoginBody>,
) -> Response {
    let password = Zeroizing::new(body.password);
    if let Err(wait) = server.take_login_attempt(client.ip(), Instant::now()) {
        warn!(%client, "login refused: too many attempts");
        let mut response = refusal(StatusCode::TOO_MANY_REQUESTS, "rate_limited", "too many attempts; try again later");
        let seconds = wait.as_secs() + u64::from(wait.subsec_nanos() > 0);
        response.headers_mut().insert("retry-after", HeaderValue::from(seconds));
        return response;
    }
    let right = password.len() <= MAX_PASSWORD && {
        let turn = server.password_check().await;
        let hash = server.password_hash().to_owned();
        // The turn goes with the hash into the blocking task, which runs on when the client leaves.
        spawn_blocking(move || {
            let _turn = turn;
            login::verify(&hash, &password)
        })
        .await
        .unwrap_or(false)
    };
    if !right {
        warn!(%client, "login refused: wrong password");
        return refusal(StatusCode::UNAUTHORIZED, "wrong_password", "wrong password");
    }
    let token = server.new_session(Instant::now());
    info!(%client, "logged in");
    (StatusCode::NO_CONTENT, [(SET_COOKIE, session::cookie(&token))]).into_response()
}

/// Ends the session and clears its cookie.
async fn logout(State(server): State<Arc<Server>>, headers: HeaderMap) -> Response {
    if let Some(token) = session::token(&headers) {
        server.end_session(token);
    }
    info!("logged out");
    (StatusCode::NO_CONTENT, [(SET_COOKIE, session::cleared_cookie())]).into_response()
}

/// Every account, and their totals.
async fn accounts(State(server): State<Arc<Server>>) -> Response {
    let (accounts, totals) = server.overview();
    Json(json!({ "accounts": accounts, "totals": totals })).into_response()
}

/// One account.
async fn account(State(server): State<Arc<Server>>, AddressPath(address): AddressPath) -> Response {
    match server.account_view(&address) {
        Some(view) => Json(view).into_response(),
        None => no_account(),
    }
}

/// One account's latest fills, newest first.
async fn fills(State(server): State<Arc<Server>>, AddressPath(address): AddressPath) -> Response {
    match server.fills(&address) {
        Some(fills) => Json(json!({ "fills": *fills })).into_response(),
        None => no_account(),
    }
}

/// Whether the request's session is live: 204 when it is; the session check answers 401 when not.
async fn session() -> StatusCode {
    StatusCode::NO_CONTENT
}

/// Lets the account's strategies place orders.
async fn start(
    State(server): State<Arc<Server>>,
    Extension(id): Extension<RequestId>,
    AddressPath(address): AddressPath,
) -> Response {
    control(&server, id, &address, CONTROL_WAIT, Control::Run { running: true }).await
}

/// Stops the account's strategies placing orders.
async fn stop(
    State(server): State<Arc<Server>>,
    Extension(id): Extension<RequestId>,
    AddressPath(address): AddressPath,
) -> Response {
    control(&server, id, &address, CONTROL_WAIT, Control::Run { running: false }).await
}

/// Stops the account, then cancels its resting orders and closes its positions.
async fn flatten(
    State(server): State<Arc<Server>>,
    Extension(id): Extension<RequestId>,
    AddressPath(address): AddressPath,
) -> Response {
    control(&server, id, &address, FLATTEN_WAIT, Control::Flatten).await
}

/// Lets one strategy place orders.
async fn start_strategy(
    State(server): State<Arc<Server>>,
    Extension(id): Extension<RequestId>,
    StrategyPath(address, strategy): StrategyPath,
) -> Response {
    control(&server, id, &address, CONTROL_WAIT, Control::RunStrategy { strategy, running: true }).await
}

/// Stops one strategy placing orders.
async fn stop_strategy(
    State(server): State<Arc<Server>>,
    Extension(id): Extension<RequestId>,
    StrategyPath(address, strategy): StrategyPath,
) -> Response {
    control(&server, id, &address, CONTROL_WAIT, Control::RunStrategy { strategy, running: false }).await
}

/// Sets one strategy's parameters, `{kind, every parameter}`, within the config's bounds.
async fn set_params(
    State(server): State<Arc<Server>>,
    Extension(id): Extension<RequestId>,
    StrategyPath(address, strategy): StrategyPath,
    Body(params): Body<serde_json::Value>,
) -> Response {
    control(&server, id, &address, CONTROL_WAIT, Control::SetParams { strategy, params }).await
}

/// Keeps API answers out of every cache.
async fn no_store(request: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Sends `control` to the account at `address` as console request `id`, and answers with what it
/// did, or 504 when it has not answered within `wait`; it may still act on the request then.
async fn control(
    server: &Server,
    RequestId(id): RequestId,
    address: &str,
    wait: Duration,
    control: Control,
) -> Response {
    let Some((account, requests)) = server.requests(address) else { return no_account() };
    if server.halted(address) {
        let message = "the account is halted: its controls do nothing";
        return refusal(StatusCode::UNPROCESSABLE_ENTITY, "account_halted", message);
    }
    let (reply, answer) = oneshot::channel();
    match requests.try_send(AccountRequest { account, id, control, reply }) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => {
            let message = "the accounts have requests waiting; try again";
            return refusal(StatusCode::SERVICE_UNAVAILABLE, "requests_waiting", message);
        }
        Err(TrySendError::Closed(_)) => {
            return refusal(StatusCode::SERVICE_UNAVAILABLE, "bot_stopping", "the bot is stopping");
        }
    }
    match timeout(wait, answer).await {
        Ok(Ok(Ok(()))) => StatusCode::NO_CONTENT.into_response(),
        Ok(Ok(Err(refused))) => {
            let status = match refused {
                Refusal::NoStrategy(_) => StatusCode::NOT_FOUND,
                Refusal::Invalid { .. } => StatusCode::UNPROCESSABLE_ENTITY,
                Refusal::Failed { .. } => StatusCode::INTERNAL_SERVER_ERROR,
            };
            refusal(status, refused.code(), &refused.to_string())
        }
        Ok(Err(_)) => refusal(StatusCode::SERVICE_UNAVAILABLE, "bot_stopping", "the bot stopped before it answered"),
        Err(_) => {
            let message = "the account has not answered yet; it may still act on the request";
            refusal(StatusCode::GATEWAY_TIMEOUT, "no_answer_yet", message)
        }
    }
}

/// A JSON request body; one that is not the JSON expected is refused as `bad_request`.
struct Body<T>(T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for Body<T> {
    type Rejection = Response;

    async fn from_request(request: Request, state: &S) -> Result<Self, Response> {
        match Json::<T>::from_request(request, state).await {
            Ok(Json(body)) => Ok(Self(body)),
            Err(e) => Err(refusal(e.status(), "bad_request", &e.body_text())),
        }
    }
}

/// An account's address from the path; one that is not text is refused as `bad_request`.
struct AddressPath(String);

impl<S: Send + Sync> FromRequestParts<S> for AddressPath {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Response> {
        match Path::<String>::from_request_parts(parts, state).await {
            Ok(Path(address)) => Ok(Self(address)),
            Err(e) => Err(refusal(e.status(), "bad_request", &e.body_text())),
        }
    }
}

/// An account's address and the index of one of its strategies, from the path; an index that is
/// not a number is refused as `bad_request`.
struct StrategyPath(String, usize);

impl<S: Send + Sync> FromRequestParts<S> for StrategyPath {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Response> {
        match Path::<(String, usize)>::from_request_parts(parts, state).await {
            Ok(Path((address, strategy))) => Ok(Self(address, strategy)),
            Err(e) => Err(refusal(e.status(), "bad_request", &e.body_text())),
        }
    }
}

/// The answer for a route that does not take the request's method.
async fn not_allowed() -> Response {
    refusal(StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed", "the route does not take this method")
}

/// The answer for an address that is none of the bot's accounts.
fn no_account() -> Response {
    refusal(StatusCode::NOT_FOUND, "no_account", "no such account")
}
