//! The jobs: one action for one account, from its request to its end.

use std::collections::VecDeque;

use alloy_primitives::U256;
use engine_core::types::AccountId;
use serde::Serialize;
use serde_json::Value;

use crate::accounts::claim::Claimed;
use crate::accounts::cli::monkey;
use crate::accounts::declare::{Confirmed, Declared};
use crate::accounts::deposit::{Deposited, Source};
use crate::accounts::{Lifecycle, usdc_units};
use crate::console::funding::{Action, CLAIM_THRESHOLD};
use crate::keys::AccountKey;

/// The most jobs kept: the oldest that has ended goes first.
const KEPT: usize = 200;

/// How a job ended.
pub enum Outcome {
    /// It did what was asked, or found it done: what it came to, and its transactions.
    Landed { summary: String, txs: Vec<String> },
    /// It did not: why.
    Failed(String),
}

/// A job as the console reads it.
#[derive(Clone, Serialize)]
struct Job {
    id: u64,
    action: &'static str,
    account: String,
    /// `waiting` for a place, `running`, `landed` or `failed`.
    state: &'static str,
    /// What it came to, or why it failed.
    detail: String,
    /// Its transactions' hashes.
    txs: Vec<String>,
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
        let ended = self.jobs.iter().filter(|j| j.state == "landed" || j.state == "failed").count();
        if self.jobs.len() >= KEPT
            && ended > 0
            && let Some(oldest) = self.jobs.iter().position(|j| j.state == "landed" || j.state == "failed")
        {
            self.jobs.remove(oldest);
        }
        self.jobs.push_back(Job {
            id: self.next,
            action,
            account: account.to_string(),
            state: "waiting",
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
            job.state = "running";
        }
    }

    pub fn end(&mut self, id: u64, outcome: Outcome) {
        let Some(job) = self.get(id) else { return };
        match outcome {
            Outcome::Landed { summary, txs } => (job.state, job.detail, job.txs) = ("landed", summary, txs),
            Outcome::Failed(reason) => (job.state, job.detail) = ("failed", reason),
        }
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
        Ok((summary, txs)) => Outcome::Landed { summary, txs },
        Err(e) => Outcome::Failed(format!("{e:#}")),
    }
}

async fn run(lifecycle: &Lifecycle, key: &AccountKey, action: Action) -> anyhow::Result<(String, Vec<String>)> {
    let account = key.address();
    Ok(match action {
        Action::Declare(class) => match lifecycle.declare(key, Confirmed { account, class }).await? {
            Declared::Already(class) => (format!("already declared {class:?}"), vec![]),
            Declared::Recorded(class) => (format!("declared {class:?}"), vec![]),
            Declared::Refused(reason) => anyhow::bail!("the node refused the declaration: {reason}"),
        },
        Action::Deposit { amount, from } => {
            usdc_units(amount)?;
            let source = match from {
                Source::Account => "the account's wallet",
                Source::Funding => "the funding wallet",
            };
            match lifecycle.deposit(key, amount, from).await? {
                Deposited::Credited { tx } => (format!("{amount} USDC credited from {source}"), vec![tx.to_string()]),
                Deposited::AwaitingDeclaration { tx } => {
                    (format!("{amount} USDC held until the account declares"), vec![tx.to_string()])
                }
            }
        }
        Action::Claim => match lifecycle.claim(key, U256::from(CLAIM_THRESHOLD)).await? {
            Claimed::Nothing | Claimed::Below { .. } => (String::from("no MONKEY to claim"), vec![]),
            Claimed::Waiting { unclaimed } => {
                (format!("{} MONKEY claimable once the vault records the newest checkpoint", monkey(unclaimed)), vec![])
            }
            Claimed::Minted { amount, tx } => (format!("claimed {} MONKEY", monkey(amount)), vec![tx.to_string()]),
        },
        Action::Withdraw(amount) => {
            let paid = lifecycle.withdraw(key, amount).await?;
            let total = paid.iter().fold(engine_core::Fixed::ZERO, |sum, p| sum + p.amount);
            let summary = match paid.is_empty() {
                true => String::from("nothing to pay"),
                false => format!("{total} USDC paid to the account"),
            };
            (summary, paid.iter().map(|p| p.tx.to_string()).collect())
        }
    })
}
