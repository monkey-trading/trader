//! The close of an account the daily loss limit has stopped: one request at a time, a slippage bound at a
//! time, signed once the node's limits let it go.

use std::sync::Arc;

use engine_core::Fixed;
use engine_core::auth::RequestAuth;
use engine_core::command::{CloseAllRequest, CloseEntry};
use engine_core::eip712::close_all_hash;
use engine_core::types::{MarketId, Millis};
use futures_util::FutureExt;
use tokio::time::Instant;
use tracing::info;

use crate::bot::fleet::{EXPIRES_IN_MS, FLATTEN_RETRY_MS, Nonce, REPLAY_LAG_MS};
use crate::bot::run::{Done, Runner};

impl Runner {
    /// Closes the positions of accounts the daily loss limit has stopped (or flattens), one close for
    /// each account at a time, signed once the limits let it go like an order, unless the market is
    /// shut or an order signed earlier may still enter the log and move a position.
    pub(super) fn start_flattens(&mut self) {
        if self.fleet.flattens.is_empty() || self.out >= self.cap {
            return;
        }
        let Some(node_ms) = self.node.time_ms(self.decided_at).map(|ms| Millis::try_from(ms).unwrap_or(Millis::MAX))
        else {
            self.start_clock_read();
            return;
        };
        let mut again = Vec::new();
        while let Some(index) = self.fleet.flattens.pop_front() {
            if self.out >= self.cap {
                again.push(index);
                continue;
            }
            if let Some(keep) = self.start_flatten(index, node_ms) {
                if keep {
                    again.push(index);
                } else {
                    self.fleet.accounts[index as usize].flatten_queued = false;
                }
            } else {
                self.fleet.accounts[index as usize].flatten_queued = false;
            }
        }
        self.fleet.flattens.extend(again);
    }

    /// Starts the close of `account`'s positions: `Some(true)` when it must wait, `Some(false)` when
    /// it is sent, and `None` when there is nothing to do.
    fn start_flatten(&mut self, index: u32, node_ms: Millis) -> Option<bool> {
        let fleet = &mut self.fleet;
        let account = &fleet.accounts[index as usize];
        if account.failed.is_some() {
            return None;
        }
        let held: Vec<u32> = account
            .assignments
            .clone()
            .filter(|&a| !fleet.assignments[a as usize].position.is_zero())
            .filter(|&a| {
                trader_core::hours::is_open(
                    &fleet.markets[fleet.assignments[a as usize].market_slot as usize].spec.schedule,
                    fleet.log_ts,
                )
            })
            .collect();
        // A market whose last order may still enter the log waits; the others are closed meanwhile.
        let held: Vec<u32> = held.into_iter().filter(|&a| fleet.order_in_flight(a, fleet.log_ts).is_none()).collect();
        if held.is_empty() {
            return None;
        }
        // The close is quoted at marks of the log, which must be current as an order's bar must be.
        if node_ms.saturating_sub(fleet.log_ts) > REPLAY_LAG_MS {
            return Some(true);
        }
        // Closes are asked for on an account at the log; one queued before it was not waits for a read.
        if account.busy || account.behind || account.state_dirty || account.state_saving || account.state_unsaved {
            return Some(true);
        }
        let Nonce::Synced(nonce) = account.nonce else {
            if account.retry_at.is_none_or(|at| Instant::now() >= at) {
                let account = &mut fleet.accounts[index as usize];
                account.busy = true;
                self.out += 1;
                let (node, address) = (Arc::clone(&self.node), account.address);
                self.in_flight
                    .push(async move { Done::Nonce { account: index, outcome: node.nonce(address).await } }.boxed());
            }
            return Some(true);
        };
        let admission = match self.node.try_admit(Some(account.address)) {
            Ok(admission) => admission,
            Err(wait) => {
                self.wake_at = Some(self.wake_at.map_or(Instant::now() + wait, |at| at.min(Instant::now() + wait)));
                return Some(true);
            }
        };
        // One request closes the markets that share a slippage bound, the groups in turn: no market closes
        // under another's looser bound. The others follow on the next try.
        let quoted_held: Vec<Held> = held
            .iter()
            .filter_map(|&a| {
                let asg = &fleet.assignments[a as usize];
                let quoted = fleet.markets[asg.market_slot as usize].mark?;
                Some((a, asg.market, quoted, asg.max_slippage_bps))
            })
            .collect();
        let (slippage, group) = next_group(quoted_held, account.flatten_bound)?;
        let held: Vec<u32> = group.iter().map(|&(a, ..)| a).collect();
        let closes: Vec<_> = group.iter().map(|&(_, market, quoted, _)| CloseEntry { market, quoted }).collect();
        let expires_at = node_ms.saturating_add(EXPIRES_IN_MS);
        let mut request =
            CloseAllRequest { account: account.address, closes, max_slippage_bps: Some(slippage), auth: None };
        let signature = account.key.sign(&close_all_hash(&self.domain, &request, nonce, expires_at));
        request.auth = Some(RequestAuth { nonce, expires_at, signature });
        info!(account = %account.address, markets = held.len(), nonce, expires_at, "closing the account's positions");
        // Signed, the close can enter the log until it expires, whatever its answer says.
        let first = held[0];
        for &a in &held {
            fleet.assignments[a as usize].expires_at = Some(expires_at);
        }
        let account = &mut fleet.accounts[index as usize];
        if account.signed.len() == account.signed.capacity() {
            account.signed.pop_front();
        }
        account.signed.push_back(signature);
        account.nonce = Nonce::Unknown;
        account.busy = true;
        account.flatten_after = fleet.log_ts.saturating_add(FLATTEN_RETRY_MS);
        account.flatten_bound = Some(slippage);
        self.out += 1;
        for &a in &held {
            if let Some(series) = fleet.metrics.markets.get(a as usize) {
                series.orders_placed.inc();
            }
        }
        let node = Arc::clone(&self.node);
        self.in_flight.push(
            async move {
                Done::Order {
                    account: index,
                    assignment: first,
                    nonce,
                    expires_at,
                    wanted: None,
                    outcome: node.close_all(&request, admission).await,
                }
            }
            .boxed(),
        );
        Some(false)
    }
}

/// A held position to close: its assignment, market, quote and slippage bound.
type Held = (u32, MarketId, Fixed, i64);

/// The markets of `held` that share one slippage bound, with that bound: the tightest above `after`, the
/// bound of the group tried last, or the tightest when none is above it. Each group is tried in turn,
/// so one the engine keeps refusing does not hold back the others.
fn next_group(held: Vec<Held>, after: Option<i64>) -> Option<(i64, Vec<Held>)> {
    let tightest =
        |above: Option<i64>| held.iter().map(|&(.., bound)| bound).filter(|&b| above.is_none_or(|a| b > a)).min();
    let bound = tightest(after).or_else(|| tightest(None))?;
    Some((bound, held.into_iter().filter(|&(.., b)| b == bound).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_close_goes_under_one_bound_at_a_time_in_turn() {
        let quote: Fixed = "100".parse().unwrap();
        let held = vec![(0, 1, quote, 100), (1, 2, quote, 10), (2, 3, quote, 10)];
        let markets = |after| {
            next_group(held.clone(), after).map(|(bound, g)| (bound, g.iter().map(|g| g.1).collect::<Vec<_>>()))
        };
        assert_eq!(markets(None), Some((10, vec![2, 3])), "the tightest first");
        assert_eq!(markets(Some(10)), Some((100, vec![1])), "then the next, though the first remains");
        assert_eq!(markets(Some(100)), Some((10, vec![2, 3])), "and round again");
        assert!(next_group(Vec::new(), None).is_none());
    }
}
