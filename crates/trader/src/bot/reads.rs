//! The reads of accounts and the writes of their state: which are started, in what order their
//! answers are taken, and what an answer changes.

use std::sync::Arc;
use std::time::Duration;

use futures_util::FutureExt;
use tokio::time::Instant;
use tracing::{info, warn};

use crate::bot::Halt;
use crate::bot::day::Saved;
use crate::bot::run::{Done, Runner, ViewOutcome, signer_ok};

/// How long after a failed write of an account's state it is tried again.
const SAVE_RETRY: Duration = Duration::from_secs(5);

impl Runner {
    /// Reads the accounts a read is wanted for, in the order it was wanted, a few at a time: a decision
    /// waits for one that is of the log, and a fill moves the balance and the entry price.
    pub(super) fn start_reads(&mut self) {
        while self.view_reads < self.cap {
            let Some(index) = self.fleet.reads.pop_front() else { return };
            let account = &mut self.fleet.accounts[index as usize];
            account.read_wanted = false;
            if account.failed.is_some() || account.reading {
                continue;
            }
            (account.reading, account.read_stale) = (true, false);
            self.view_reads += 1;
            self.view_order.push_back((index, None));
            let (node, address) = (Arc::clone(&self.node), account.address);
            self.in_flight
                .push(async move { Done::View { account: index, outcome: node.account(address).await } }.boxed());
        }
    }

    /// Writes the state of accounts whose file is out of date, one write for each account at a time.
    pub(super) fn start_saves(&mut self) {
        let now = Instant::now();
        let mut later = Vec::new();
        while let Some(index) = self.fleet.saves.pop() {
            let account = &mut self.fleet.accounts[index as usize];
            if account.state_saving {
                // Written again when the write that is out is done.
                continue;
            }
            if let Some(at) = account.save_retry_at.filter(|at| now < *at) {
                self.wake_at = Some(self.wake_at.map_or(at, |w| w.min(at)));
                later.push(index);
                continue;
            }
            account.state_dirty = false;
            account.state_saving = true;
            let (file, saved) = (account.state_file.clone(), self.fleet.saved_for(index));
            self.in_flight.push(
                async move {
                    let outcome = file.save(saved.clone()).await;
                    Done::Saved { account: index, saved, outcome }
                }
                .boxed(),
            );
        }
        self.fleet.saves.extend(later);
    }

    /// Takes the answer to a write of an account's state: when it is kept, the account may order again.
    pub(super) fn saved_done(&mut self, account: u32, saved: Saved, outcome: anyhow::Result<()>) -> Result<(), Halt> {
        let entry = &mut self.fleet.accounts[account as usize];
        entry.state_saving = false;
        match outcome {
            Ok(()) => {
                entry.saved = saved;
                entry.save_retry_at = None;
                // Whether more changed while it was written is worked out again against what is kept.
                entry.state_dirty = false;
                if entry.state_unsaved {
                    entry.state_unsaved = false;
                    info!(alert_resolved = "day_state_unsaved", account = %entry.address, "the saved state is written");
                    let range = entry.assignments.clone();
                    if self.fleet.assignments[range.start as usize..range.end as usize]
                        .iter()
                        .any(|a| a.wanted.is_some())
                    {
                        self.fleet.want_read(account);
                    }
                }
                self.fleet.touch_state(account);
            }
            Err(e) => {
                if !entry.state_unsaved {
                    entry.state_unsaved = true;
                    warn!(alert = "day_state_unsaved", account = %entry.address,
                        "{e:#}: a restart would not know the day's loss or the holds");
                }
                entry.save_retry_at = Some(Instant::now() + SAVE_RETRY);
                entry.state_dirty = true;
                self.fleet.saves.push(account);
            }
        }
        Ok(())
    }

    /// Takes a read of an account: when it is of the log its balance and entries are the account's,
    /// and the decisions waiting for it are taken.
    /// Error cases:
    /// - an answer signed by another key than the signer
    pub(super) fn view_done(&mut self, account: u32, outcome: ViewOutcome) -> Result<(), Halt> {
        signer_ok(&outcome)?;
        self.view_reads -= 1;
        self.fleet.accounts[account as usize].reading = false;
        // The reads are taken in the order they were started, which is the order the bars wanted them,
        // so the decisions queue in bar order whichever answer comes first.
        if let Some(slot) = self.view_order.iter_mut().find(|(a, answer)| *a == account && answer.is_none()) {
            slot.1 = Some(outcome);
        }
        while self.view_order.front().is_some_and(|(_, answer)| answer.is_some()) {
            if let Some((account, Some(outcome))) = self.view_order.pop_front() {
                self.take_view(account, outcome);
            }
        }
        Ok(())
    }

    /// Takes one read of an account, in its turn.
    fn take_view(&mut self, account: u32, outcome: ViewOutcome) {
        let entry = &mut self.fleet.accounts[account as usize];
        match outcome {
            Ok(answer) => {
                // A decision, a fill or a cash event that came after the read was started is not in it:
                // its balance is not published, and the account is read again.
                if std::mem::take(&mut entry.read_stale) {
                    self.fleet.want_read(account);
                    return;
                }
                let at_log = self.fleet.apply_view(account, &answer.body);
                let entry = &mut self.fleet.accounts[account as usize];
                if entry.failed.is_some() {
                    return;
                }
                info!(account = %entry.address, balance = %entry.balance, head_seq = answer.head_seq, at_log, "account read");
                if at_log {
                    self.fleet.decide_account(account);
                    self.fleet.observe_account(account, self.fleet.log_ts);
                }
            }
            Err(e) => {
                warn!(account = %entry.address, "the account was not read: {e}");
                self.fleet.read_failed(account);
            }
        }
    }
}
