//! The stop: once the log has passed every order that could still enter it, a cancel of the
//! account's resting orders and, when configured, a close of its position, each signed on the
//! account's next nonce, then a verified read that confirms what is left.

use std::time::Duration;

use engine_core::Fixed;
use engine_core::auth::RequestAuth;
use engine_core::command::{CancelAllRequest, CloseAllRequest, CloseEntry};
use engine_core::eip712::{cancel_all_hash, close_all_hash};
use engine_core::types::Millis;
use tokio::time::{Instant, timeout_at};
use tracing::{Instrument, error, info, info_span, warn};
use trader_core::hours::is_open;

use crate::bot::orders::{EXPIRES_IN_MS, log_order_events};
use crate::bot::{Bot, Halt};
use crate::node::CallError;
use crate::stream::message::{CommandMessage, Head};
use crate::stream::{Stream, StreamError};

/// The longest the stop goes on closing a position that one close did not take whole.
const MAX_CLOSE_WAIT: Duration = Duration::from_secs(60);

/// The longest the stop waits for the log to pass the last order's expiry.
const MAX_EXPIRY_WAIT: Duration = Duration::from_millis(EXPIRES_IN_MS as u64 + 5_000);

impl Bot {
    /// Cancels the account's resting orders and, when configured, closes its position in the
    /// market, as a verified read of the account shows them once the log has passed every order that
    /// could still enter it, then reads the account again to confirm it. The close is quoted at the
    /// node's mark as it stands. Without a read to rely on, it cancels anyway and closes the position
    /// the log gave.
    /// Error cases:
    /// - an answer or frame signed by another key than the signer
    /// - orders still rest, a position it was to close is still held, or neither can be confirmed
    pub(super) async fn stop(&mut self, stream: &mut Stream) -> Result<(), Halt> {
        let account = self.key.address();
        let settled = self.wait_for_log_past(stream).await?;
        let quoted = self.node_mark().await?.or(self.mark);
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
            if let Some((nonce, now)) = self.next_nonce().await? {
                let expires_at = now.saturating_add(EXPIRES_IN_MS);
                let hash = cancel_all_hash(&self.domain, account, nonce, expires_at);
                let auth = Some(RequestAuth { nonce, expires_at, signature: self.key.sign(&hash) });
                match self.node.cancel_all(&CancelAllRequest { account, auth }).await {
                    Ok(answer) => log_order_events(&answer.body),
                    Err(CallError::WrongSigner { .. }) => return Err(Halt::WrongSigner),
                    Err(e) => warn!("the cancel failed: {e}"),
                }
            }
        }
        let closing = self.close_on_stop && !held.is_zero() && quoted.is_some();
        let deadline = Instant::now() + MAX_CLOSE_WAIT;
        let mut before = held;
        let left = loop {
            if closing {
                let quoted = self.node_mark().await?.or(quoted).or(self.mark);
                if let Some(quoted) = quoted {
                    self.close_position(before, quoted).await?;
                }
            }
            let left = match self.read_account().await {
                Ok(left) => left,
                Err(Halt::WrongSigner) => return Err(Halt::WrongSigner),
                Err(e) => return Err(incomplete(format!("the account cannot be read to confirm it: {e}"))),
            };
            let remaining = left.positions.get(&self.market).map_or(Fixed::ZERO, |p| p.qty);
            // One close takes at most `Fixed::MAX_INPUT` units, so a confirmed close that leaves
            // part of the position is followed by another on a fresh nonce; one that changed
            // nothing is not repeated.
            if closing && !remaining.is_zero() && remaining.abs() < before.abs() && Instant::now() < deadline {
                before = remaining;
                continue;
            }
            break left;
        };
        let position = left.positions.get(&self.market).map_or(Fixed::ZERO, |p| p.qty);
        if !left.orders.is_empty() || (self.close_on_stop && !position.is_zero()) {
            let orders = left.orders.len();
            return Err(incomplete(format!("{orders} resting orders and a position of {position} are left")));
        }
        if !settled {
            return Err(incomplete("the log was not seen past the last order's expiry".into()));
        }
        info!(resting_orders = 0, %position, "stopped");
        Ok(())
    }

    /// Reads the log until a command sequenced after the latest expiry an order signed before the
    /// stop can carry, this process's or an earlier one's, for at most `MAX_EXPIRY_WAIT`. The log
    /// holds only what is durable, so once it is past that expiry every such order is in it, and in
    /// the account a read then gives, or never will be. Returns whether the log got there.
    /// Error cases:
    /// - a frame signed by another key than the signer
    async fn wait_for_log_past(&mut self, stream: &mut Stream) -> Result<bool, Halt> {
        let barrier = self.order_expires_at.map_or(self.trading_from, |expires_at| expires_at.max(self.trading_from));
        if self.log_ts > barrier {
            return Ok(true);
        }
        info!(barrier, "waiting until the log passes every order that could still enter it");
        let deadline = Instant::now() + MAX_EXPIRY_WAIT;
        loop {
            let frame = match timeout_at(deadline, stream.next()).await {
                Err(_) => return Ok(false),
                Ok(Err(StreamError::WrongSigner)) => return Err(Halt::WrongSigner),
                Ok(Err(StreamError::Unreadable(e))) => {
                    warn!("the log cannot be read on: {e:#}");
                    return Ok(false);
                }
                Ok(Ok(frame)) => frame,
            };
            let ts = serde_json::from_str::<Head<'_>>(&frame.msg)
                .ok()
                .filter(|head| head.kind == "command")
                .and_then(|_| serde_json::from_str::<CommandMessage<'_>>(&frame.msg).ok())
                .map(|message| message.ts);
            if let Some(ts) = ts {
                self.log_ts = ts;
                if ts > barrier {
                    return Ok(true);
                }
            }
        }
    }

    /// Signs and sends one close of the position `held`, on the account's next nonce, quoted at
    /// `quoted`. A failed close is logged: the account read that follows shows what it did.
    /// Error cases:
    /// - an answer signed by another key than the signer
    pub(super) async fn close_position(&mut self, held: Fixed, quoted: Fixed) -> Result<(), Halt> {
        let account = self.key.address();
        info!(position = %held, %quoted, "closing the position");
        let Some((nonce, now)) = self.next_nonce().await? else { return Ok(()) };
        let expires_at = now.saturating_add(EXPIRES_IN_MS);
        let mut request = CloseAllRequest {
            account,
            closes: vec![CloseEntry { market: self.market, quoted }],
            max_slippage_bps: Some(self.max_slippage_bps),
            auth: None,
        };
        let hash = close_all_hash(&self.domain, &request, nonce, expires_at);
        let signature = self.key.sign(&hash);
        request.auth = Some(RequestAuth { nonce, expires_at, signature });
        self.order_expires_at = Some(expires_at);
        self.remember_signature(signature);
        // The expiry names the try, so the lines of a close and its answer join.
        let span = info_span!("close", id = expires_at);
        async {
            match self.node.close_all(&request).await {
                Ok(answer) => log_order_events(&answer.body),
                Err(CallError::WrongSigner { .. }) => return Err(Halt::WrongSigner),
                Err(e) => warn!("the close failed: {e}"),
            }
            Ok(())
        }
        .instrument(span)
        .await
    }

    /// Closes the position at the last mark, as the daily loss limit does at `ts`, unless the market
    /// is shut, or an order signed earlier may still enter the log and move the position.
    /// Error cases:
    /// - an answer signed by another key than the signer
    pub(super) async fn flatten(&mut self, ts: Millis) -> Result<(), Halt> {
        if self.position.is_zero() {
            return Ok(());
        }
        if !is_open(&self.spec.schedule, ts) {
            info!("the market is shut: the position is closed when it opens");
            return Ok(());
        }
        if let Some(reason) = self.order_in_flight(ts) {
            info!("{reason}: the position is closed at a later bar");
            return Ok(());
        }
        let Some(quoted) = self.mark else {
            warn!("no mark to quote the close at");
            return Ok(());
        };
        self.close_position(self.position, quoted).await
    }

    /// The market's mark as the node gives it now, if it gives one.
    /// Error cases:
    /// - an answer signed by another key than the signer
    async fn node_mark(&self) -> Result<Option<Fixed>, Halt> {
        match self.node.markets().await {
            Ok(markets) => Ok(markets.body.iter().find(|m| m.spec.id == self.market).and_then(|m| m.mark)),
            Err(CallError::WrongSigner { .. }) => Err(Halt::WrongSigner),
            Err(e) => {
                warn!("the market's mark was not read: {e}");
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
