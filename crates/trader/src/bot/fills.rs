//! The accounts' fills in the log: each moves its account's position in the market, and every fill
//! after this start must come from an order this process signed.

use anyhow::anyhow;
use engine_core::Fixed;
use engine_core::command::Command;
use engine_core::eip712::Signature;
use engine_core::types::AccountId;
use serde_json::value::RawValue;
use tracing::{error, info};

use crate::bot::Halt;
use crate::bot::fleet::Fleet;
use crate::stream::message::{CommandMessage, EventHead, FillRef, OrderRef};

impl Fleet {
    /// Follows one event: a fill of one of the fleet's accounts moves its position in the market,
    /// marks its balance for a read, and, unless it is a close-out, must come from an order this
    /// process signed.
    /// Error cases:
    /// - an event or fill that is not the shape the pinned engine writes
    pub(super) fn apply_event(&mut self, message: &CommandMessage<'_>, event: &RawValue) -> Result<(), Halt> {
        let unreadable = |e: serde_json::Error| Halt::Unfollowable(anyhow!(e).context("an event"));
        let head: EventHead<'_> = serde_json::from_str(event.get()).map_err(unreadable)?;
        if head.kind != "fill" {
            return Ok(());
        }
        let Some(account) = head.account.and_then(|address| self.account_of(address)) else { return Ok(()) };
        let fill: FillRef<'_> = serde_json::from_str(event.get()).map_err(unreadable)?;
        let position: Fixed =
            fill.position.parse().map_err(|e| Halt::Unfollowable(anyhow!("a fill's position: {e}")))?;
        if let Some(a) = self.assignment_of(account, fill.market) {
            self.assignments[a as usize].position = position;
        }
        let entry = &mut self.accounts[account as usize];
        if !entry.balance_stale {
            entry.balance_stale = true;
            self.stale_balances.push(account);
        }
        info!(
            seq = message.seq,
            account = %fill.account,
            order_id = fill.order_id,
            market = fill.market,
            side = %fill.side,
            qty = %fill.qty,
            price = %fill.price,
            realised_pnl = %fill.realised_pnl,
            profit_fee = %fill.profit_fee,
            liquidation = fill.liquidation,
            position = %fill.position,
            "fill"
        );
        if !fill.liquidation && message.ts > self.trading_from && !self.signed_here(account, message) {
            let reason = format!("a fill at seq {} is from an order this process did not sign", message.seq);
            error!(
                alert = "account_shared",
                seq = message.seq,
                account = fill.account,
                "{reason}: another process or person trades the account"
            );
            self.fail(account, reason);
        }
        Ok(())
    }

    /// Whether the command of `message` is an order of `account` whose signature this process made.
    fn signed_here(&self, account: u32, message: &CommandMessage<'_>) -> bool {
        let entry = &self.accounts[account as usize];
        let signed = |address: &str, signature: Option<&str>| {
            address.parse::<AccountId>().ok() == Some(entry.address)
                && signature.and_then(|s| s.parse::<Signature>().ok()).is_some_and(|s| entry.signed.contains(&s))
        };
        if message.head().is_ok_and(|head| head.kind == "place_order") {
            let order = serde_json::from_str::<OrderRef<'_>>(message.cmd.get());
            return order.is_ok_and(|order| signed(order.account, order.auth.map(|auth| auth.signature)));
        }
        match message.command() {
            Ok(Command::PlaceOrder(request) | Command::AccessOrder { request, .. }) => {
                request.account == entry.address
                    && request.auth.is_some_and(|auth| entry.signed.contains(&auth.signature))
            }
            _ => false,
        }
    }
}
