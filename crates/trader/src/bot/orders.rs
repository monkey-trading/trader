//! The market order the bot signs toward the strategy's target, on the account's next nonce, read
//! just before it is signed, with an expiry counted from the node's signed time.

use engine_core::auth::RequestAuth;
use engine_core::command::{OrderKind, OrderRequest};
use engine_core::eip712::{Signature, order_hash};
use engine_core::event::{Event, Reject};
use engine_core::types::Millis;
use tracing::{Instrument, info, info_span, warn};
use trader_core::bars::Bar;
use trader_core::strategy::OrderIntent;

use crate::bot::startup::SIGNED_ORDERS;
use crate::bot::{Bot, Halt, REPLAY_LAG_MS};
use crate::node::CallError;
use crate::node::api::AccountView;

/// How long after the node's time a signed request may still be sequenced: long enough to reach a
/// node over a slow link, and shorter than the shortest bar, so a live bot's next bar is past it.
pub(super) const EXPIRES_IN_MS: Millis = 30_000;

impl Bot {
    /// Places a signed market order for `intent`, quoted at the last mark, under `bar`'s start as
    /// its client id, unless the node's time when it gives the nonce is more than `REPLAY_LAG_MS`
    /// past `ts`, the time of the command that closed the bar: the bar is then history, however
    /// promptly its frame was signed. A refusal, by the node or the engine, is logged, and the next
    /// bar tries again.
    /// Error cases:
    /// - an answer signed by another key than the signer
    pub(super) async fn place(&mut self, intent: OrderIntent, bar: Bar, ts: Millis) -> Result<(), Halt> {
        // Bar starts are positive milliseconds since the epoch, one per bar.
        let client_id = u64::try_from(bar.start).unwrap_or_default();
        let span = info_span!("order", id = client_id);
        async {
            let Some(quoted) = self.mark else {
                warn!("no mark to quote the order at");
                return Ok(());
            };
            let Some((nonce, now)) = self.next_nonce().await? else { return Ok(()) };
            if now.saturating_sub(ts) > REPLAY_LAG_MS {
                info!(now, ts, "the bar is history by the node's time: no order");
                return Ok(());
            }
            let expires_at = now.saturating_add(EXPIRES_IN_MS);
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
            let signature = self.key.sign(&hash);
            request.auth = Some(RequestAuth { nonce, expires_at, signature });
            // Signed, the order can enter the log until it expires, whatever its answer says.
            self.order_expires_at = Some(expires_at);
            self.remember_signature(signature);
            self.close_first = false;
            info!(side = ?intent.side, qty = %intent.qty, %quoted, reduce_only = intent.reduce_only, nonce, expires_at, "placing a market order");
            self.metrics.orders_placed.inc();
            match self.node.place_order(&request).await {
                Ok(answer) => {
                    log_order_events(&answer.body);
                    if answer.body.iter().any(|event| matches!(event, Event::OrderRejected { .. })) {
                        self.metrics.orders_declined.inc();
                    }
                    // The pure close passes where a reversal's blended spread does not.
                    self.close_first = answer.body.iter().any(|event| {
                        matches!(event, Event::OrderRejected { reason: Reject::CloseSpreadCeiling { .. }, .. })
                    });
                }
                Err(CallError::WrongSigner { .. }) => return Err(Halt::WrongSigner),
                Err(e @ CallError::Refused { .. }) => {
                    self.metrics.orders_declined.inc();
                    warn!("order declined: {e}");
                }
                Err(e) => warn!("no answer to the order, which may still enter the log until it expires: {e}"),
            }
            Ok(())
        }
        .instrument(span)
        .await
    }

    /// Remembers `signature` as one this process signed, to tell its fills from others'.
    pub(super) fn remember_signature(&mut self, signature: Signature) {
        if self.signed_orders.len() == SIGNED_ORDERS {
            self.signed_orders.pop_front();
        }
        self.signed_orders.push_back(signature);
    }

    /// Why no order is signed for the command at `ts`, if an earlier one may still enter the log and
    /// move the position: this process's last, or one an earlier process signed before this start.
    pub(super) fn order_in_flight(&self, ts: Millis) -> Option<&'static str> {
        if self.order_expires_at.is_some_and(|expires_at| ts <= expires_at) {
            return Some("the last order may still enter the log");
        }
        (ts <= self.trading_from).then_some("an order signed before this start may still enter the log")
    }

    /// The account, verified.
    /// Error cases:
    /// - an answer signed by another key than the signer, or none to rely on
    pub(super) async fn read_account(&self) -> Result<AccountView, Halt> {
        match self.node.account(self.key.address()).await {
            Ok(answer) => Ok(answer.body),
            Err(CallError::WrongSigner { .. }) => Err(Halt::WrongSigner),
            Err(e) => Err(Halt::Unfollowable(e.into())),
        }
    }

    /// The account's next nonce and the node's time when it gave it, or `None`, with a warning,
    /// when the node gives no answer to rely on.
    /// Error cases:
    /// - an answer signed by another key than the signer
    pub(super) async fn next_nonce(&self) -> Result<Option<(u64, Millis)>, Halt> {
        match self.node.nonce(self.key.address()).await {
            Ok(answer) => match Millis::try_from(answer.time_ms) {
                Ok(now) => Ok(Some((answer.body.next_nonce, now))),
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

/// Logs what an order request did: each refusal with the engine's reason, each fill, each accepted
/// or cancelled order.
pub(super) fn log_order_events(events: &[Event]) {
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
