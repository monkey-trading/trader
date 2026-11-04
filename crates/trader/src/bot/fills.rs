//! The account's fills in the log: its position in the market, its balance read again after each,
//! and a check that every fill after this start comes from an order this process signed.

use anyhow::anyhow;
use engine_core::command::Command;
use engine_core::event::Event;
use tracing::{error, info, warn};

use crate::bot::{Bot, Halt};
use crate::node::CallError;
use crate::stream::message::CommandMessage;

impl Bot {
    /// Follows the account's fills: the position in the market. Returns whether `event` is one.
    pub(super) fn on_event(&mut self, seq: u64, event: &Event) -> bool {
        let Event::Fill(fill) = event else { return false };
        if fill.account != self.key.address() {
            return false;
        }
        if fill.market == self.market {
            self.position = fill.position;
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
            _ => false,
        };
        if signed_here {
            return Ok(());
        }
        let reason = format!("a fill at seq {} is from an order this process did not sign", message.seq);
        error!(alert = "account_shared", seq = message.seq, "{reason}: another process or person trades the account");
        Err(Halt::Unfollowable(anyhow!("{reason}")))
    }

    /// Reads the balance again, verified, after the account's fills: fills, financing, fees and
    /// transfers all move it, and the node's answer is the one figure that holds all of them.
    /// Error cases:
    /// - an answer signed by another key than the signer
    pub(super) async fn refresh_balance(&mut self) -> Result<(), Halt> {
        match self.node.account(self.key.address()).await {
            Ok(answer) => {
                self.balance = answer.body.balance;
                info!(balance = %self.balance, head_seq = answer.head_seq, "balance");
            }
            Err(CallError::WrongSigner { .. }) => return Err(Halt::WrongSigner),
            Err(e) => warn!("the balance was not read: {e}"),
        }
        Ok(())
    }
}
