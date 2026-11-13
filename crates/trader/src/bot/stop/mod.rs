//! The stop: once the log has passed every order that could still enter it, a cancel of each
//! account's resting orders and, when configured, a close of its positions in its markets, each
//! signed on the account's next nonce once the node's limits let it go, then a verified read that
//! confirms what is left. Accounts stop side by side under the node's pacing; one that fails to
//! stop does not hold back the others, and a signer that is not the node's ends them all at once.

mod quotes;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use engine_core::Fixed;
use engine_core::auth::RequestAuth;
use engine_core::command::{CancelAllRequest, CloseAllRequest, CloseEntry};
use engine_core::eip712::{Eip712Domain, Signature, cancel_all_hash, close_all_hash};
use engine_core::types::{AccountId, MarketId, Millis};
use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use tokio::sync::{Mutex, mpsc};
use tokio::time::{Instant, timeout_at};
use tracing::{error, info, warn};

use crate::bot::Halt;
use crate::bot::fleet::EXPIRES_IN_MS;
use crate::bot::orders::log_order_events;
use crate::bot::run::Runner;
use crate::bot::stop::quotes::Quotes;
use crate::keys::AccountKey;
use crate::node::{Admission, CallError, Node};
use crate::stream::message::{CommandMessage, Head};
use crate::stream::{Stream, StreamError};

/// The longest the stop goes on closing a position that one close did not take whole.
const MAX_CLOSE_WAIT: Duration = Duration::from_secs(60);
/// The longest a request of the stop waits for the node's limits and a connection.
const ADMIT_WAIT: Duration = Duration::from_secs(60);

/// The longest the stop waits for the log to pass the last order's expiry.
const MAX_EXPIRY_WAIT: Duration = Duration::from_millis(EXPIRES_IN_MS as u64 + 5_000);

/// How long the markets' marks the stop read are used for the closes of other accounts: closes are
/// quoted at a mark the bot read no more than this long before.
const QUOTE_AGE: Duration = Duration::from_secs(2);
/// How many times a close reads the marks again after waiting too long to be admitted.
const QUOTE_ATTEMPTS: u32 = 3;
/// How many times a cancel or close the node refused for its nonce, which another request of the
/// account took (such as the console's withdrawal), is signed again on the next nonce.
const NONCE_ATTEMPTS: u32 = 3;

/// A close an account's flatten signed, reported before it is sent: the account's slot, the
/// signature, and the expiry until which the close may still enter the log.
pub(super) type SignedClose = (u32, Signature, Millis);

/// What one account's stop needs.
pub(super) struct Stop {
    node: Arc<Node>,
    domain: Eip712Domain,
    key: Arc<AccountKey>,
    close_on_stop: bool,
    /// Each market the account trades, the position the log last gave, and the market's slippage bound.
    markets: Vec<(MarketId, Fixed, i64)>,
    quotes: Arc<Quotes>,
    /// Where the closes are reported before they go, with the account's slot, when the stop runs
    /// beside the log's reading: their fills are then the account's own.
    report: Option<(u32, mpsc::Sender<SignedClose>)>,
    /// Set when the account stops trading: nothing more is signed for it then.
    halt: Option<Arc<AtomicBool>>,
}

/// Why an account's stop did not complete.
pub(super) enum Failed {
    WrongSigner,
    Incomplete(String),
}

impl Runner {
    /// Writes the state of every account whose file is out of date, before the run ends: a halt that
    /// the bot cannot go on from (a split, say) must not lose what it has decided.
    pub async fn flush_state(&mut self) {
        let dirty: Vec<u32> =
            (0u32..).zip(&self.fleet.accounts).filter(|(_, a)| a.state_dirty).map(|(i, _)| i).collect();
        let writes = dirty.into_iter().map(|index| {
            let (file, saved) = (self.fleet.accounts[index as usize].state_file.clone(), self.fleet.saved_for(index));
            async move { (index, file.save(saved).await) }
        });
        for (index, result) in futures_util::future::join_all(writes).await {
            if let Err(e) = result {
                warn!(account = %self.fleet.accounts[index as usize].address, "the saved state was not written: {e:#}");
            }
        }
    }

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
        // A flatten under way sends the closes it signs here: they are taken in as they come, so none
        // waits for room while this waits for it.
        while !self.in_flight.is_empty() {
            tokio::select! {
                Some(done) = self.in_flight.next() => self.handle(done)?,
                Some(close) = self.closes.1.recv() => self.take_close(close),
            }
        }
        self.record_closes();
        let settled = self.wait_for_log_past(stream).await?;
        let last = self.fleet.markets.iter().map(|m| (m.id, m.mark)).collect();
        let quotes = Arc::new(Quotes { last, read: Mutex::new(None) });
        let mut stops: FuturesUnordered<_> = self
            .fleet
            .accounts
            .iter()
            .filter(|account| account.failed.is_none())
            .map(|account| {
                let markets = account
                    .assignments
                    .clone()
                    .map(|a| &self.fleet.assignments[a as usize])
                    .map(|asg| (asg.market, asg.position, asg.max_slippage_bps))
                    .collect();
                Stop {
                    node: Arc::clone(&self.node),
                    domain: self.domain.clone(),
                    key: Arc::clone(&account.key),
                    close_on_stop: account.close_on_stop,
                    markets,
                    quotes: Arc::clone(&quotes),
                    report: None,
                    halt: None,
                }
                .run()
            })
            .collect();
        let mut incomplete = Vec::new();
        while let Some(result) = stops.next().await {
            match result {
                Ok(()) => {}
                // The rest are dropped before they send anything more.
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
        // An order's expiry is counted on a clock that can lead the node's by an answer's time.
        let deadline = Instant::now() + MAX_EXPIRY_WAIT + self.node.timeout();
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

impl Runner {
    /// The stop of account `a` that cancels its resting orders and closes its positions, run while
    /// the bot follows the log: each close is sent on `report` before it goes. It acts on the account
    /// as the log last gave it, at the markets' marks as the fleet has them.
    pub(super) fn stop_of(&self, a: u32, report: Option<mpsc::Sender<SignedClose>>) -> Stop {
        let account = &self.fleet.accounts[a as usize];
        let markets = account
            .assignments
            .clone()
            .map(|i| &self.fleet.assignments[i as usize])
            .map(|asg| (asg.market, asg.position, asg.max_slippage_bps))
            .collect();
        let last = self.fleet.markets.iter().map(|m| (m.id, m.mark)).collect();
        Stop {
            node: Arc::clone(&self.node),
            domain: self.domain.clone(),
            key: Arc::clone(&account.key),
            close_on_stop: true,
            markets,
            quotes: Arc::new(Quotes { last, read: Mutex::new(None) }),
            report: report.map(|report| (a, report)),
            halt: Some(Arc::clone(&account.halt)),
        }
    }
}

impl Stop {
    /// Stops the account: cancels its resting orders, closes its positions when configured, and
    /// confirms with a read.
    pub(super) async fn run(self) -> Result<(), Failed> {
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
                self.markets
                    .iter()
                    .map(|(m, _, slippage)| (*m, view.positions.get(m).map_or(Fixed::ZERO, |p| p.qty), *slippage))
                    .collect(),
            ),
            Err(Failed::WrongSigner) => return Err(Failed::WrongSigner),
            Err(Failed::Incomplete(e)) => {
                warn!(alert = "stop_unread", %account, "the account cannot be read, so the stop acts on what it last knew: {e}");
                (None, self.markets.clone())
            }
        };
        if resting != Some(0) {
            info!(%account, ?resting, "cancelling the account's resting orders");
            self.cancel(account).await?;
        }
        // One close takes at most `Fixed::MAX_INPUT` units of a position, so a confirmed close that
        // leaves part of one is followed by another on a fresh nonce; one that changed nothing is not
        // repeated.
        let deadline = Instant::now() + MAX_CLOSE_WAIT;
        let mut before = held;
        let left = loop {
            let closing = self.close_on_stop && before.iter().any(|(_, qty, _)| !qty.is_zero());
            if closing {
                self.close(account, &before).await?;
            }
            let left = match self.read(account).await {
                Ok(left) => left,
                Err(Failed::WrongSigner) => return Err(Failed::WrongSigner),
                Err(Failed::Incomplete(e)) => {
                    return Err(Failed::Incomplete(format!("the account cannot be read to confirm it: {e}")));
                }
            };
            let remaining: Vec<(MarketId, Fixed, i64)> = before
                .iter()
                .map(|(m, _, slippage)| (*m, left.positions.get(m).map_or(Fixed::ZERO, |p| p.qty), *slippage))
                .collect();
            let progressed =
                before.iter().zip(&remaining).any(|((_, was, _), (_, now, _))| !now.is_zero() && now.abs() < was.abs());
            // A resting order can fill before its cancel, opening a position the account did not hold when
            // it was read.
            let appeared =
                before.iter().zip(&remaining).any(|((_, was, _), (_, now, _))| was.is_zero() && !now.is_zero());
            if self.close_on_stop && (closing && progressed || appeared) && Instant::now() < deadline {
                before = remaining;
                continue;
            }
            break left;
        };
        // Every position counts, in a market the account trades or not: one a resting order opened in
        // another market before its cancel has no slippage bound to close at, and is left, said so.
        let open: Vec<_> = left.positions.iter().map(|(m, p)| (*m, p.qty)).filter(|(_, qty)| !qty.is_zero()).collect();
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

    /// Waits for the node's limits and a connection, for no longer than `ADMIT_WAIT`: a node that has
    /// become unreachable ends the account's stop as incomplete and does not hold the others' up.
    /// Error cases:
    /// - the limits and a connection were not there in time
    async fn admit(&self, account: AccountId) -> Result<Admission, Failed> {
        tokio::time::timeout(ADMIT_WAIT, self.node.admit(Some(account)))
            .await
            .map_err(|_| Failed::Incomplete(format!("no connection to the node within {ADMIT_WAIT:?}")))
    }

    /// Signs and sends a cancel of every resting order, on the account's next nonce, once the node's
    /// limits let it go, and again on the next nonce when the node refuses it for its nonce. A failed
    /// cancel is logged: the account read that follows shows what it did.
    async fn cancel(&self, account: AccountId) -> Result<(), Failed> {
        for attempt in 1..=NONCE_ATTEMPTS {
            self.check_halt()?;
            let Some(nonce) = self.nonce(account).await? else { return Ok(()) };
            let admission = self.admit(account).await?;
            let Some(expires_at) = self.expiry() else { return Ok(()) };
            let hash = cancel_all_hash(&self.domain, account, nonce, expires_at);
            let auth = Some(RequestAuth { nonce, expires_at, signature: self.key.sign(&hash) });
            match self.node.cancel_all(&CancelAllRequest { account, auth }, admission).await {
                Ok(answer) => log_order_events(account, &answer.body),
                Err(CallError::WrongSigner { .. }) => return Err(Failed::WrongSigner),
                Err(e @ CallError::Refused { status: 409, .. }) if attempt < NONCE_ATTEMPTS => {
                    info!(%account, "the cancel's nonce was taken by another request of the account; signing again: {e}");
                    continue;
                }
                Err(e) => warn!(%account, "the cancel failed: {e}"),
            }
            break;
        }
        Ok(())
    }

    /// Signs and sends the closes of the positions in `held`: one request for the markets that share
    /// a slippage bound, each market quoted at the node's mark as it was read no more than
    /// `QUOTE_AGE` before the request was admitted, which is when it is signed, and bounded as its
    /// strategy sets; again on the next nonce when the node refuses one for its nonce. A failed close
    /// is logged: the account read that follows shows what it did.
    async fn close(&self, account: AccountId, held: &[(MarketId, Fixed, i64)]) -> Result<(), Failed> {
        let mut by_slippage: BTreeMap<i64, Vec<(MarketId, Fixed)>> = BTreeMap::new();
        for (market, qty, slippage) in held.iter().filter(|(_, qty, _)| !qty.is_zero()) {
            by_slippage.entry(*slippage).or_default().push((*market, *qty));
        }
        for (slippage, markets) in by_slippage {
            for attempt in 1..=NONCE_ATTEMPTS {
                if !self.close_group(account, slippage, &markets, attempt == NONCE_ATTEMPTS).await? {
                    break;
                }
            }
        }
        Ok(())
    }

    /// Signs and sends one close of `markets`, which share the slippage bound `slippage`; whether the
    /// node refused it for its nonce, and it is to be signed again, which the `last` attempt is not.
    /// Error cases:
    /// - see `close`
    async fn close_group(
        &self,
        account: AccountId,
        slippage: i64,
        markets: &[(MarketId, Fixed)],
        last: bool,
    ) -> Result<bool, Failed> {
        self.check_halt()?;
        let Some(nonce) = self.nonce(account).await? else { return Ok(false) };
        // The marks are read, and then the limits let the request go; if that took long, again.
        let (marks, admission) = {
            let mut attempt = 0;
            loop {
                let (marks, read_at) = self.quotes.get(&self.node).await?;
                let admission = self.admit(account).await?;
                attempt += 1;
                if read_at.elapsed() < QUOTE_AGE {
                    break (marks, admission);
                }
                if attempt == QUOTE_ATTEMPTS {
                    return Err(Failed::Incomplete(format!(
                        "the marks were older than {QUOTE_AGE:?} when the close could be sent, {QUOTE_ATTEMPTS} times"
                    )));
                }
            }
        };
        let closes: Vec<CloseEntry> = markets
            .iter()
            .filter_map(|(market, qty)| {
                let quoted = marks.get(market).copied().flatten()?;
                info!(%account, market, position = %qty, %quoted, "closing the position");
                Some(CloseEntry { market: *market, quoted })
            })
            .collect();
        if closes.is_empty() {
            return Ok(false);
        }
        let Some(expires_at) = self.expiry() else { return Ok(false) };
        let mut request = CloseAllRequest { account, closes, max_slippage_bps: Some(slippage), auth: None };
        let hash = close_all_hash(&self.domain, &request, nonce, expires_at);
        let signature = self.key.sign(&hash);
        request.auth = Some(RequestAuth { nonce, expires_at, signature });
        if let Some((slot, report)) = &self.report {
            // The runner takes this in before it applies any message of the log, so the fill
            // the close makes is known for the account's own. A runner that ended has no use for it.
            let _ = report.send((*slot, signature, expires_at)).await;
        }
        match self.node.close_all(&request, admission).await {
            Ok(answer) => log_order_events(account, &answer.body),
            Err(CallError::WrongSigner { .. }) => return Err(Failed::WrongSigner),
            Err(e @ CallError::Refused { status: 409, .. }) if !last => {
                info!(%account, "the close's nonce was taken by another request of the account; signing again: {e}");
                return Ok(true);
            }
            Err(e) => warn!(%account, "the close failed: {e}"),
        }
        Ok(false)
    }

    /// Ends the stop of an account that has stopped trading since it began, before anything more is signed.
    /// Error cases:
    /// - the account halted
    fn check_halt(&self) -> Result<(), Failed> {
        match &self.halt {
            Some(halt) if halt.load(Ordering::Relaxed) => {
                Err(Failed::Incomplete("the account halted: nothing more was signed for it".into()))
            }
            _ => Ok(()),
        }
    }

    /// The expiry for a request signed now: the node's time as the bot last verified it, counted on,
    /// plus the window. `None`, with a warning, before any answer.
    fn expiry(&self) -> Option<Millis> {
        let now = self.node.latest_time_ms().and_then(|ms| Millis::try_from(ms).ok());
        if now.is_none() {
            warn!("the node's time is not known: nothing is signed");
        }
        now.map(|now| now.saturating_add(EXPIRES_IN_MS))
    }

    /// The account, verified.
    async fn read(&self, account: AccountId) -> Result<crate::node::api::AccountView, Failed> {
        match self.node.account(account).await {
            Ok(answer) => Ok(answer.body),
            Err(CallError::WrongSigner { .. }) => Err(Failed::WrongSigner),
            Err(e) => Err(Failed::Incomplete(e.to_string())),
        }
    }

    /// The account's next nonce, or `None`, with a warning, when the node gives no answer to rely on.
    async fn nonce(&self, account: AccountId) -> Result<Option<u64>, Failed> {
        match self.node.nonce(account).await {
            Ok(answer) => Ok(Some(answer.body.next_nonce)),
            Err(CallError::WrongSigner { .. }) => Err(Failed::WrongSigner),
            Err(e) => {
                warn!(%account, "no nonce to sign with: {e}");
                Ok(None)
            }
        }
    }
}
