//! The console's funding: what each account holds and owes on chain and in the node, and the
//! four actions of its life outside trading (`accounts`): declaring, depositing, claiming and
//! withdrawing. An action runs as a job per account in the background, since it returns only once
//! its transactions land; the console reads the jobs' states until they end. Every action acts for
//! one account's own key, so what it pays goes to that account's address, and the funding key,
//! when there is one, only pays gas and funds deposits. No key is returned.

mod jobs;
mod routes;
mod view;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use engine_core::Fixed;
use engine_core::access::EligibilityClass;
use engine_core::types::AccountId;
use futures_util::StreamExt;
use futures_util::stream;
use serde_json::{Value, json};
use tokio::sync::Semaphore;
use tokio::time::{Instant, timeout};
use tracing::warn;

use crate::accounts::Lifecycle;
use crate::accounts::deposit::Source;
use crate::console::funding::jobs::{Jobs, Outcome};
use crate::keys::AccountKey;

pub(super) use routes::routes;

/// The most accounts one request may name.
const MOST_ACCOUNTS: usize = 500;
/// The most jobs run at once: each holds a node connection and a chain transaction.
const JOBS_AT_ONCE: usize = 4;
/// The most statuses read at once, how long one read may take, and how long a read stays current.
const READS_AT_ONCE: usize = 8;
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const FRESH: Duration = Duration::from_secs(5);
/// A claim is sent for any unclaimed MONKEY above nothing, as `trader accounts claim` does.
const CLAIM_THRESHOLD: u64 = 1;

/// One of the four actions.
#[derive(Clone, Copy, Debug)]
pub enum Action {
    Declare(EligibilityClass),
    Deposit { amount: Fixed, from: Source },
    Claim,
    Withdraw(Option<Fixed>),
}

/// Why a request was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// No account is named, or too many.
    Accounts(String),
    /// The address is none of the bot's accounts.
    Unknown(AccountId),
    /// The account has a job that has not ended.
    Busy(AccountId),
    /// The action needs a funding key the config does not name.
    NoFundingKey,
}

/// The funding of the bot's accounts.
pub struct Funding {
    lifecycle: Arc<Lifecycle>,
    /// Every account with an unlocked key, in the config's order.
    keys: Vec<Arc<AccountKey>>,
    by_address: BTreeMap<AccountId, usize>,
    running: Semaphore,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    jobs: Jobs,
    busy: BTreeSet<AccountId>,
    /// An account's last status read, as the console shows it.
    reads: BTreeMap<AccountId, (Instant, Value)>,
}

impl Funding {
    /// The funding of `keys`' accounts, in order, through `lifecycle`. A key listed twice counts once.
    pub fn new(lifecycle: Arc<Lifecycle>, keys: Vec<Arc<AccountKey>>) -> Arc<Self> {
        let mut by_address = BTreeMap::new();
        let mut unique = Vec::with_capacity(keys.len());
        for key in keys {
            if by_address.insert(key.address(), unique.len()).is_none() {
                unique.push(key);
            }
        }
        Arc::new(Self {
            lifecycle,
            keys: unique,
            by_address,
            running: Semaphore::new(JOBS_AT_ONCE),
            state: Mutex::default(),
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Starts `action` for each of `accounts`, one job each, and returns the jobs as they stand. A
    /// request that is refused starts none.
    /// Error cases:
    /// - see `Refused`
    pub fn submit(self: &Arc<Self>, action: Action, accounts: &[AccountId]) -> Result<Vec<Value>, Refused> {
        if accounts.is_empty() || accounts.len() > MOST_ACCOUNTS {
            return Err(Refused::Accounts(format!("name between one and {MOST_ACCOUNTS} accounts")));
        }
        if matches!(action, Action::Deposit { from: Source::Funding, .. }) && !self.lifecycle.has_funding() {
            return Err(Refused::NoFundingKey);
        }
        let accounts: Vec<AccountId> = accounts.iter().copied().collect::<BTreeSet<_>>().into_iter().collect();
        let mut state = self.state();
        for account in &accounts {
            if !self.by_address.contains_key(account) {
                return Err(Refused::Unknown(*account));
            }
            if state.busy.contains(account) {
                return Err(Refused::Busy(*account));
            }
        }
        let mut started = Vec::with_capacity(accounts.len());
        for account in accounts {
            state.busy.insert(account);
            let id = state.jobs.add(action_name(action), account);
            started.push(state.jobs.view(id));
            let funding = Arc::clone(self);
            tokio::spawn(async move { funding.run(id, action, account).await });
        }
        Ok(started)
    }

    /// Runs one job: waits for a place, acts, and records how it ended.
    async fn run(self: Arc<Self>, id: u64, action: Action, account: AccountId) {
        let _place = self.running.acquire().await.expect("the semaphore is never closed");
        self.state().jobs.running(id);
        let key = &self.keys[self.by_address[&account]];
        let outcome = jobs::act(&self.lifecycle, key, action).await;
        if let Outcome::Failed(reason) = &outcome {
            warn!(%account, "a funding action failed: {reason}");
        }
        let mut state = self.state();
        state.jobs.end(id, outcome);
        state.busy.remove(&account);
        // What it changed is not what the last read showed.
        state.reads.remove(&account);
    }

    /// Every account's status, jobs included, as the console shows them: reads that are current are
    /// reused, the rest read at most `READS_AT_ONCE` at a time, an account that cannot be read
    /// carrying its error.
    pub async fn overview(&self) -> Value {
        let now = Instant::now();
        let stale: Vec<AccountId> = {
            let state = self.state();
            self.keys
                .iter()
                .map(|key| key.address())
                .filter(|a| state.reads.get(a).is_none_or(|(at, _)| now.duration_since(*at) >= FRESH))
                .collect()
        };
        let read: Vec<(AccountId, Value)> = stream::iter(stale)
            .map(|account| async move {
                let status = match timeout(READ_TIMEOUT, self.lifecycle.status(account)).await {
                    Ok(Ok(status)) => view::status(&status),
                    Ok(Err(e)) => view::unread(account, &format!("{e:#}")),
                    Err(_) => view::unread(account, "the read took too long"),
                };
                (account, status)
            })
            .buffer_unordered(READS_AT_ONCE)
            .collect()
            .await;
        let mut state = self.state();
        let at = Instant::now();
        state.reads.extend(read.into_iter().map(|(account, status)| (account, (at, status))));
        let accounts: Vec<Value> = self
            .keys
            .iter()
            .filter_map(|key| {
                let account = key.address();
                let mut status = state.reads.get(&account)?.1.clone();
                status["busy"] = json!(state.busy.contains(&account));
                Some(status)
            })
            .collect();
        json!({
            "enabled": true,
            "funding_key": self.lifecycle.has_funding(),
            "statements": {
                "professional": EligibilityClass::Professional.statement(),
                "eligible_client": EligibilityClass::EligibleClient.statement(),
            },
            "accounts": accounts,
            "jobs": state.jobs.all(),
        })
    }
}

/// The action's name in a job.
fn action_name(action: Action) -> &'static str {
    match action {
        Action::Declare(_) => "declare",
        Action::Deposit { .. } => "deposit",
        Action::Claim => "claim",
        Action::Withdraw(_) => "withdraw",
    }
}
