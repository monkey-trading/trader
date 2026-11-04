//! The account's fills in the log: its position in the market, its balance read again after each,
//! and a check that every fill after this start comes from an order this process signed.

use anyhow::anyhow;
use engine_core::Fixed;
use engine_core::command::Command;
use engine_core::event::Event;
use engine_core::types::Millis;
use tracing::{debug, error, info, warn};

use crate::bot::{Bot, Halt};
use crate::node::CallError;
use crate::stream::message::CommandMessage;

impl Bot {
    /// Follows the account's fills: the position in the market, and when it opened. Returns whether
    /// `event` is one, or moves the account's cash or the position without a fill, as a dividend or
    /// a split does.
    pub(super) fn on_event(&mut self, seq: u64, ts: Millis, event: &Event) -> bool {
        match event {
            Event::CorporateDividend { account, .. } | Event::CorporateCashInLieu { account, .. }
                if *account == self.key.address() =>
            {
                return true;
            }
            Event::CorporateSplit { market, .. } if *market == self.market => {
                self.resync_position = true;
                return true;
            }
            _ => {}
        }
        let Event::Fill(fill) = event else { return false };
        if fill.account != self.key.address() {
            return false;
        }
        if fill.market == self.market {
            let before = std::mem::replace(&mut self.position, fill.position);
            self.controls.on_fill(ts, fill.market, before, fill.position);
        }
        info!(
            seq,
            order_id = fill.order_id,
            market = fill.market,
            side = ?fill.side,
            qty = %fill.qty,
            price = %fill.price,
            realised_pnl = %fill.realised_pnl,
            profit_fee = %fill.profit_fee,
            liquidation = fill.liquidation,
            position = %self.position,
            "fill"
        );
        true
    }

    /// Checks that the account's fills in `message` come from an order this process signed, or a
    /// close-out, once orders signed before this start can no longer enter the log. A fill from
    /// another order means another process, or a person, trades the account, and the position the
    /// bot decides on is not its own: it stops.
    /// Error cases:
    /// - a fill of the account from an order whose signature this process did not make
    pub(super) fn check_own_fills(&self, message: &CommandMessage<'_>, events: &[Event]) -> Result<(), Halt> {
        if message.ts <= self.trading_from {
            return Ok(());
        }
        let account = self.key.address();
        let traded = events
            .iter()
            .any(|event| matches!(event, Event::Fill(fill) if fill.account == account && !fill.liquidation));
        if !traded {
            return Ok(());
        }
        let signed_here = match message.command() {
            Ok(Command::PlaceOrder(request) | Command::AccessOrder { request, .. }) if request.account == account => {
                request.auth.as_ref().is_some_and(|auth| self.signed_orders.contains(&auth.signature))
            }
            Ok(Command::AccessCloseAll { request, .. }) if request.account == account => {
                request.auth.as_ref().is_some_and(|auth| self.signed_orders.contains(&auth.signature))
            }
            _ => false,
        };
        if signed_here {
            return Ok(());
        }
        let reason = format!("a fill at seq {} is from an order this process did not sign", message.seq);
        error!(alert = "account_shared", seq = message.seq, "{reason}: another process or person trades the account");
        Err(Halt::Unfollowable(anyhow!("{reason}")))
    }

    /// Reads the account again, verified, after the account's fills: fills, financing, fees and
    /// transfers all move the balance, and the node's answer is the one figure that holds all of them;
    /// it also gives the position's entry price. A read whose position is not the log's is behind it,
    /// and gives neither figure until a later read agrees; after a split, which moves the position
    /// without a fill, the position is the read's own.
    /// Error cases:
    /// - an answer signed by another key than the signer
    pub(super) async fn refresh_balance(&mut self) -> Result<(), Halt> {
        match self.node.account(self.key.address()).await {
            Ok(answer) => {
                let held = answer.body.positions.get(&self.market);
                let read = held.map_or(Fixed::ZERO, |position| position.qty);
                if std::mem::take(&mut self.resync_position) {
                    self.position = read;
                    info!(position = %self.position, "the position after a split");
                }
                if read != self.position {
                    self.account_behind = true;
                    debug!(read = %read, position = %self.position, "the account read is behind the log");
                    return Ok(());
                }
                self.account_behind = false;
                self.balance = answer.body.balance;
                self.entry = held.map_or(Fixed::ZERO, |position| position.entry);
                info!(balance = %self.balance, head_seq = answer.head_seq, "balance");
            }
            Err(CallError::WrongSigner { .. }) => return Err(Halt::WrongSigner),
            Err(e) => {
                self.account_behind = true;
                warn!("the balance was not read: {e}");
            }
        }
        Ok(())
    }
}
