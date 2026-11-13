//! The funding routes: one read, and one request per action. A request names its accounts and
//! starts a job for each, answered 202 with the jobs; the read shows them as they progress.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use engine_core::Fixed;
use engine_core::access::EligibilityClass;
use engine_core::types::AccountId;
use serde::Deserialize;
use serde_json::json;

use crate::accounts::deposit::Source;
use crate::console::Server;
use crate::console::funding::{Action, Funding, Refused};
use crate::console::guard::refusal;
use crate::console::routes::Body;

/// The funding routes, which the session and `Origin` guards cover with the rest.
pub fn routes() -> Router<Arc<Server>> {
    Router::new()
        .route("/api/funding", get(overview))
        .route("/api/funding/declare", post(declare))
        .route("/api/funding/deposit", post(deposit))
        .route("/api/funding/claim", post(claim))
        .route("/api/funding/withdraw", post(withdraw))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Declare {
    class: EligibilityClass,
    /// The statement the client showed, word for word: the console sends it back to say it was read.
    statement: String,
    accounts: Vec<AccountId>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Deposit {
    accounts: Vec<AccountId>,
    amount: Fixed,
    from: Wallet,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Wallet {
    Account,
    Funding,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Accounts {
    accounts: Vec<AccountId>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Withdraw {
    accounts: Vec<AccountId>,
    /// Without it, only what the vault already approved is paid.
    amount: Option<Fixed>,
}

/// Every account's funding and the jobs.
async fn overview(State(server): State<Arc<Server>>) -> Response {
    match server.funding() {
        Some(funding) => Json(funding.overview()).into_response(),
        None => Json(json!({ "enabled": false })).into_response(),
    }
}

async fn declare(State(server): State<Arc<Server>>, Body(body): Body<Declare>) -> Response {
    if body.statement != body.class.statement() {
        return refusal(StatusCode::UNPROCESSABLE_ENTITY, "statement_mismatch", "the statement is not the class's");
    }
    submit(&server, Action::Declare(body.class), &body.accounts)
}

async fn deposit(State(server): State<Arc<Server>>, Body(body): Body<Deposit>) -> Response {
    let from = match body.from {
        Wallet::Account => Source::Account,
        Wallet::Funding => Source::Funding,
    };
    if let Err(e) = crate::accounts::usdc_units(body.amount) {
        return refusal(StatusCode::UNPROCESSABLE_ENTITY, "bad_amount", &format!("{e:#}"));
    }
    submit(&server, Action::Deposit { amount: body.amount, from }, &body.accounts)
}

async fn claim(State(server): State<Arc<Server>>, Body(body): Body<Accounts>) -> Response {
    submit(&server, Action::Claim, &body.accounts)
}

async fn withdraw(State(server): State<Arc<Server>>, Body(body): Body<Withdraw>) -> Response {
    if let Some(amount) = body.amount
        && let Err(e) = crate::accounts::usdc_units(amount)
    {
        return refusal(StatusCode::UNPROCESSABLE_ENTITY, "bad_amount", &format!("{e:#}"));
    }
    submit(&server, Action::Withdraw(body.amount), &body.accounts)
}

/// Starts `action` for `accounts` and answers with the jobs.
fn submit(server: &Server, action: Action, accounts: &[AccountId]) -> Response {
    let Some(funding): Option<&Arc<Funding>> = server.funding() else {
        let message = "the config has no [chain] section, which funding needs";
        return refusal(StatusCode::CONFLICT, "funding_off", message);
    };
    match funding.submit(action, accounts) {
        Ok(jobs) => (StatusCode::ACCEPTED, Json(json!({ "jobs": jobs }))).into_response(),
        Err(Refused::Accounts(message)) => refusal(StatusCode::UNPROCESSABLE_ENTITY, "bad_accounts", &message),
        Err(Refused::Unknown(account)) => {
            refusal(StatusCode::NOT_FOUND, "no_account", &format!("{account} is not one of the bot's accounts"))
        }
        Err(Refused::Busy(account)) => {
            refusal(StatusCode::CONFLICT, "busy", &format!("{account} has an action that has not ended"))
        }
        Err(Refused::NoFundingKey) => {
            refusal(StatusCode::UNPROCESSABLE_ENTITY, "no_funding_key", "the config names no [funding] key")
        }
        Err(Refused::Stopping) => refusal(StatusCode::SERVICE_UNAVAILABLE, "stopping", "the bot is stopping"),
    }
}
