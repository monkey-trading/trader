//! The stand-in's account reads beyond the bot's own: an account's fill history, paged as the node
//! pages it, and its MONKEY entitlement.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use super::StandIn;

/// One page of an account's fills, newest first, strictly between the cursors `after` and `before`.
pub async fn history(
    State(s): State<Arc<StandIn>>,
    Path(address): Path<String>,
    Query(query): Query<BTreeMap<String, String>>,
) -> Response {
    assert_eq!(query.get("kind").map(String::as_str), Some("fill"), "the console reads fills only");
    let cursor = |text: &str| {
        let (seq, index) = text.split_once(':').unwrap();
        (seq.parse::<u64>().unwrap(), index.parse::<u32>().unwrap())
    };
    let after = query.get("after").map(|c| cursor(c));
    let before = query.get("before").map(|c| cursor(c));
    let limit: usize = query["limit"].parse().unwrap();
    let (events, next) = s.with(|i| {
        i.history_reads += 1;
        let page = limit.min(i.history_page.unwrap_or(limit));
        let window: Vec<Value> = i
            .history
            .get(&address.to_lowercase())
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|event| {
                let at = cursor(event["cursor"].as_str().unwrap());
                after.is_none_or(|after| at > after) && before.is_none_or(|before| at < before)
            })
            .collect();
        let next = (window.len() > page).then(|| window[page - 1]["cursor"].clone());
        (window.into_iter().take(page).collect::<Vec<_>>(), next)
    });
    axum::Json(json!({ "events": events, "next": next })).into_response()
}

/// The account's entitlement, or 404 while it has none.
pub async fn emissions(State(s): State<Arc<StandIn>>, Path(address): Path<String>) -> Response {
    match s.with(|i| i.emissions.get(&address.to_lowercase()).cloned()) {
        Some(answer) => axum::Json(answer).into_response(),
        None => (StatusCode::NOT_FOUND, axum::Json(json!({ "error": "not found" }))).into_response(),
    }
}

/// A fill of `account` in its history at `cursor`, realising `pnl` with a profit fee of `fee`.
pub fn history_fill(account: &str, cursor: &str, ts: i64, side: &str, pnl: &str, fee: &str) -> Value {
    json!({ "cursor": cursor, "ts": ts, "type": "fill", "account": account, "order_id": 1, "market": 1,
        "side": side, "qty": "1", "price": "100", "index": "100", "spread_bps": 0, "realised_pnl": pnl,
        "profit_fee": fee, "position": "0", "liquidation": false })
}

/// The emissions answer naming `entitled` at the accepted checkpoint and `newest` at the newest.
pub fn emissions_answer(account: &str, entitled: &str, newest: &str) -> Value {
    json!({ "seq": 40, "root": format!("0x{}", "00".repeat(32)), "account": account, "entitled": entitled,
        "siblings": [], "accepted_seq": 40, "newest_seq": 50, "newest_entitled": newest, "claimable": true })
}
