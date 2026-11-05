//! The stop: once the log has passed every order that could still enter it, a cancel of each
//! account's resting orders and, when configured, a close of its positions in its markets, each
//! signed on the account's next nonce, then a verified read that confirms what is left. Accounts
//! stop side by side under the node's pacing; one that fails to stop does not hold back the others.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use engine_core::Fixed;
use engine_core::auth::RequestAuth;
use engine_core::command::{CancelAllRequest, CloseAllRequest, CloseEntry};
use engine_core::eip712::{Eip712Domain, cancel_all_hash, close_all_hash};
use engine_core::types::{AccountId, MarketId, Millis};
use futures_util::future::join_all;
use tokio::time::{Instant, timeout_at};
use tracing::{error, info, warn};

use crate::bot::Halt;
use crate::bot::fleet::EXPIRES_IN_MS;
use crate::bot::orders::log_order_events;
use crate::bot::run::Runner;
use crate::keys::AccountKey;
use crate::node::{CallError, Node};
use crate::stream::message::{CommandMessage, Head};
use crate::stream::{Stream, StreamError};

/// The longest the stop goes on closing a position that one close did not take whole.
const MAX_CLOSE_WAIT: Duration = Duration::from_secs(60);

/// The longest the stop waits for the log to pass the last order's expiry.
const MAX_EXPIRY_WAIT: Duration = Duration::from_millis(EXPIRES_IN_MS as u64 + 5_000);

/// What one account's stop needs.
struct Stop {
    node: Arc<Node>,
    domain: Eip712Domain,
    key: Arc<AccountKey>,
    close_on_stop: bool,
    /// Each market the account trades, and the position the log last gave.
    markets: Vec<(MarketId, Fixed)>,
    max_slippage_bps: i64,
    quotes: Arc<BTreeMap<MarketId, Option<Fixed>>>,
}

/// Why an account's stop did not complete.
enum Failed {
    WrongSigner,
    Incomplete(String),
}

impl Runner {
    /// Ends the run on a stop signal: lets the requests out finish, drops the decisions still queued,
    /// waits for the log to pass every order that could still enter it, then stops each account.
    /// Error cases:
    /// - an answer or frame signed by another key than the signer
    /// - an account still has resting orders, or a position it was to close, or neither can be
    ///   confirmed
    pub async fn stop(&mut self, stream: &mut Stream) -> Result<(), Halt> {
        for a in std::mem::take(&mut self.fleet.queue) {
            let asg = &mut self.fleet.assignments[a as usize];
            (asg.pending, asg.queued) = (None, false);
        }
        while let Some(done) = futures_util::StreamExt::next(&mut self.in_flight).await {
            self.handle_stopping(done)?;
        }
        let settled = self.wait_for_log_past(stream).await?;
        let last: BTreeMap<_, _> = self.fleet.markets.iter().map(|m| (m.id, m.mark)).collect();
        let quotes = Arc::new(node_marks(&self.node, last).await?);
        let stops: Vec<_> = self
            .fleet
            .accounts
            .iter()
            .filter(|account| account.failed.is_none())
            .map(|account| {
                let markets: Vec<_> = account
                    .assignments
                    .clone()
                    .map(|a| &self.fleet.assignments[a as usize])
                    .map(|asg| (asg.market, asg.position))
                    .collect();
                let max_slippage_bps =
                    account.assignments.clone().map(|a| self.fleet.assignments[a as usize].max_slippage_bps).max();
                Stop {
                    node: Arc::clone(&self.node),
                    domain: self.domain.clone(),
                    key: Arc::clone(&account.key),
                    close_on_stop: account.close_on_stop,
                    markets,
                    max_slippage_bps: max_slippage_bps.unwrap_or_default(),
                    quotes: Arc::clone(&quotes),
                }
            })
            .collect();
        let mut incomplete = Vec::new();
        for result in join_all(stops.into_iter().map(Stop::run)).await {
            match result {
                Ok(()) => {}
                Err(Failed::WrongSigner) => return Err(Halt::WrongSigner),
                Err(Failed::Incomplete(reason)) => incomplete.push(reason),
            }
        }
        if !incomplete.is_empty() {
            let (count, reasons) = (incomplete.len(), incomplete.join("; "));
            return Err(Halt::StopIncomplete(format!(
                "{count} accounts were not left as the stop sets out: {reasons}"
            )));
        }
        if !settled {
            let reason = "the log was not seen past the last order's expiry";
            error!(alert = "stop_incomplete", "{reason}");
            return Err(Halt::StopIncomplete(reason.into()));
        }
        info!(accounts = self.fleet.accounts.len(), "stopped");
        Ok(())
    }

    /// Applies an answer that arrives while stopping, which only records what the account's request
    /// did: no request follows.
    /// Error cases:
    /// - an answer signed by another key than the signer
    fn handle_stopping(&mut self, done: crate::bot::run::Done) -> Result<(), Halt> {
        self.handle(done)
    }

    /// Reads the log until a command sequenced after the latest expiry an order signed before the
    /// stop can carry, this process's or an earlier one's, for at most `MAX_EXPIRY_WAIT`. The log
    /// holds only what is durable, so once it is past that expiry every such order is in it, and in
    /// the accounts a read then gives, or never will be. Returns whether the log got there.
    /// Error cases:
    /// - a frame signed by another key than the signer
    async fn wait_for_log_past(&mut self, stream: &mut Stream) -> Result<bool, Halt> {
        let barrier =
            self.fleet.assignments.iter().filter_map(|a| a.expires_at).fold(self.fleet.trading_from, Millis::max);
        if self.fleet.log_ts > barrier {
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
                self.fleet.log_ts = ts;
                if ts > barrier {
                    return Ok(true);
                }
            }
        }
    }
}

/// Each traded market's mark as the node gives it now, where it gives one; `last`, the log's last,
/// otherwise.
/// Error cases:
/// - an answer signed by another key than the signer
async fn node_marks(
    node: &Node,
    last: BTreeMap<MarketId, Option<Fixed>>,
) -> Result<BTreeMap<MarketId, Option<Fixed>>, Halt> {
    match node.markets().await {
        Ok(listed) => Ok(last
            .into_iter()
            .map(|(id, mark)| (id, listed.body.iter().find(|m| m.spec.id == id).and_then(|m| m.mark).or(mark)))
            .collect()),
        Err(CallError::WrongSigner { .. }) => Err(Halt::WrongSigner),
        Err(e) => {
            warn!("the markets' marks were not read: {e}");
            Ok(last)
        }
    }
}

impl Stop {
    /// Stops the account: cancels its resting orders, closes its positions when configured, and
    /// confirms with a read.
    async fn run(self) -> Result<(), Failed> {
        let account = self.key.address();
        let result = self.stop(account).await;
        if let Err(Failed::Incomplete(reason)) = &result {
            error!(alert = "stop_incomplete", %account, "{reason}");
        }
        result
    }

    async fn stop(&self, account: AccountId) -> Result<(), Failed> {
        let (resting, held) = match self.read(account).await {
            Ok(view) => (
                Some(view.orders.len()),
                self.markets.iter().map(|(m, _)| (*m, view.positions.get(m).map_or(Fixed::ZERO, |p| p.qty))).collect(),
            ),
            Err(Failed::WrongSigner) => return Err(Failed::WrongSigner),
            Err(Failed::Incomplete(e)) => {
                warn!(alert = "stop_unread", %account, "the account cannot be read, so the stop acts on what it last knew: {e}");
                (None, self.markets.clone())
            }
        };
        if resting != Some(0) {
            info!(%account, ?resting, "cancelling the account's resting orders");
            if let Some((nonce, now)) = self.nonce(account).await? {
                let expires_at = now.saturating_add(EXPIRES_IN_MS);
                let hash = cancel_all_hash(&self.domain, account, nonce, expires_at);
                let auth = Some(RequestAuth { nonce, expires_at, signature: self.key.sign(&hash) });
                match self.node.cancel_all(&CancelAllRequest { account, auth }).await {
                    Ok(answer) => log_order_events(&answer.body),
                    Err(CallError::WrongSigner { .. }) => return Err(Failed::WrongSigner),
                    Err(e) => warn!(%account, "the cancel failed: {e}"),
                }
            }
        }
        // One close takes at most `Fixed::MAX_INPUT` units of a position, so a confirmed close that
        // leaves part of one is followed by another on a fresh nonce; one that changed nothing is not
        // repeated.
        let deadline = Instant::now() + MAX_CLOSE_WAIT;
        let mut before = held;
        let left = loop {
            let closes: Vec<CloseEntry> = before
                .iter()
                .filter(|(_, qty): &&(MarketId, Fixed)| !qty.is_zero())
                .filter_map(|(market, _)| Some(CloseEntry { market: *market, quoted: (*self.quotes.get(market)?)? }))
                .collect();
            let closing = self.close_on_stop && !closes.is_empty();
            if closing {
                self.close(account, closes).await?;
            }
            let left = match self.read(account).await {
                Ok(left) => left,
                Err(Failed::WrongSigner) => return Err(Failed::WrongSigner),
                Err(Failed::Incomplete(e)) => {
                    return Err(Failed::Incomplete(format!("the account cannot be read to confirm it: {e}")));
                }
            };
            let remaining: Vec<(MarketId, Fixed)> =
                before.iter().map(|(m, _)| (*m, left.positions.get(m).map_or(Fixed::ZERO, |p| p.qty))).collect();
            let progressed =
                before.iter().zip(&remaining).any(|((_, was), (_, now))| !now.is_zero() && now.abs() < was.abs());
            if closing && progressed && Instant::now() < deadline {
                before = remaining;
                continue;
            }
            break left;
        };
        let open: Vec<_> = self
            .markets
            .iter()
            .filter_map(|(m, _)| left.positions.get(m).map(|p| (*m, p.qty)))
            .filter(|(_, qty)| !qty.is_zero())
            .collect();
        if !left.orders.is_empty() || (self.close_on_stop && !open.is_empty()) {
            let orders = left.orders.len();
            let positions: Vec<_> =
                open.iter().map(|(market, qty)| format!("a position of {qty} in market {market}")).collect();
            return Err(Failed::Incomplete(format!(
                "{orders} resting orders and {} are left",
                positions.join(" and ")
            )));
        }
        info!(%account, resting_orders = 0, "stopped");
        Ok(())
    }

    /// Signs and sends one close of `closes`, on the account's next nonce. A failed close is logged:
    /// the account read that follows shows what it did.
    async fn close(&self, account: AccountId, closes: Vec<CloseEntry>) -> Result<(), Failed> {
        info!(%account, markets = closes.len(), "closing the positions");
        let Some((nonce, now)) = self.nonce(account).await? else { return Ok(()) };
        let expires_at = now.saturating_add(EXPIRES_IN_MS);
        let mut request =
            CloseAllRequest { account, closes, max_slippage_bps: Some(self.max_slippage_bps), auth: None };
        let hash = close_all_hash(&self.domain, &request, nonce, expires_at);
        request.auth = Some(RequestAuth { nonce, expires_at, signature: self.key.sign(&hash) });
        match self.node.close_all(&request).await {
            Ok(answer) => log_order_events(&answer.body),
            Err(CallError::WrongSigner { .. }) => return Err(Failed::WrongSigner),
            Err(e) => warn!(%account, "the close failed: {e}"),
        }
        Ok(())
    }

    /// The account, verified.
    async fn read(&self, account: AccountId) -> Result<crate::node::api::AccountView, Failed> {
        match self.node.account(account).await {
            Ok(answer) => Ok(answer.body),
            Err(CallError::WrongSigner { .. }) => Err(Failed::WrongSigner),
            Err(e) => Err(Failed::Incomplete(e.to_string())),
        }
    }

    /// The account's next nonce and the node's time when it gave it, or `None`, with a warning,
    /// when the node gives no answer to rely on.
    async fn nonce(&self, account: AccountId) -> Result<Option<(u64, Millis)>, Failed> {
        match self.node.nonce(account).await {
            Ok(answer) => match Millis::try_from(answer.time_ms) {
                Ok(now) => Ok(Some((answer.body.next_nonce, now))),
                Err(_) => {
                    warn!(time_ms = answer.time_ms, "the node's time is out of range");
                    Ok(None)
                }
            },
            Err(CallError::WrongSigner { .. }) => Err(Failed::WrongSigner),
            Err(e) => {
                warn!(%account, "no nonce to sign with: {e}");
                Ok(None)
            }
        }
    }
}
