//! The jobs: one action for one account, from its request to its end, with the transactions it has
//! sent and what each has come to.

use std::collections::VecDeque;

use alloy_primitives::{B256, U256};
use engine_core::types::AccountId;
use serde::Serialize;
use serde_json::Value;

use crate::accounts::claim::{Claimed, is_wrong_signer};
use crate::accounts::cli::monkey;
use crate::accounts::declare::{Confirmed, Declared};
use crate::accounts::deposit::{Deposited, Source};
use crate::accounts::withdraw::StillQueued;
use crate::accounts::{Lifecycle, usdc_units};
use crate::chain::progress::Progress;
use crate::console::funding::{Action, CLAIM_THRESHOLD};
use crate::keys::AccountKey;

/// The most jobs kept: the oldest that has ended goes first.
const KEPT: usize = 200;

/// How a job stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Waiting for a place among the jobs that run.
    Waiting,
    Running,
    /// It did what was asked, or found it done.
    Landed,
    /// It did not.
    Failed,
}

impl State {
    fn ended(self) -> bool {
        matches!(self, Self::Landed | Self::Failed)
    }
}

/// How a transaction a job sent stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum TxState {
    Sent,
    Mined,
    Reverted,
}

/// A transaction a job sent.
#[derive(Clone, Serialize)]
struct Tx {
    hash: String,
    /// What it does.
    what: String,
    state: TxState,
}

/// How an action came to an end.
pub enum Outcome {
    /// It did what was asked, or found it done: what it came to.
    Landed(String),
    /// It did not: why, and whether the node answered with another key's signature.
    Failed { reason: String, signed_elsewhere: bool },
}

/// A job as the console reads it.
#[derive(Clone, Serialize)]
struct Job {
    id: u64,
    action: &'static str,
    account: String,
    state: State,
    /// What it came to, or why it failed.
    detail: String,
    /// The transactions it has sent, in order.
    txs: Vec<Tx>,
}

/// The jobs, oldest first.
#[derive(Default)]
pub struct Jobs {
    next: u64,
    jobs: VecDeque<Job>,
}

impl Jobs {
    /// A new job waiting for a place; its id.
    pub fn add(&mut self, action: &'static str, account: AccountId) -> u64 {
        self.next += 1;
        if self.jobs.len() >= KEPT
            && let Some(oldest) = self.jobs.iter().position(|j| j.state.ended())
        {
            self.jobs.remove(oldest);
        }
        self.jobs.push_back(Job {
            id: self.next,
            action,
            account: account.to_string(),
            state: State::Waiting,
            detail: String::new(),
            txs: Vec::new(),
        });
        self.next
    }

    fn get(&mut self, id: u64) -> Option<&mut Job> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }

    pub fn running(&mut self, id: u64) {
        if let Some(job) = self.get(id) {
            job.state = State::Running;
        }
    }

    /// Records a step of one of the job's transactions.
    pub fn progress(&mut self, id: u64, progress: Progress) {
        let Some(job) = self.get(id) else { return };
        let mut set = |tx: B256, state: TxState| {
            if let Some(found) = job.txs.iter_mut().find(|t| t.hash == tx.to_string()) {
                found.state = state;
            }
        };
        match progress {
            Progress::Sent { tx, what } => job.txs.push(Tx { hash: tx.to_string(), what, state: TxState::Sent }),
            Progress::Mined { tx } => set(tx, TxState::Mined),
            Progress::Reverted { tx } => set(tx, TxState::Reverted),
        }
    }

    pub fn end(&mut self, id: u64, outcome: Outcome) {
        let Some(job) = self.get(id) else { return };
        match outcome {
            Outcome::Landed(summary) => (job.state, job.detail) = (State::Landed, summary),
            Outcome::Failed { reason, .. } => (job.state, job.detail) = (State::Failed, reason),
        }
    }

    /// Whether job `id` has ended.
    pub fn is_over(&self, id: u64) -> bool {
        self.jobs.iter().find(|j| j.id == id).is_none_or(|j| j.state.ended())
    }

    /// Job `id` as the console reads it.
    pub fn view(&self, id: u64) -> Value {
        let job = self.jobs.iter().find(|j| j.id == id).expect("a job just added is kept");
        serde_json::to_value(job).expect("a job serializes")
    }

    /// Every job kept, oldest first.
    pub fn all(&self) -> Value {
        serde_json::to_value(&self.jobs).expect("jobs serialize")
    }
}

/// Does `action` for `key`'s account, and says how it ended. It returns once the action has landed.
pub async fn act(lifecycle: &Lifecycle, key: &AccountKey, action: Action) -> Outcome {
    match run(lifecycle, key, action).await {
        Ok(summary) => Outcome::Landed(summary),
        Err(e) => Outcome::Failed {
            signed_elsewhere: is_wrong_signer(&e),
            reason: match e.downcast_ref::<StillQueued>() {
                Some(StillQueued(seq)) => format!(
                    "the withdrawal is queued as seq {seq} but was not paid in time: a withdrawal with the amount \
                     empty pays what is queued, and asking again queues another ({e:#})"
                ),
                None => format!("{e:#}"),
            },
        },
    }
}

async fn run(lifecycle: &Lifecycle, key: &AccountKey, action: Action) -> anyhow::Result<String> {
    let account = key.address();
    Ok(match action {
        Action::Declare(class) => match lifecycle.declare(key, Confirmed { account, class }).await? {
            Declared::Already(class) => format!("already declared {class:?}"),
            Declared::Recorded(class) => format!("declared {class:?}"),
            Declared::Refused(reason) => anyhow::bail!("the node refused the declaration: {reason}"),
        },
        Action::Deposit { amount, from } => {
            usdc_units(amount)?;
            let source = match from {
                Source::Account => "the account's wallet",
                Source::Funding => "the funding wallet",
            };
            match lifecycle.deposit(key, amount, from).await? {
                Deposited::Credited { .. } => format!("{amount} USDC credited from {source}"),
                Deposited::AwaitingDeclaration { .. } => format!("{amount} USDC held until the account declares"),
            }
        }
        Action::Claim => match lifecycle.claim(key, U256::from(CLAIM_THRESHOLD)).await? {
            Claimed::Nothing | Claimed::Below { .. } => String::from("no MONKEY to claim"),
            Claimed::Waiting { unclaimed } => anyhow::bail!(
                "nothing was claimed: {} MONKEY becomes claimable once the vault records the newest checkpoint",
                monkey(unclaimed)
            ),
            Claimed::Minted { amount, .. } => format!("claimed {} MONKEY", monkey(amount)),
        },
        Action::Withdraw(amount) => {
            let paid = lifecycle.withdraw(key, amount).await?;
            let total = paid.iter().fold(engine_core::Fixed::ZERO, |sum, p| sum + p.amount);
            match paid.is_empty() {
                true => String::from("nothing to pay"),
                false => format!("{total} USDC paid to the account"),
            }
        }
    })
}
