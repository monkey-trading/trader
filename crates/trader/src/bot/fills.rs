//! The accounts' fills in the log: each moves its account's position in the market, and every fill
//! after this start must come from an order this process signed.

use anyhow::anyhow;
use engine_core::Fixed;
use engine_core::command::Command;
use engine_core::eip712::Signature;
use engine_core::event::Event;
use engine_core::types::AccountId;
use serde_json::value::RawValue;
use tracing::{error, info};

use crate::bot::Halt;
use crate::bot::fleet::Fleet;
use crate::stream::message::{CommandMessage, EventHead, FillRef, OrderRef};

/// The events that move an account's cash without a fill, each naming the account.
const CASH_EVENTS: [&str; 8] = [
    "corporate_dividend",
    "corporate_cash_in_lieu",
    "deposited",
    "withdrawal_approved",
    "close_out",
    "bad_debt",
    "financing",
    "distribution_allocated",
];

/// The kinds of event the pinned engine writes (`engine_core::event::Event`'s variants, in snake case),
/// which are skipped without being read. A kind not listed is read as the pinned engine's `Event`,
/// which refuses one the pinned engine does not write: the bot halts on a log of another version.
/// A bump of the engine's `rev` that adds a kind needs it listed here to keep the fast path.
const KNOWN_EVENTS: [&str; 55] = [
    "corporate_action_scheduled",
    "corporate_entitlements_captured",
    "corporate_dividend",
    "corporate_split",
    "corporate_action_applied",
    "corporate_cash_in_lieu",
    "corporate_action_failed",
    "eligibility_attested",
    "eligibility_rejected",
    "deposit_pending",
    "deposit_held",
    "deposited",
    "deposit_rejected",
    "custody_deposit_changed",
    "custody_deposit_rejected",
    "withdrawal_approved",
    "withdrawal_queued",
    "withdrawal_queue_updated",
    "withdrawal_rejected",
    "house_capital_moved",
    "order_accepted",
    "order_rejected",
    "order_cancelled",
    "fill",
    "close_out",
    "bad_debt",
    "financing",
    "benchmark_rate_missing",
    "benchmark_rate_undated",
    "benchmark_rate_observed",
    "market_opened",
    "market_closed",
    "market_rolled",
    "stress_changed",
    "prices_rejected",
    "admin_applied",
    "admin_rejected",
    "emission_allocated",
    "emission_period_closed",
    "emission_periods_cancelled",
    "staked",
    "unstaked",
    "staking_rejected",
    "distribution_allocated",
    "operator_share_allocated",
    "distribution_epoch_closed",
    "distribution_epochs_skipped",
    "rotation_switched",
    "operator_elected",
    "operator_installed",
    "election_rejected",
    "session_key_registered",
    "session_key_revoked",
    "session_key_rejected",
    "catch_up_deferred",
];

impl Fleet {
    /// Follows one event: a fill of one of the fleet's accounts moves its position in the market,
    /// marks its balance for a read, and, unless it is a close-out, must come from an order this
    /// process signed.
    /// Error cases:
    /// - an event or fill that is not the shape the pinned engine writes
    pub(super) fn apply_event(&mut self, message: &CommandMessage<'_>, event: &RawValue) -> Result<(), Halt> {
        let unreadable = |e: serde_json::Error| Halt::Unfollowable(anyhow!(e).context("an event"));
        let head: EventHead<'_> = serde_json::from_str(event.get()).map_err(unreadable)?;
        if let "corporate_split" | "corporate_action_failed" = head.kind {
            return self.corporate_action(message, event);
        }
        if let (true, Some(address)) = (CASH_EVENTS.contains(&head.kind), head.account) {
            // Cash moves without a fill: the balance held is not the account's, and nothing is valued from
            // it, or ordered on it, until a read agrees.
            if let Some(account) = self.account_of(address) {
                let (log_ts, entry) = (self.log_ts, &mut self.accounts[account as usize]);
                entry.epoch += 1;
                entry.behind = true;
                entry.read_after = log_ts;
                self.want_read(account);
            }
            return Ok(());
        }
        if head.kind != "fill" {
            // An event of a kind the pinned engine does not write is a log the bot cannot follow: the
            // engine's own reading of it refuses it. Known kinds are skipped without being read.
            if !KNOWN_EVENTS.contains(&head.kind) {
                serde_json::from_str::<Event>(event.get()).map_err(unreadable)?;
            }
            return Ok(());
        }
        let Some(address) = head.account else {
            return Err(Halt::Unfollowable(anyhow!("a fill event with no account")));
        };
        let Some(account) = self.account_of(address) else { return Ok(()) };
        let fill: FillRef<'_> = serde_json::from_str(event.get()).map_err(unreadable)?;
        let position: Fixed =
            fill.position.parse().map_err(|e| Halt::Unfollowable(anyhow!("a fill's position: {e}")))?;
        if let Some(a) = self.assignment_of(account, fill.market) {
            let before = std::mem::replace(&mut self.assignments[a as usize].position, position);
            self.accounts[account as usize].controls.on_fill(message.ts, fill.market, before, position);
            // The entry price the engine keeps is read again with the balance, which a fill moves.
            self.touch_state(account);
        }
        // The fill changes the position, so the last read of the account (its balance and entry price)
        // is not of the position the log gives: nothing is valued or ordered from it until a read agrees.
        let log_ts = self.log_ts;
        let entry = &mut self.accounts[account as usize];
        entry.epoch += 1;
        entry.behind = true;
        entry.read_after = log_ts;
        self.want_read(account);
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
        if !fill.liquidation {
            if self.signed_here(account, message) {
                // The order has entered the log and filled: it can no longer move the position later.
                if let Some(a) = self.assignment_of(account, fill.market) {
                    self.assignments[a as usize].expires_at = None;
                }
            } else if message.ts > self.trading_from && self.accounts[account as usize].failed.is_none() {
                let reason = format!("a fill at seq {} is from an order this process did not sign", message.seq);
                error!(
                    alert = "account_shared",
                    seq = message.seq,
                    account = fill.account,
                    "{reason}: another process or person trades the account"
                );
                self.fail(account, reason);
            }
        }
        Ok(())
    }

    /// A corporate action on a market the fleet trades. A split changes the units of every price the
    /// strategies have seen, and a failed action stops dealing in the market: the bot keeps the split,
    /// for the accounts' files, and stops.
    /// Error cases:
    /// - the action is a split or a failure on a market an account trades, or an event unreadable
    fn corporate_action(&mut self, message: &CommandMessage<'_>, event: &RawValue) -> Result<(), Halt> {
        let event: Event = serde_json::from_str(event.get())
            .map_err(|e| Halt::Unfollowable(anyhow!(e).context("a corporate action")))?;
        match event {
            Event::CorporateActionFailed { market, reason, .. } if self.market_slot.contains_key(&market) => {
                let reason =
                    format!("a corporate action failed for the market {market}, which stops dealing in it: {reason}");
                Err(Halt::Unfollowable(anyhow!(reason)))
            }
            Event::CorporateSplit { market, .. } => {
                let Some(&slot) = self.market_slot.get(&market) else { return Ok(()) };
                self.markets[slot as usize].split_at = Some(message.ts);
                for k in 0..self.markets[slot as usize].assignments.len() {
                    let account = self.assignments[self.markets[slot as usize].assignments[k] as usize].account;
                    self.touch_state(account);
                }
                let reason = format!("market {market} split, which changes its units: the bot does not follow a split");
                Err(Halt::Unfollowable(anyhow!(reason)))
            }
            _ => Ok(()),
        }
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
            // The close of a stopped account is a close-all request, also signed here.
            Ok(Command::AccessCloseAll { request, .. }) => {
                request.account == entry.address
                    && request.auth.is_some_and(|auth| entry.signed.contains(&auth.signature))
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use engine_core::event::Event;

    use super::{CASH_EVENTS, KNOWN_EVENTS};

    /// Every kind that moves cash is one the bot knows the pinned engine writes.
    #[test]
    fn every_cash_event_kind_is_a_known_one() {
        for kind in CASH_EVENTS {
            assert!(KNOWN_EVENTS.contains(&kind), "{kind}");
        }
    }

    /// Every kind the bot skips unread is a variant the pinned engine writes: the engine's reading of
    /// `{"type": kind}` fails for a missing field of the variant, never as an unknown variant (the
    /// unit variants read whole).
    #[test]
    fn every_known_event_kind_is_the_pinned_engines() {
        for kind in KNOWN_EVENTS {
            if let Err(e) = serde_json::from_str::<Event>(&format!(r#"{{"type":"{kind}"}}"#)) {
                assert!(!e.to_string().contains("unknown variant"), "{kind}: {e}");
            }
        }
        let unknown = serde_json::from_str::<Event>(r#"{"type":"no_such_event"}"#).unwrap_err();
        assert!(unknown.to_string().contains("unknown variant"), "{unknown}");
    }
}
