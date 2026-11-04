//! The node's routes the bot uses, with the shapes the pinned engine serves. Reads take what the
//! bot needs and ignore the rest; requests are the engine's own types, signed by the caller.

use std::collections::BTreeMap;

use anyhow::anyhow;
use engine_core::Fixed;
use engine_core::account::{Position, RestingOrder};
use engine_core::command::{CancelAllRequest, CloseAllRequest, OrderRequest};
use engine_core::event::Event;
use engine_core::market::MarketSpec;
use engine_core::types::{AccountId, MarketId, Millis, OrderId};
use serde::Deserialize;

use crate::node::{CallError, Node, Verified};

/// The most marks one page of `GET /v1/markets/{id}/marks` carries.
const MARKS_PAGE: usize = 2000;

/// One market as `GET /v1/markets` lists it.
#[derive(Debug, Deserialize)]
pub struct MarketView {
    #[serde(flatten)]
    pub spec: MarketSpec,
    pub mark: Option<Fixed>,
}

/// One account as `GET /v1/accounts/{address}` shows it.
#[derive(Debug, Deserialize)]
pub struct AccountView {
    pub balance: Fixed,
    pub positions: BTreeMap<MarketId, Position>,
    pub orders: BTreeMap<OrderId, RestingOrder>,
}

/// `GET /v1/accounts/{address}/nonce`.
#[derive(Debug, Deserialize)]
pub struct NonceView {
    pub next_nonce: u64,
}

/// One one-minute bucket of a market's index marks: its start and its last mark.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub struct MarkBucket {
    pub ts: Millis,
    pub close: Fixed,
}

/// One page of `GET /v1/markets/{id}/marks`.
#[derive(Debug, Deserialize)]
struct MarksPage {
    entries: Vec<MarkBucket>,
    next: Option<Millis>,
}

impl Node {
    /// Every listed market.
    /// Error cases:
    /// - see `CallError`
    pub async fn markets(&self) -> Result<Verified<Vec<MarketView>>, CallError> {
        self.get("/v1/markets").await
    }

    /// `account`'s balance, positions, and resting orders; a refusal with 404 for an account the
    /// node has never credited.
    /// Error cases:
    /// - see `CallError`
    pub async fn account(&self, account: AccountId) -> Result<Verified<AccountView>, CallError> {
        self.get(&format!("/v1/accounts/{account}")).await
    }

    /// The nonce `account`'s next signed request must carry.
    /// Error cases:
    /// - see `CallError`
    pub async fn nonce(&self, account: AccountId) -> Result<Verified<NonceView>, CallError> {
        self.get(&format!("/v1/accounts/{account}/nonce")).await
    }

    /// `market`'s one-minute mark buckets that start in [`from_ms`, `to_ms`], oldest first, each
    /// page verified.
    /// Error cases:
    /// - see `CallError`
    /// - a page whose `next` does not move past the window it answered
    pub async fn marks(&self, market: MarketId, from_ms: Millis, to_ms: Millis) -> Result<Vec<MarkBucket>, CallError> {
        let mut buckets = Vec::new();
        let mut from = from_ms;
        loop {
            let path =
                format!("/v1/markets/{market}/marks?resolution=1m&from_ms={from}&to_ms={to_ms}&limit={MARKS_PAGE}");
            let page: MarksPage = self.get(&path).await?.body;
            buckets.extend(page.entries.into_iter().filter(|b| (from..=to_ms).contains(&b.ts)));
            match page.next {
                None => return Ok(buckets),
                Some(next) if next > from && next <= to_ms => from = next,
                Some(next) => {
                    let source = anyhow!("the next page starts at {next}, outside ({from}, {to_ms}]");
                    return Err(CallError::Failed { target: format!("GET {path}"), source });
                }
            }
        }
    }

    /// Places a signed order; the events it produced.
    /// Error cases:
    /// - see `CallError`
    pub async fn place_order(&self, request: &OrderRequest) -> Result<Verified<Vec<Event>>, CallError> {
        self.post("/v1/orders", request).await
    }

    /// Cancels every resting order of a signed request's account.
    /// Error cases:
    /// - see `CallError`
    pub async fn cancel_all(&self, request: &CancelAllRequest) -> Result<Verified<Vec<Event>>, CallError> {
        self.post("/v1/orders/cancel-all", request).await
    }

    /// Closes the positions a signed request names.
    /// Error cases:
    /// - see `CallError`
    pub async fn close_all(&self, request: &CloseAllRequest) -> Result<Verified<Vec<Event>>, CallError> {
        self.post("/v1/positions/close-all", request).await
    }
}
