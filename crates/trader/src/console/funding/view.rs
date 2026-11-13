//! An account's funding as the console reads it. Amounts are decimal strings in whole units.

use engine_core::types::AccountId;
use serde_json::{Value, json};

use crate::accounts::cli::{monkey, units};
use crate::accounts::{AccountStatus, USDC_DECIMALS};

const ETH_DECIMALS: u8 = 18;

/// `status`, for the console.
pub fn status(status: &AccountStatus) -> Value {
    let queued: Vec<Value> =
        status.queued.iter().map(|q| json!({ "position": q.position, "remaining": q.remaining.to_string() })).collect();
    let unpaid: Vec<Value> = status
        .unpaid
        .iter()
        .map(|p| json!({ "nonce": p.nonce, "amount": p.amount.to_string(), "authorized": p.withdrawal.is_some() }))
        .collect();
    json!({
        "address": status.account.to_string(),
        "error": null,
        "declared": status.declared,
        "wallet_eth": units(status.wallet_eth, ETH_DECIMALS),
        "wallet_usdc": units(status.wallet_usdc, USDC_DECIMALS),
        "wallet_monkey": monkey(status.wallet_monkey),
        "balance": status.balance.map(|b| b.to_string()),
        "unclaimed": monkey(status.unclaimed),
        "unclaimed_newest": monkey(status.unclaimed_newest),
        "queued": queued,
        "unpaid": unpaid,
        "payouts_loaded": status.payouts_loaded,
    })
}

/// An account whose status could not be read, and why.
pub fn unread(account: AccountId, reason: &str) -> Value {
    json!({ "address": account.to_string(), "error": reason })
}
