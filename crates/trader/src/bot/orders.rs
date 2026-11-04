//! The requests the bot signs: a market order toward the strategy's target, and on stopping, a
//! cancel of the account's resting orders and a close of its position. Each carries the account's
//! next nonce, read just before it is signed, and an expiry counted from the node's signed time.

use std::time::Duration;

use engine_core::Fixed;
use engine_core::auth::RequestAuth;
use engine_core::command::{CancelAllRequest, CloseAllRequest, CloseEntry, OrderKind, OrderRequest};
use engine_core::eip712::{cancel_all_hash, close_all_hash, order_hash};
use engine_core::event::Event;
use engine_core::types::Millis;
use tracing::{Instrument, error, info, info_span, warn};
use trader_core::bars::Bar;
use trader_core::strategy::OrderIntent;

use crate::bot::{Bot, Halt};
use crate::node::CallError;

/// How long after the node's time a signed request may still be sequenced: long enough to reach a
/// node over a slow link, and shorter than the shortest bar, so a live bot's next bar is past it.
const EXPIRES_IN_MS: Millis = 30_000;
/// The longest the stop waits for the last order's expiry to pass in the node's time.
const MAX_EXPIRY_WAIT: Duration = Duration::from_millis(EXPIRES_IN_MS as u64 + 5_000);

impl Bot {
    /// Places a signed market order for `intent`, quoted at the last mark, under `bar`'s start as
    /// its client id. A refusal, by the node or the engine, is logged, and the next bar tries again.
    /// Error cases:
    /// - an answer signed by another key than the signer
    pub(super) async fn place(&mut self, intent: OrderIntent, bar: Bar) -> Result<(), Halt> {
        // Bar starts are positive milliseconds since the epoch, one per bar.
        let client_id = u64::try_from(bar.start).unwrap_or_default();
        let span = info_span!("order", id = client_id);
        async {
            let Some(quoted) = self.mark else {
                warn!("no mark to quote the order at");
                return Ok(());
            };
            let Some((nonce, expires_at)) = self.next_nonce().await? else { return Ok(()) };
            let mut request = OrderRequest {
                account: self.key.address(),
                client_id,
                market: self.market,
                side: intent.side,
                qty: intent.qty,
                kind: OrderKind::Market { quoted, max_slippage_bps: Some(self.max_slippage_bps) },
                reduce_only: intent.reduce_only,
                auth: None,
            };
            let hash = order_hash(&self.domain, &request, nonce, expires_at);
            request.auth = Some(RequestAuth { nonce, expires_at, signature: self.key.sign(&hash) });
            // Signed, the order can enter the log until it expires, whatever its answer says.
            self.order_expires_at = Some(expires_at);
            info!(side = ?intent.side, qty = %intent.qty, %quoted, reduce_only = intent.reduce_only, nonce, expires_at, "placing a market order");
            match self.node.place_order(&request).await {
                Ok(answer) => log_order_events(&answer.body),
                Err(CallError::WrongSigner { .. }) => return Err(Halt::WrongSigner),
                Err(e @ CallError::Refused { .. }) => warn!("order declined: {e}"),
                Err(e) => warn!("no answer to the order, which may still enter the log until it expires: {e}"),
            }
            Ok(())
        }
        .instrument(span)
        .await
    }

    /// Cancels the account's resting orders and, when configured, closes its position in the
    /// market, as a verified read of the account shows them once the last order can no longer enter
    /// the log, then reads the account again to confirm it. The close is quoted at the node's mark
    /// as it stands. Without a read to rely on, it cancels anyway and closes the position the log
    /// gave.
    /// Error cases:
    /// - an answer signed by another key than the signer
    /// - orders still rest, or a position it was to close is still held, or neither can be confirmed
    pub(super) async fn stop(&mut self) -> Result<(), Halt> {
        let account = self.key.address();
        let quoted = self.wait_out_last_order().await?.or(self.mark);
        let (resting, held) = match self.read_account().await {
            Ok(view) => (Some(view.orders.len()), view.positions.get(&self.market).map_or(Fixed::ZERO, |p| p.qty)),
            Err(Halt::WrongSigner) => return Err(Halt::WrongSigner),
            Err(e) => {
                warn!(alert = "stop_unread", "the account cannot be read, so the stop acts on what it last knew: {e}");
                (None, self.position)
            }
        };
        if resting != Some(0) {
            info!(?resting, "cancelling the account's resting orders");
            if let Some((nonce, expires_at)) = self.next_nonce().await? {
                let hash = cancel_all_hash(&self.domain, account, nonce, expires_at);
                let auth = Some(RequestAuth { nonce, expires_at, signature: self.key.sign(&hash) });
                match self.node.cancel_all(&CancelAllRequest { account, auth }).await {
                    Ok(answer) => log_order_events(&answer.body),
                    Err(CallError::WrongSigner { .. }) => return Err(Halt::WrongSigner),
                    Err(e) => warn!("the cancel failed: {e}"),
                }
            }
        }
        if let (true, false, Some(quoted)) = (self.close_on_stop, held.is_zero(), quoted) {
            info!(position = %held, %quoted, "closing the position");
            if let Some((nonce, expires_at)) = self.next_nonce().await? {
                let mut request = CloseAllRequest {
                    account,
                    closes: vec![CloseEntry { market: self.market, quoted }],
                    max_slippage_bps: Some(self.max_slippage_bps),
                    auth: None,
                };
                let hash = close_all_hash(&self.domain, &request, nonce, expires_at);
                request.auth = Some(RequestAuth { nonce, expires_at, signature: self.key.sign(&hash) });
                match self.node.close_all(&request).await {
                    Ok(answer) => log_order_events(&answer.body),
                    Err(CallError::WrongSigner { .. }) => return Err(Halt::WrongSigner),
                    Err(e) => warn!("the close failed: {e}"),
                }
            }
        }
        let left = match self.read_account().await {
            Ok(left) => left,
            Err(Halt::WrongSigner) => return Err(Halt::WrongSigner),
            Err(e) => return Err(incomplete(format!("the account cannot be read to confirm it: {e}"))),
        };
        let position = left.positions.get(&self.market).map_or(Fixed::ZERO, |p| p.qty);
        if !left.orders.is_empty() || (self.close_on_stop && !position.is_zero()) {
            return Err(incomplete(format!(
                "{} resting orders and a position of {position} are left",
                left.orders.len()
            )));
        }
        info!(resting_orders = 0, %position, "stopped");
        Ok(())
    }

    /// Waits until the node's time is past the last order's expiry, so that order is in the log or
    /// never will be, and returns the market's mark as the node gives it then, if it gives one.
    /// Error cases:
    /// - an answer signed by another key than the signer
    async fn wait_out_last_order(&self) -> Result<Option<Fixed>, Halt> {
        let mut waited = false;
        loop {
            let markets = match self.node.markets().await {
                Ok(markets) => markets,
                Err(CallError::WrongSigner { .. }) => return Err(Halt::WrongSigner),
                Err(e) => {
                    warn!("the market's mark was not read: {e}");
                    return Ok(None);
                }
            };
            let mark = markets.body.iter().find(|m| m.spec.id == self.market).and_then(|m| m.mark);
            let now = Millis::try_from(markets.time_ms).unwrap_or(Millis::MAX);
            match self.order_expires_at {
                Some(expires_at) if now <= expires_at && !waited => {
                    let wait = Duration::from_millis(u64::try_from(expires_at - now).unwrap_or(0) + 1_000);
                    info!(expires_at, "waiting until the last order can no longer enter the log");
                    tokio::time::sleep(wait.min(MAX_EXPIRY_WAIT)).await;
                    waited = true;
                }
                _ => return Ok(mark),
            }
        }
    }

    /// The account, verified.
    /// Error cases:
    /// - an answer signed by another key than the signer, or none to rely on
    async fn read_account(&self) -> Result<crate::node::api::AccountView, Halt> {
        match self.node.account(self.key.address()).await {
            Ok(answer) => Ok(answer.body),
            Err(CallError::WrongSigner { .. }) => Err(Halt::WrongSigner),
            Err(e) => Err(Halt::Unfollowable(e.into())),
        }
    }

    /// The account's next nonce and an expiry `EXPIRES_IN_MS` after the node's time, or `None`, with
    /// a warning, when the node gives no answer to rely on.
    /// Error cases:
    /// - an answer signed by another key than the signer
    async fn next_nonce(&self) -> Result<Option<(u64, Millis)>, Halt> {
        match self.node.nonce(self.key.address()).await {
            Ok(answer) => match Millis::try_from(answer.time_ms) {
                Ok(now) => Ok(Some((answer.body.next_nonce, now.saturating_add(EXPIRES_IN_MS)))),
                Err(_) => {
                    warn!(time_ms = answer.time_ms, "the node's time is out of range");
                    Ok(None)
                }
            },
            Err(CallError::WrongSigner { .. }) => Err(Halt::WrongSigner),
            Err(e) => {
                warn!("no nonce to sign with: {e}");
                Ok(None)
            }
        }
    }
}

/// A stop that left the account other than it was to leave it, with an alert.
fn incomplete(reason: String) -> Halt {
    error!(alert = "stop_incomplete", "{reason}");
    Halt::StopIncomplete(reason)
}

/// Logs what an order request did: each refusal with the engine's reason, each fill, each accepted
/// or cancelled order.
fn log_order_events(events: &[Event]) {
    for event in events {
        match event {
            Event::OrderRejected { client_id, reason, .. } => warn!(client_id, ?reason, "order declined"),
            Event::OrderAccepted { client_id, order_id, .. } => info!(client_id, order_id, "order accepted"),
            Event::OrderCancelled { order_id, reason, .. } => info!(order_id, ?reason, "order cancelled"),
            Event::Fill(fill) => {
                info!(order_id = fill.order_id, side = ?fill.side, qty = %fill.qty, price = %fill.price, "order filled")
            }
            _ => {}
        }
    }
    if events.is_empty() {
        info!("the request changed nothing");
    }
}
