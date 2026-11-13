//! The console's funding: what each account holds and owes on chain and in the node, and the
//! four actions of its life outside trading (`accounts`): declaring, depositing, claiming and
//! withdrawing. An action runs as a job per account in the background, since it returns only once
//! its transactions land; the console reads the jobs, and the transactions each has sent, until they
//! end. Every action acts for one account's own key, so what it pays goes to that account's address,
//! and the funding key, when there is one, only pays gas and funds deposits. No key is returned.
//! The accounts' figures are read in the background while the console is watching, at most a quarter
//! of the node's limit on the bot so its orders keep the share the console's other reads leave them,
//! and the read of them answers from the last reads at once.

mod jobs;
mod routes;
mod view;

use std::collections::{BTreeMap, BTreeSet};
use std::convert::Infallible;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use engine_core::Fixed;
use engine_core::access::EligibilityClass;
use engine_core::types::AccountId;
use futures_util::StreamExt;
use futures_util::stream;
use serde_json::{Value, json};
use tokio::sync::{Semaphore, watch};
use tokio::task::AbortHandle;
use tokio::time::{Instant, sleep, sleep_until, timeout};
use tracing::{Instrument, debug, info, info_span, warn};

use crate::accounts::Lifecycle;
use crate::accounts::claim::is_wrong_signer;
use crate::accounts::deposit::Source;
use crate::chain::progress;
use crate::config::RateSection;
use crate::console::funding::jobs::{Jobs, Outcome};
use crate::keys::AccountKey;
use crate::node::CallError;

pub(super) use routes::routes;

/// The most accounts one request may name, and the most bytes a request naming them takes: an address
/// is 44 bytes of JSON, and a declaration's statement is short.
const MOST_ACCOUNTS: usize = 500;
pub(super) const MAX_BODY: usize = 32 * 1024;
/// The most jobs run at once: each holds a node connection and may hold a chain transaction.
const JOBS_AT_ONCE: usize = 4;
/// The most statuses read at once, how long one read may take, how old a read may be before it is made
/// again, and how long after the console last asked the reads go on.
const READS_AT_ONCE: usize = 2;
/// The most accounts one pass reads before it looks again for the stalest, so an account a job just
/// changed waits for a few reads, not for every account's.
const PASS: usize = 16;
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const REFRESH: Duration = Duration::from_secs(30);
const WATCH: Duration = Duration::from_secs(60);
/// How long the reading rests between its passes.
const REST: Duration = Duration::from_secs(1);
/// The node reads one status takes, about (its payouts and queue may take more pages), and the share
/// of the node's limit on the bot the reads keep to: a quarter, beside the refresher's half.
const STATUS_READS: u32 = 5;
const READ_SHARE: u32 = 4;
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
    /// The bot is stopping or halting, so no job starts.
    Stopping,
}

/// The funding of the bot's accounts.
pub struct Funding {
    lifecycle: Arc<Lifecycle>,
    /// Every account with an unlocked key, in the config's order.
    keys: Vec<Arc<AccountKey>>,
    by_address: BTreeMap<AccountId, usize>,
    running: Semaphore,
    state: Mutex<State>,
    /// The target of an answer another key signed, once one has been read.
    signed_elsewhere: watch::Sender<Option<String>>,
}

#[derive(Default)]
struct State {
    jobs: Jobs,
    busy: BTreeSet<AccountId>,
    /// An account's last status read, as the console shows it, and when it was made.
    reads: BTreeMap<AccountId, (Instant, Value)>,
    /// When the console last asked for the funding.
    watched: Option<Instant>,
    /// How many jobs of each account have ended: a read made across one is not kept.
    epochs: BTreeMap<AccountId, u64>,
    /// The jobs' tasks that may not have ended, and whether the bot is stopping, which ends them.
    running: Vec<AbortHandle>,
    stopped: bool,
}

impl Funding {
    /// The funding of `keys`' accounts through `lifecycle`, each with its place in the config; a key
    /// listed twice counts once. Their figures are read by `run_reads`.
    pub fn new(lifecycle: Arc<Lifecycle>, mut keys: Vec<(usize, Arc<AccountKey>)>) -> Arc<Self> {
        keys.sort_by_key(|(order, _)| *order);
        let mut by_address = BTreeMap::new();
        let mut unique = Vec::with_capacity(keys.len());
        for (_, key) in keys {
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
            signed_elsewhere: watch::channel(None).0,
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
        if state.stopped {
            return Err(Refused::Stopping);
        }
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
            let span = info_span!("funding_job", id, action = action_name(action), %account);
            let job = tokio::spawn(async move { funding.run_job(id, action, account).await }.instrument(span));
            state.running.retain(|job| !job.is_finished());
            state.running.push(job.abort_handle());
        }
        Ok(started)
    }

    /// Ends every job that has not ended, at once, and starts no more: the bot is stopping or halting,
    /// so nothing more is sent for a job. What a job already sent may still land.
    pub fn stop_jobs(&self) {
        // Taken out first: an ended job's guard takes the state as it goes.
        let running = {
            let mut state = self.state();
            state.stopped = true;
            std::mem::take(&mut state.running)
        };
        for job in running {
            job.abort();
        }
    }

    /// Runs one job: waits for a place, acts, and records how it ended. A job that stops before it
    /// ends, by a panic or the runtime's end, is recorded as failed, frees its account and is logged.
    async fn run_job(self: Arc<Self>, id: u64, action: Action, account: AccountId) {
        let mut finish = Finish { funding: &self, id, account, ended: false };
        let _place = self.running.acquire().await.expect("the semaphore is never closed");
        self.state().jobs.running(id);
        info!("started");
        let key = &self.keys[self.by_address[&account]];
        let sink: progress::Sink = {
            let funding = Arc::clone(&self);
            Arc::new(move |step| funding.state().jobs.progress(id, step))
        };
        let outcome = progress::with_sink(sink, jobs::act(&self.lifecycle, key, action)).await;
        match &outcome {
            Outcome::Landed(summary) => info!("landed: {summary}"),
            Outcome::Failed { reason, signed_elsewhere } => {
                warn!(alert = "funding_failed", "failed: {reason}");
                if *signed_elsewhere {
                    self.flag_signer(account);
                }
            }
        }
        finish.ended = true;
        self.end(id, account, outcome);
    }

    /// Ends job `id`, frees its account and drops what was read of it, which the job changed.
    fn end(&self, id: u64, account: AccountId, outcome: Outcome) {
        let mut state = self.state();
        if !state.jobs.is_over(id) {
            state.jobs.end(id, outcome);
        }
        state.busy.remove(&account);
        state.reads.remove(&account);
        *state.epochs.entry(account).or_default() += 1;
    }

    /// Records that the node answered `account`'s request with another key's signature.
    fn flag_signer(&self, account: AccountId) {
        self.signed_elsewhere.send_replace(Some(format!("funding of {account}")));
    }

    /// Reads the accounts' figures while the console watches them, at most a `READ_SHARE` of
    /// `per_ip`, the node's limit on the bot, until a funding read or action meets an answer another
    /// key signed than the node's.
    /// Error cases:
    /// - a funding read or action met an answer another key signed: the error that stops the bot
    pub async fn run_reads(self: Arc<Self>, per_ip: RateSection) -> anyhow::Result<()> {
        let gap = STATUS_READS * READ_SHARE * Duration::from_secs(1) / per_ip.per_second.max(1);
        tokio::select! {
            error = self.signed_elsewhere() => Err(error),
            never = self.read_while_watched(gap) => match never {},
        }
    }

    /// Waits until a funding read or action meets an answer another key signed than the node's, and
    /// says so as the error that stops the bot.
    async fn signed_elsewhere(&self) -> anyhow::Error {
        let mut seen = self.signed_elsewhere.subscribe();
        let target = match seen.wait_for(Option::is_some).await.map(|target| target.clone()) {
            Ok(target) => target.unwrap_or_default(),
            Err(_) => std::future::pending().await,
        };
        CallError::WrongSigner { target }.into()
    }

    /// Every account's status as last read, and the jobs, as the console shows them. It answers at
    /// once, an account not read yet saying so, and asks for the reads to go on.
    pub fn overview(&self) -> Value {
        let mut state = self.state();
        state.watched = Some(Instant::now());
        let accounts: Vec<Value> = self
            .keys
            .iter()
            .map(|key| {
                let account = key.address();
                let mut status = match state.reads.get(&account) {
                    Some((_, status)) => status.clone(),
                    None => view::reading(account),
                };
                status["busy"] = json!(state.busy.contains(&account));
                status
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

    /// Reads the accounts' status for as long as the console watches, the reads starting `gap` apart,
    /// at most `READS_AT_ONCE` at a time, each kept as it is read.
    async fn read_while_watched(&self, gap: Duration) -> Infallible {
        let mut next = Instant::now();
        loop {
            let mut stale = self.stale();
            stale.truncate(PASS);
            let starts: Vec<(Instant, AccountId)> = stale
                .into_iter()
                .map(|account| {
                    let at = next.max(Instant::now());
                    next = at + gap;
                    (at, account)
                })
                .collect();
            stream::iter(starts)
                .for_each_concurrent(READS_AT_ONCE, |(at, account)| async move {
                    sleep_until(at).await;
                    self.read(account).await;
                })
                .await;
            sleep(REST).await;
        }
    }

    /// Reads the status of `account`, and keeps it unless a job of the account ended meanwhile, which
    /// changed what it read.
    async fn read(&self, account: AccountId) {
        let epoch = self.state().epochs.get(&account).copied();
        let status = match timeout(READ_TIMEOUT, self.lifecycle.status(account)).await {
            Ok(Ok(status)) => view::status(&status),
            Ok(Err(e)) => {
                if is_wrong_signer(&e) {
                    self.flag_signer(account);
                }
                debug!(%account, "the account's funding was not read: {e:#}");
                view::unread(account, &format!("{e:#}"))
            }
            Err(_) => {
                debug!(%account, "the account's funding was not read within {READ_TIMEOUT:?}");
                view::unread(account, "the read took too long")
            }
        };
        let mut state = self.state();
        if state.epochs.get(&account).copied() == epoch {
            state.reads.insert(account, (Instant::now(), status));
        }
    }

    /// The accounts whose status the console wants read now, those never read or changed by a job
    /// first: none unless it asked lately.
    fn stale(&self) -> Vec<AccountId> {
        let state = self.state();
        let now = Instant::now();
        if !state.watched.is_some_and(|at| now.duration_since(at) < WATCH) {
            return Vec::new();
        }
        let mut stale: Vec<(Option<Instant>, AccountId)> = self
            .keys
            .iter()
            .map(|key| (state.reads.get(&key.address()).map(|(at, _)| *at), key.address()))
            .filter(|(at, _)| at.is_none_or(|at| now.duration_since(at) >= REFRESH))
            .collect();
        // Unread first, then oldest first; the sort is stable, so the config's order breaks ties.
        stale.sort_by_key(|(at, _)| *at);
        stale.into_iter().map(|(_, account)| account).collect()
    }
}

/// Ends a job that stopped before it ended.
struct Finish<'a> {
    funding: &'a Funding,
    id: u64,
    account: AccountId,
    ended: bool,
}

impl Drop for Finish<'_> {
    fn drop(&mut self) {
        if !self.ended {
            // The chain and the node hold what it did; the next status read shows it.
            warn!(alert = "funding_job_unfinished", id = self.id, account = %self.account,
                "the job stopped before it ended, by a panic or the bot's end: what it sent may still land");
            let reason = String::from("the job stopped before it ended");
            self.funding.end(self.id, self.account, Outcome::Failed { reason, signed_elsewhere: false });
        }
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
