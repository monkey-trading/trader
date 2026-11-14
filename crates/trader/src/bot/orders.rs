//! The market orders the runner signs toward the strategies' targets, each on its account's own
//! nonce and with an expiry counted from the node's time, and what happens to an account when they
//! are answered.

use std::sync::Arc;
use std::time::Duration;

use engine_core::auth::RequestAuth;
use engine_core::command::{OrderKind, OrderRequest};
use engine_core::eip712::order_hash;
use engine_core::event::{Event, Reject};
use engine_core::types::{AccountId, Millis};
use futures_util::FutureExt;
use tokio::time::Instant;
use tracing::{debug, info, warn};

use crate::bot::Halt;
use crate::bot::fleet::{EXPIRES_IN_MS, Fleet, Nonce, REPLAY_LAG_MS, Wanted, Warm};
use crate::bot::run::{Done, Runner, signer_ok};
use crate::node::api::{MarkBucket, NonceView};
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
                    Done::History { series: request.series, generation: request.generation, outcome }
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
        self.admit_blocked = false;
        let mut queue = std::mem::take(&mut self.fleet.queue);
        queue.retain_mut(|&mut a| self.start_order(a, node_ms, now));
        self.fleet.queue = queue;
        if self.stale_dropped > 0 {
            warn!(
                alert = "orders_stale",
                dropped = self.stale_dropped,
                "orders were dropped because their bars were history by the time the node's limits let them go; \
                 raise node.limits to the node's, or trade fewer accounts"
            );
            self.stale_dropped = 0;
        }
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
        if account.failed.is_some() || !account.running || !asg.running {
            (asg.pending, asg.queued) = (None, false);
            return false;
        }
        if self.out >= self.cap || account.busy {
            return true;
        }
        if node_ms.saturating_sub(pending.wanted.ts) > REPLAY_LAG_MS {
            self.stale_dropped += 1;
            fleet.drop_pending(a, false);
            return false;
        }
        // The decision was taken from a read of the log, for an account that was trading. A stop or a close
        // since ends it; a fill or a change of cash or listing changes what it should be, and the target
        // is decided again once the account is read.
        let stopped = account.controls.is_stopped() || account.flatten_queued;
        if stopped || account.behind || account.epoch != pending.epoch {
            info!(account = %account.address, market = asg.market, stopped, behind = account.behind,
                again = !stopped, "the account changed since the order was decided: it is dropped, and its target decided again unless the account is stopped");
            fleet.drop_pending(a, !stopped);
            return false;
        }
        // The decision was taken against the state saved with it, and waits until that is kept.
        if account.state_dirty || account.state_saving || account.state_unsaved {
            return true;
        }
        let (intent, wanted) = (pending.intent, (pending.wanted, fleet.series[asg.series as usize].generation));
        let Some(quoted) = fleet.markets[asg.market_slot as usize].mark else {
            warn!(account = %account.address, market = asg.market, "no mark to quote the order at");
            fleet.drop_pending(a, false);
            return false;
        };
        let nonce = match account.nonce {
            Nonce::Synced(nonce) => nonce,
            Nonce::Unknown => {
                match account.retry_at.filter(|at| now < *at) {
                    None => {
                        account.busy = true;
                        self.out += 1;
                        let (node, address) = (Arc::clone(&self.node), account.address);
                        self.in_flight.push(
                            async move { Done::Nonce { account: index, outcome: node.nonce(address).await } }.boxed(),
                        );
                        // The orders behind this one wait for its answer, so they leave in bar order; a pass
                        // later, with the account busy, the ones behind it go on.
                        self.admit_blocked = true;
                    }
                    // An account in a back-off after a failed read holds nobody up.
                    Some(at) => self.wake_at = Some(self.wake_at.map_or(at, |w| w.min(at))),
                }
                return true;
            }
        };
        // The order is signed only once the node's limits let it go, and goes at once, so the node
        // time it is signed on, and the freshness checked above, are those of the moment it leaves.
        if self.admit_blocked {
            return true;
        }
        let admission = match self.node.try_admit(Some(account.address)) {
            Ok(admission) => admission,
            Err(wait) => {
                self.admit_blocked = true;
                self.wake_at = Some(self.wake_at.map_or(now + wait, |at| at.min(now + wait)));
                return true;
            }
        };
        // Bar starts are positive milliseconds since the epoch, one per bar.
        let client_id = u64::try_from(pending.wanted.bar_start).unwrap_or_default();
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
        if let Some(series) = fleet.metrics.markets.get(a as usize) {
            series.orders_placed.inc();
        }
        info!(account = %account.address, market = asg.market, side = ?intent.side, qty = %intent.qty, %quoted,
            reduce_only = intent.reduce_only, nonce, expires_at, client_id, "placing a market order");
        let node = Arc::clone(&self.node);
        self.in_flight.push(
            async move {
                Done::Order {
                    account: index,
                    assignment: a,
                    nonce,
                    expires_at,
                    wanted: Some(wanted),
                    outcome: node.place_order(&request, admission).await,
                }
            }
            .boxed(),
        );
        false
    }

    /// Reads the node's time again, through any account's nonce.
    pub(super) fn start_clock_read(&mut self) {
        if self.clock_reading {
            return;
        }
        let now = Instant::now();
        if let Some(retry_at) = self.clock_retry_at.filter(|at| now < *at) {
            self.wake_at = Some(self.wake_at.map_or(retry_at, |at| at.min(retry_at)));
            return;
        }
        let Some(account) = self.fleet.accounts.iter().find(|a| a.failed.is_none()) else { return };
        self.clock_reading = true;
        let (node, address) = (Arc::clone(&self.node), account.address);
        self.in_flight.push(async move { Done::Clock(node.nonce(address).await) }.boxed());
    }

    /// Takes an order's answer: the account's nonce moves on when the engine used it, an account
    /// whose key the node refuses stops, an order refused for a nonce another request of the account
    /// took has its target decided again, and the node's refusal counts as declined in each market the
    /// request named. The engine's rejections count as the log gives them (`Fleet::apply_event`, or
    /// `Fleet::count_unfollowed` as the stop reads past the last order, and the expiry of a rejected
    /// request), whether this answer came or not.
    /// Error cases:
    /// - an answer signed by another key than the signer
    pub(super) fn order_done(
        &mut self,
        account: u32,
        assignment: u32,
        nonce: u64,
        expires_at: Millis,
        wanted: Option<(Wanted, u32)>,
        outcome: Result<Verified<Vec<Event>>, CallError>,
    ) -> Result<(), Halt> {
        signer_ok(&outcome)?;
        self.out -= 1;
        self.fleet.accounts[account as usize].busy = false;
        let mut failure = None;
        if let Err(CallError::Refused { .. }) = &outcome {
            // The request's markets are those its expiry fences: an order's own, or each a close names.
            let range = self.fleet.accounts[account as usize].assignments.clone();
            let fenced = |b: &u32| self.fleet.assignments[*b as usize].expires_at == Some(expires_at);
            let mut named: Vec<_> =
                range.clone().filter(fenced).map(|b| self.fleet.assignments[b as usize].market).collect();
            if named.is_empty() {
                named.push(self.fleet.assignments[assignment as usize].market);
            }
            self.fleet.order_series(range).declined(&named);
        }
        let next = match outcome {
            Ok(answer) => {
                log_order_events(self.fleet.accounts[account as usize].address, &answer.body);
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
                // A request the engine answered with a rejection and no fill is in the log and moved nothing:
                // no fill is to come from it, for any market it named.
                if rejected && !answer.body.iter().any(|event| matches!(event, Event::Fill { .. })) {
                    unfence(&mut self.fleet, account, expires_at);
                }
                // The rejection is counted from the log, which a stop that began before it was read
                // reads on for.
                if rejected {
                    self.fleet.counts_until = self.fleet.counts_until.max(expires_at);
                }
                if rejected { Nonce::Unknown } else { Nonce::Synced(nonce + 1) }
            }
            Err(e @ CallError::Refused { status: 401 | 403, .. }) => {
                failure = Some(format!("the node refuses the account: {e}"));
                Nonce::Unknown
            }
            // The node checks a request's nonce before the log, so this one never entered it: another
            // request of the account, such as the console's withdrawal, took the nonce. The order's
            // target waits to be decided again on the account as it now is, with the node's next nonce,
            // once for its bar; a second refusal leaves it to the next bar. A target from before the
            // series warmed up again, as after a change of parameters, is not decided again.
            Err(e @ CallError::Refused { status: 409, .. }) => {
                unfence(&mut self.fleet, account, expires_at);
                let asg = &mut self.fleet.assignments[assignment as usize];
                let series = &self.fleet.series[asg.series as usize];
                let current = |generation| series.warm == Warm::Done && series.generation == generation;
                let again = wanted
                    .filter(|(w, generation)| {
                        current(*generation) && asg.wanted.is_none() && asg.nonce_retried != Some(w.bar_start)
                    })
                    .map(|(w, _)| w);
                if let Some(wanted) = again {
                    (asg.wanted, asg.nonce_retried) = (Some(wanted), Some(wanted.bar_start));
                }
                warn!(account = %self.fleet.accounts[account as usize].address, again = again.is_some(),
                    "order declined for its nonce, which another request of the account took: {e}");
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
        // A target of another market was waiting for this order.
        self.fleet.read_if_waiting(account);
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
            Err(e @ CallError::Refused { status: 401 | 403, .. }) => {
                self.fleet.fail(account, format!("the node refuses the account: {e}"));
            }
            Err(e) => {
                warn!(account = %entry.address, "no nonce to sign with: {e}");
                entry.retry_at = Some(Instant::now() + NONCE_RETRY);
            }
        }
        Ok(())
    }

    /// Takes a marks history read for a series' warm-up.
    /// Error cases:
    /// - an answer signed by another key than the signer
    pub(super) fn history_done(
        &mut self,
        series: u32,
        generation: u32,
        outcome: Result<Vec<MarkBucket>, CallError>,
    ) -> Result<(), Halt> {
        signer_ok(&outcome)?;
        let node_time =
            self.node.latest_time_ms().map_or(self.fleet.log_ts, |ms| Millis::try_from(ms).unwrap_or(Millis::MAX));
        match outcome {
            Ok(buckets) => self.fleet.warmed(series, generation, Some(&buckets), node_time),
            Err(e) => {
                warn!("the marks history was not read; trying again: {e}");
                self.fleet.warmed(series, generation, None, node_time);
            }
        }
        Ok(())
    }
}

/// Logs what an order request did: each refusal with the engine's reason, each fill, each accepted
/// or cancelled order.
pub(super) fn log_order_events(account: AccountId, events: &[Event]) {
    for event in events {
        match event {
            Event::OrderRejected { client_id, reason, .. } => warn!(%account, client_id, ?reason, "order declined"),
            Event::OrderAccepted { client_id, order_id, .. } => info!(%account, client_id, order_id, "order accepted"),
            Event::OrderCancelled { order_id, reason, .. } => info!(%account, order_id, ?reason, "order cancelled"),
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
        info!(%account, "the request changed nothing");
    }
}

/// Lifts the fence of every assignment of `account` that the request signed with `expires_at` set: the
/// request moved nothing and no fill is to come from it.
fn unfence(fleet: &mut Fleet, account: u32, expires_at: Millis) {
    for b in fleet.accounts[account as usize].assignments.clone() {
        let fence = &mut fleet.assignments[b as usize].expires_at;
        if *fence == Some(expires_at) {
            *fence = None;
        }
    }
}
