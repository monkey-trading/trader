//! The market orders the runner signs toward the strategies' targets, each on its account's own
//! nonce and with an expiry counted from the node's time, and what happens to an account when they
//! are answered.

use std::sync::Arc;
use std::time::Duration;

use engine_core::auth::RequestAuth;
use engine_core::command::{OrderKind, OrderRequest};
use engine_core::eip712::order_hash;
use engine_core::event::{Event, Reject};
use engine_core::types::Millis;
use futures_util::FutureExt;
use tokio::time::Instant;
use tracing::{debug, info, warn};
use trader_core::strategy::order_for;

use crate::bot::Halt;
use crate::bot::fleet::{EXPIRES_IN_MS, Nonce, REPLAY_LAG_MS};
use crate::bot::run::{Done, Runner, signer_ok};
use crate::node::api::{AccountView, MarkBucket, NonceView};
use crate::node::{CallError, Verified};

/// How long after a failed nonce read it is tried again.
const NONCE_RETRY: Duration = Duration::from_secs(1);

impl Runner {
    /// Asks for the marks history each series wants.
    pub(super) fn start_warm_ups(&mut self) {
        for request in std::mem::take(&mut self.fleet.warm_requests) {
            let node = Arc::clone(&self.node);
            self.in_flight.push(
                async move {
                    let outcome = node.marks(request.market, request.resolution, request.from, request.to).await;
                    Done::History { series: request.series, outcome }
                }
                .boxed(),
            );
        }
    }

    /// Sends the queued decisions in bar order, as far as the requests out and the accounts' state
    /// allow: an account with a request out waits, a stale decision is dropped, an account whose
    /// nonce is unknown reads it first, and the rest are signed and sent.
    pub(super) fn start_orders(&mut self) {
        if self.fleet.queue.is_empty() || self.out >= self.cap {
            return;
        }
        let now = Instant::now();
        // The node's time must be as late as the newest decision: an answer to a request sent since.
        let Some(node_ms) = self.node.time_ms(self.decided_at) else {
            self.start_clock_read();
            return;
        };
        let node_ms = Millis::try_from(node_ms).unwrap_or(Millis::MAX);
        let mut queue = std::mem::take(&mut self.fleet.queue);
        queue.retain_mut(|&mut a| self.start_order(a, node_ms, now));
        self.fleet.queue = queue;
        debug!(queued = self.fleet.queue.len(), out = self.out, "orders started");
    }

    /// Starts the decision of assignment `a`, or drops it; whether it stays queued.
    fn start_order(&mut self, a: u32, node_ms: Millis, now: Instant) -> bool {
        let fleet = &mut self.fleet;
        let asg = &mut fleet.assignments[a as usize];
        let index = asg.account;
        let account = &mut fleet.accounts[index as usize];
        let Some(pending) = asg.pending else {
            asg.queued = false;
            return false;
        };
        if account.failed.is_some() {
            (asg.pending, asg.queued) = (None, false);
            return false;
        }
        if self.out >= self.cap || account.busy {
            return true;
        }
        let drop_with = |asg: &mut crate::bot::fleet::Assignment| {
            (asg.pending, asg.queued) = (None, false);
            false
        };
        if node_ms.saturating_sub(pending.ts) > REPLAY_LAG_MS {
            info!(account = %account.address, market = asg.market, "the bar is history by the node's time: no order");
            return drop_with(asg);
        }
        let Some(intent) = order_for(pending.target, asg.qty, asg.position, asg.close_first) else {
            return drop_with(asg);
        };
        let Some(quoted) = fleet.markets[asg.market_slot as usize].mark else {
            warn!(account = %account.address, market = asg.market, "no mark to quote the order at");
            return drop_with(asg);
        };
        let nonce = match account.nonce {
            Nonce::Synced(nonce) => nonce,
            Nonce::Unknown => {
                if account.retry_at.is_none_or(|at| now >= at) {
                    account.busy = true;
                    self.out += 1;
                    let (node, address) = (Arc::clone(&self.node), account.address);
                    self.in_flight.push(
                        async move { Done::Nonce { account: index, outcome: node.nonce(address).await } }.boxed(),
                    );
                } else {
                    self.wake_at = Some(
                        self.wake_at
                            .map_or(account.retry_at.unwrap_or(now), |w| w.min(account.retry_at.unwrap_or(now))),
                    );
                }
                return true;
            }
        };
        // Bar starts are positive milliseconds since the epoch, one per bar.
        let client_id = u64::try_from(pending.bar_start).unwrap_or_default();
        let expires_at = node_ms.saturating_add(EXPIRES_IN_MS);
        let mut request = OrderRequest {
            account: account.address,
            client_id,
            market: asg.market,
            side: intent.side,
            qty: intent.qty,
            kind: OrderKind::Market { quoted, max_slippage_bps: Some(asg.max_slippage_bps) },
            reduce_only: intent.reduce_only,
            auth: None,
        };
        let signature = account.key.sign(&order_hash(&self.domain, &request, nonce, expires_at));
        request.auth = Some(RequestAuth { nonce, expires_at, signature });
        // Signed, the order can enter the log until it expires, whatever its answer says.
        asg.expires_at = Some(expires_at);
        if account.signed.len() == account.signed.capacity() {
            account.signed.pop_front();
        }
        account.signed.push_back(signature);
        account.nonce = Nonce::Unknown;
        account.busy = true;
        asg.close_first = false;
        (asg.pending, asg.queued) = (None, false);
        self.out += 1;
        info!(account = %account.address, market = asg.market, side = ?intent.side, qty = %intent.qty, %quoted,
            reduce_only = intent.reduce_only, nonce, expires_at, client_id, "placing a market order");
        let node = Arc::clone(&self.node);
        self.in_flight.push(
            async move { Done::Order { account: index, assignment: a, nonce, outcome: node.place_order(&request).await } }
                .boxed(),
        );
        false
    }

    /// Reads the node's time again, through any account's nonce.
    fn start_clock_read(&mut self) {
        if self.clock_reading {
            return;
        }
        let Some(account) = self.fleet.accounts.iter().find(|a| a.failed.is_none()) else { return };
        self.clock_reading = true;
        let (node, address) = (Arc::clone(&self.node), account.address);
        self.in_flight.push(async move { Done::Clock(node.nonce(address).await) }.boxed());
    }

    /// Reads the balance of accounts that have filled since their last read, a few at a time.
    pub(super) fn start_balance_reads(&mut self) {
        while self.balance_reads < 2 {
            let Some(index) = self.fleet.stale_balances.pop() else { return };
            let account = &mut self.fleet.accounts[index as usize];
            account.balance_stale = false;
            account.balance_reading = true;
            self.balance_reads += 1;
            let (node, address) = (Arc::clone(&self.node), account.address);
            self.in_flight
                .push(async move { Done::Balance { account: index, outcome: node.account(address).await } }.boxed());
        }
    }

    /// Takes an order's answer: the account's nonce moves on when the engine used it, and an
    /// account whose key the node refuses stops.
    /// Error cases:
    /// - an answer signed by another key than the signer
    pub(super) fn order_done(
        &mut self,
        account: u32,
        assignment: u32,
        nonce: u64,
        outcome: Result<Verified<Vec<Event>>, CallError>,
    ) -> Result<(), Halt> {
        signer_ok(&outcome)?;
        self.out -= 1;
        self.fleet.accounts[account as usize].busy = false;
        let mut failure = None;
        let next = match outcome {
            Ok(answer) => {
                log_order_events(&answer.body);
                let rejected = answer.body.iter().any(|event| matches!(event, Event::OrderRejected { .. }));
                let refused_key = answer.body.iter().any(|event| {
                    matches!(
                        event,
                        Event::OrderRejected { reason: Reject::InvalidSignature | Reject::MissingSignature, .. }
                    )
                });
                if refused_key {
                    failure = Some("the node refuses the account's signature".to_owned());
                }
                // The pure close passes where a reversal's blended spread does not.
                let close_first = answer.body.iter().any(|event| {
                    matches!(event, Event::OrderRejected { reason: Reject::CloseSpreadCeiling { .. }, .. })
                });
                self.fleet.assignments[assignment as usize].close_first = close_first;
                if rejected { Nonce::Unknown } else { Nonce::Synced(nonce + 1) }
            }
            Err(e @ CallError::Refused { status: 401 | 403, .. }) => {
                failure = Some(format!("the node refuses the account: {e}"));
                Nonce::Unknown
            }
            Err(e @ CallError::Refused { .. }) => {
                warn!(account = %self.fleet.accounts[account as usize].address, "order declined: {e}");
                Nonce::Unknown
            }
            Err(e) => {
                warn!(account = %self.fleet.accounts[account as usize].address,
                    "no answer to the order, which may still enter the log until it expires: {e}");
                Nonce::Unknown
            }
        };
        self.fleet.accounts[account as usize].nonce = next;
        if let Some(reason) = failure {
            self.fleet.fail(account, reason);
        }
        Ok(())
    }

    /// Takes a nonce read: the account's nonce is the node's next, or is read again soon.
    /// Error cases:
    /// - an answer signed by another key than the signer
    pub(super) fn nonce_done(
        &mut self,
        account: u32,
        outcome: Result<Verified<NonceView>, CallError>,
    ) -> Result<(), Halt> {
        signer_ok(&outcome)?;
        self.out -= 1;
        let entry = &mut self.fleet.accounts[account as usize];
        entry.busy = false;
        match outcome {
            Ok(answer) => {
                entry.nonce = Nonce::Synced(answer.body.next_nonce);
                entry.retry_at = None;
            }
            Err(e) => {
                warn!(account = %entry.address, "no nonce to sign with: {e}");
                entry.retry_at = Some(Instant::now() + NONCE_RETRY);
            }
        }
        Ok(())
    }

    /// Takes a balance read: the node's figure is the one that holds fills, financing, fees and
    /// transfers alike.
    /// Error cases:
    /// - an answer signed by another key than the signer
    pub(super) fn balance_done(
        &mut self,
        account: u32,
        outcome: Result<Verified<AccountView>, CallError>,
    ) -> Result<(), Halt> {
        signer_ok(&outcome)?;
        self.balance_reads -= 1;
        let entry = &mut self.fleet.accounts[account as usize];
        entry.balance_reading = false;
        match outcome {
            Ok(answer) => {
                entry.balance = answer.body.balance;
                info!(account = %entry.address, balance = %entry.balance, head_seq = answer.head_seq, "balance");
            }
            Err(e) => warn!(account = %entry.address, "the balance was not read: {e}"),
        }
        Ok(())
    }

    /// Takes a marks history read for a series' warm-up.
    /// Error cases:
    /// - an answer signed by another key than the signer
    pub(super) fn history_done(
        &mut self,
        series: u32,
        outcome: Result<Vec<MarkBucket>, CallError>,
    ) -> Result<(), Halt> {
        signer_ok(&outcome)?;
        let node_time =
            self.node.latest_time_ms().map_or(self.fleet.log_ts, |ms| Millis::try_from(ms).unwrap_or(Millis::MAX));
        match outcome {
            Ok(buckets) => self.fleet.warmed(series, Some(&buckets), node_time),
            Err(e) => {
                warn!("the marks history was not read; trying again: {e}");
                self.fleet.warmed(series, None, node_time);
            }
        }
        Ok(())
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
            Event::Fill(fill) => info!(
                account = %fill.account,
                order_id = fill.order_id,
                market = fill.market,
                side = ?fill.side,
                qty = %fill.qty,
                price = %fill.price,
                "order filled"
            ),
            _ => {}
        }
    }
    if events.is_empty() {
        info!("the request changed nothing");
    }
}
