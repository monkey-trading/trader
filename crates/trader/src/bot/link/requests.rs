//! What the runner does with the console's requests: start and stop an account or a strategy, set a
//! strategy's parameters within the config's bounds, and flatten an account. Each control is saved
//! before it is taken, but a stop holds even when it cannot be saved. A flatten stops the account,
//! waits until the log has passed every order it signed, then runs the account's stop with its
//! positions closed, as one request that holds the account; nothing starts the account meanwhile.

use futures_util::FutureExt;
use serde_json::Value;
use tracing::{Instrument, error, info, info_span, warn};

use crate::bot::Halt;
use crate::bot::fleet::Nonce;
use crate::bot::link::{AccountRequest, Control, Refusal, Reply, Saved, Waiting};
use crate::bot::run::{Done, Runner};
use crate::bot::stop::Failed;
use crate::config::bounds;

impl Runner {
    /// Acts on a request from the console and answers it; a flatten waits, unanswered, for its
    /// account's turn (`start_console_flattens`).
    pub(in crate::bot) async fn on_request(&mut self, request: AccountRequest) {
        let AccountRequest { account, id, control, reply } = request;
        let span = info_span!("console_control", id, %account, ?control);
        async {
            info!("console request");
            let Some(a) = self.fleet.index_of(account) else {
                return answer(reply, Err(Refusal::failed("no_account", format!("{account} is not traded here"))));
            };
            let flattening = {
                let end = self.console_end(a);
                end.flattening || !end.flattens.is_empty()
            };
            let result = match control {
                Control::Run { running: true } | Control::RunStrategy { running: true, .. } if flattening => {
                    Err(Refusal::invalid(
                        "flatten_in_progress",
                        "the account is flattening: start it once the flatten ends",
                    ))
                }
                Control::Run { running } => {
                    let mut next = self.saved(a).clone();
                    next.running = running;
                    self.set_controls(a, next, !running).await
                }
                Control::RunStrategy { strategy, running } => match self.strategy_controls(a, strategy) {
                    Ok(mut next) => {
                        next.strategies[strategy].running = running;
                        self.set_controls(a, next, !running).await
                    }
                    Err(refusal) => Err(refusal),
                },
                Control::SetParams { strategy, params } => self.set_params(a, strategy, params).await,
                Control::Flatten => {
                    let mut next = self.saved(a).clone();
                    next.running = false;
                    // The stop holds even when it was not saved, so the flatten goes on; its answer
                    // says the stop was not saved.
                    let unsaved = self.set_controls(a, next, true).await.err();
                    info!("flattening once the log has passed the account's last order");
                    let console = self.console.as_mut().expect("the runner has a console");
                    if !flattening {
                        console.waiting.push(a);
                    }
                    console.accounts[a as usize].flattens.push(Waiting { id, reply, unsaved });
                    self.fleet.mark_changed(a);
                    return;
                }
            };
            self.fleet.mark_changed(a);
            answer(reply, result);
        }
        .instrument(span)
        .await;
    }

    /// Starts the flatten of each account with flattens waiting, once the log has passed every
    /// order it signed and no other request of it is out.
    pub(in crate::bot) fn start_console_flattens(&mut self) {
        let Some(console) = &self.console else { return };
        if console.waiting.is_empty() {
            return;
        }
        for a in console.waiting.clone() {
            if self.fleet.accounts[a as usize].failed.is_some() {
                self.refuse_waiting_flattens(a);
                continue;
            }
            let account = &self.fleet.accounts[a as usize];
            let barrier = account
                .assignments
                .clone()
                .filter_map(|i| self.fleet.assignments[i as usize].expires_at)
                .fold(self.fleet.trading_from, i64::max);
            if account.busy || self.fleet.log_ts <= barrier {
                continue;
            }
            let ids: Vec<u64> = self.console_end(a).flattens.iter().map(|waiting| waiting.id).collect();
            let stop = self.stop_of(a, Some(self.closes.0.clone()));
            self.fleet.accounts[a as usize].busy = true;
            self.console_end(a).flattening = true;
            self.console.as_mut().expect("the runner has a console").waiting.retain(|&w| w != a);
            let span =
                info_span!("console_flatten", requests = ?ids, account = %self.fleet.accounts[a as usize].address);
            self.in_flight
                .push(async move { Done::Flatten { account: a, outcome: stop.run().await } }.instrument(span).boxed());
        }
    }

    /// Answers the flattens waiting for account `a`, which has halted since they were asked: the
    /// bot leaves a halted account as it is, so nothing is signed for it.
    fn refuse_waiting_flattens(&mut self, a: u32) {
        let console = self.console.as_mut().expect("the runner has a console");
        console.waiting.retain(|&w| w != a);
        for Waiting { reply, .. } in std::mem::take(&mut console.accounts[a as usize].flattens) {
            let message = "the account halted before the flatten began: nothing was done";
            answer(reply, Err(Refusal::invalid("account_halted", message)));
        }
        self.fleet.mark_changed(a);
    }

    /// Takes in the closes the flattens signed: each one's fills are the account's own, and until
    /// the log passes its expiry, no strategy of the account orders.
    pub(in crate::bot) fn record_closes(&mut self) {
        while let Ok((a, signature, expires_at)) = self.closes.1.try_recv() {
            let account = &mut self.fleet.accounts[a as usize];
            // As an order's signature is: the oldest goes when the list is full, never growing it.
            if account.signed.len() == account.signed.capacity() {
                account.signed.pop_front();
            }
            account.signed.push_back(signature);
            for i in account.assignments.clone() {
                let asg = &mut self.fleet.assignments[i as usize];
                asg.expires_at = asg.expires_at.max(Some(expires_at));
            }
        }
    }

    /// Applies a flatten's outcome: answers its requests, and leaves the account's nonce to be read
    /// again, since the flatten signed on it.
    /// Error cases:
    /// - an answer signed by another key than the signer
    pub(in crate::bot) fn flatten_done(&mut self, a: u32, outcome: Result<(), Failed>) -> Result<(), Halt> {
        let account = &mut self.fleet.accounts[a as usize];
        (account.busy, account.nonce) = (false, Nonce::Unknown);
        let (result, halt) = match outcome {
            Ok(()) => (Ok(()), None),
            Err(Failed::Incomplete(reason)) => (Err(Refusal::failed("flatten_incomplete", reason)), None),
            Err(Failed::WrongSigner) => {
                let refusal = Refusal::failed("wrong_signer", "the node's answers are not its signer's");
                (Err(refusal), Some(Halt::WrongSigner))
            }
        };
        let end = self.console_end(a);
        end.flattening = false;
        for Waiting { reply, unsaved, .. } in std::mem::take(&mut end.flattens) {
            let result = match (result.clone(), unsaved) {
                (result, None) => result,
                (Ok(()), Some(unsaved)) => {
                    Err(Refusal::failed("flattened_stop_unsaved", format!("the account is flattened; {unsaved}")))
                }
                (Err(refusal), Some(unsaved)) => Err(Refusal::failed(refusal.code(), format!("{refusal}; {unsaved}"))),
            };
            answer(reply, result);
        }
        self.fleet.mark_changed(a);
        halt.map_or(Ok(()), Err)
    }

    /// The controls saved for account `a`.
    fn saved(&mut self, a: u32) -> &Saved {
        &self.console_end(a).saved
    }

    /// Account `a`'s controls, which have strategy `strategy`.
    /// Error cases:
    /// - the account has no such strategy
    fn strategy_controls(&mut self, a: u32, strategy: usize) -> Result<Saved, Refusal> {
        let saved = self.saved(a);
        if strategy >= saved.strategies.len() {
            return Err(Refusal::NoStrategy(strategy));
        }
        Ok(saved.clone())
    }

    /// Saves `next` as account `a`'s controls, and takes them once saved. A stop (`stopping`) holds
    /// even when it cannot be saved, since a stop is the safe way to be wrong; anything else that
    /// cannot be saved changes nothing.
    /// Error cases:
    /// - the controls cannot be written
    async fn set_controls(&mut self, a: u32, next: Saved, stopping: bool) -> Result<(), Refusal> {
        let path = self.console_end(a).path.clone();
        let saved = next.save(path).await;
        if let Err(e) = &saved
            && !stopping
        {
            warn!("the console's controls were not saved, so nothing changed: {e:#}");
            let message = format!("nothing changed: the controls could not be saved: {e:#}");
            return Err(Refusal::failed("controls_unsaved", message));
        }
        let account = &mut self.fleet.accounts[a as usize];
        account.running = next.running;
        for (index, strategy) in account.assignments.clone().zip(&next.strategies) {
            self.fleet.assignments[index as usize].running = strategy.running;
        }
        self.console_end(a).saved = next;
        let Err(e) = saved else { return Ok(()) };
        error!(alert = "console_controls_unsaved", "the stop holds, but was not saved: {e:#}");
        let message = format!("the stop holds, but was not saved, so a restart would undo it: {e:#}");
        Err(Refusal::failed("stop_unsaved", message))
    }

    /// Sets strategy `strategy` of account `a` to the parameters `body` gives, within the config's
    /// bounds. Its series warms up again from the marks history, so the strategy decides on the new
    /// parameters from the first bar after it has, and places no order until then.
    /// Error cases:
    /// - the account has no such strategy
    /// - the parameters are not the strategy's kind and every parameter, lie outside the bounds or
    ///   change one the config sets none for, or are ones the strategy refuses (see `bounds::admit`)
    /// - the controls cannot be saved
    async fn set_params(&mut self, a: u32, strategy: usize, body: Value) -> Result<(), Refusal> {
        let mut next = self.strategy_controls(a, strategy)?;
        let index = self.fleet.accounts[a as usize].assignments.start + u32::try_from(strategy).unwrap_or(u32::MAX);
        let asg = &self.fleet.assignments[index as usize];
        let current = &self.fleet.series[asg.series as usize].strategies[asg.strategy as usize].params;
        let limits = asg.bounds.clone().unwrap_or_default();
        let params = bounds::admit(current, &body, &limits)
            .map_err(|refused| Refusal::invalid(refused.code, refused.message))?;
        if &params == current {
            return Ok(());
        }
        next.strategies[strategy].params = params.clone();
        self.set_controls(a, next, false).await?;
        info!(?params, "new parameters");
        self.fleet.set_params(index, params).map_err(|e| Refusal::invalid("params_invalid", e.to_string()))
    }
}

/// Answers a request; a console that stopped waiting has nothing to hear it.
fn answer(reply: Reply, result: Result<(), Refusal>) {
    if let Err(refusal) = &result {
        info!(%refusal, "console request refused");
    }
    let _ = reply.send(result);
}
