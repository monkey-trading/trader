//! The accounts' side of the console: the status each publishes, the requests the console sends
//! (start, stop, new parameters, flatten), and the controls those requests set, saved per account so
//! a restart keeps them. Only the runner, which owns every account, changes their controls; the
//! console asks it to (`requests`). An account that could not start has a status and no controls.

mod requests;
mod saved;
pub use saved::{Saved, SavedStrategy, path};

use std::path::PathBuf;
use std::time::Duration;

use engine_core::Fixed;
use engine_core::types::{AccountId, Millis};
use serde::Serialize;
use serde_json::{Map, Value};
use tokio::sync::{mpsc, oneshot, watch};
use trader_core::strategies::StrategyParams;
use trader_core::strategy::Target;

use crate::bot::fleet::{Fleet, Warm};
use crate::bot::run::Runner;
use crate::config::{ParamBounds, bounds};

/// The most requests waiting for the runner; the console refuses more until it takes them.
pub const REQUESTS: usize = 64;
const MS_PER_DAY: Millis = 86_400_000;

/// A strategy's kind, as the config's `kind` names it.
pub fn kind_of(params: &StrategyParams) -> String {
    match bounds::to_json(params).remove("kind") {
        Some(Value::String(kind)) => kind,
        _ => unreachable!("a strategy's parameters carry their kind"),
    }
}

/// Where an account stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountState {
    /// Following the log; its running strategies place orders.
    Running,
    /// Following the log, placing no order.
    Stopped,
    /// Stopped, and closing its positions once the log has passed its last order.
    Flattening,
    /// Stopped trading on its own, for the reason its status gives.
    Halted,
}

/// What the console shows of an account's own state.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AccountStatus {
    pub state: AccountState,
    /// Why the account halted.
    pub halt: Option<String>,
    /// What kind of reason it is, for a halt, as a refusal's code names it.
    pub code: Option<&'static str>,
    /// What names an account whose address is not known: where its key comes from.
    pub label: Option<String>,
    /// The daily loss limit's stop, while it holds.
    pub day_stop: Option<DayStop>,
    pub strategies: Vec<StrategyStatus>,
}

/// The daily loss limit stopping an account until the next UTC day.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct DayStop {
    /// When the stop ends, in the log's milliseconds: the start of the next UTC day after the log's
    /// time, which is the day after the stop's own for one carried over midnight while a position is
    /// held.
    pub until_ms: Millis,
    /// What the day has lost, in USD: the day's opening equity less the account's equity.
    pub loss: Fixed,
}

/// An account that could not start, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartFailure {
    /// The account's place in the config's accounts.
    pub order: usize,
    /// Where its key comes from.
    pub label: String,
    /// Its address, once its key was unlocked.
    pub address: Option<AccountId>,
    /// One of `key_locked`, `account_refused`, `account_unread`, `no_balance`, `position_elsewhere`,
    /// `controls_unreadable` and `market_unsupported`.
    pub code: &'static str,
    pub reason: String,
}

/// One strategy of an account.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StrategyStatus {
    pub id: usize,
    pub kind: String,
    pub market: String,
    #[serde(with = "humantime_serde")]
    pub bar: Duration,
    pub running: bool,
    /// The strategy's `kind` and each parameter, as the config writes them: whole numbers as
    /// numbers, decimals as strings.
    pub params: Map<String, Value>,
    /// Each parameter the console may set, with its lowest and highest value; `None` when it may set
    /// none.
    pub bounds: Option<ParamBounds>,
    /// Whether the strategy has warmed up from the marks history and decides on each bar.
    pub warmed_up: bool,
    /// The position the strategy last wanted: `long`, `short` or `flat`.
    pub target: Option<&'static str>,
}

/// A request from the console, answered once the account has acted on it.
#[derive(Debug)]
pub struct AccountRequest {
    pub account: AccountId,
    /// The id of the console request that sent it, which the account's log lines for it carry.
    pub id: u64,
    pub control: Control,
    pub reply: Reply,
}

/// What the console asks of an account.
#[derive(Clone, Debug, PartialEq)]
pub enum Control {
    /// Lets the account's strategies place orders, or stops them.
    Run { running: bool },
    /// Lets one strategy place orders, or stops it.
    RunStrategy { strategy: usize, running: bool },
    /// Sets a strategy's parameters, `{kind, every parameter}`, within the config's bounds.
    SetParams { strategy: usize, params: Value },
    /// Stops the account, then cancels its resting orders and closes every position.
    Flatten,
}

/// Where the answer to a request goes.
pub type Reply = oneshot::Sender<Result<(), Refusal>>;

/// Why a request did not do what it asked, with a code a client can match on.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("the account has no strategy {0}")]
    NoStrategy(usize),
    /// The request asks for what the config does not allow, or what the account cannot do now.
    #[error("{message}")]
    Invalid { code: &'static str, message: String },
    /// The account could not do it, or not all of it.
    #[error("{message}")]
    Failed { code: &'static str, message: String },
}

impl Refusal {
    /// A refusal of what the config does not allow, or the account cannot do now.
    pub fn invalid(code: &'static str, message: impl Into<String>) -> Self {
        Self::Invalid { code, message: message.into() }
    }

    /// A refusal of what the account could not do.
    pub fn failed(code: &'static str, message: impl Into<String>) -> Self {
        Self::Failed { code, message: message.into() }
    }

    /// The refusal's code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoStrategy(_) => "no_strategy",
            Self::Invalid { code, .. } | Self::Failed { code, .. } => code,
        }
    }
}

/// The console's end of an account: its status, and where requests for it go.
#[derive(Clone, Debug)]
pub struct AccountLink {
    /// The account's place in the config's accounts.
    pub order: usize,
    /// `None` for an account whose key could not be unlocked.
    pub address: Option<AccountId>,
    /// Whether the account trades, so the node's figures for it are read.
    pub figures: bool,
    pub status: watch::Receiver<AccountStatus>,
    pub requests: mpsc::Sender<AccountRequest>,
}

/// An account's controls as startup found them, and what its status names each strategy by: its
/// market's symbol and its bar length, in the account's order.
pub struct Kept {
    /// The account's place in the config's accounts.
    pub order: usize,
    /// Where the controls are saved; `None` without a console.
    pub path: Option<PathBuf>,
    pub saved: Saved,
    pub strategies: Vec<(String, Duration)>,
}

/// The runner's end of the console: the requests, and each account's controls and status, in the
/// fleet's account order.
pub struct ConsoleEnd {
    requests: Option<mpsc::Receiver<AccountRequest>>,
    accounts: Vec<AccountEnd>,
    /// The accounts with flattens waiting to start.
    waiting: Vec<u32>,
}

/// One account's saved controls, its status, and the flattens waiting for it.
struct AccountEnd {
    path: PathBuf,
    saved: Saved,
    strategies: Vec<(String, Duration)>,
    status: watch::Sender<AccountStatus>,
    /// Flatten requests waiting for the log to pass the account's last order, or for the flatten
    /// under way.
    flattens: Vec<Waiting>,
    flattening: bool,
}

/// A flatten request waiting for its answer.
struct Waiting {
    /// The console request's id.
    id: u64,
    reply: Reply,
    /// Why the stop it set was not saved, when it was not.
    unsaved: Option<Refusal>,
}

impl ConsoleEnd {
    /// The runner's end and the console's for the accounts `fleet` trades, with what startup `kept`
    /// for each, in the fleet's order.
    pub fn new(fleet: &Fleet, kept: Vec<Kept>) -> (Self, Vec<AccountLink>) {
        assert_eq!(kept.len(), fleet.accounts.len(), "startup keeps the controls of every account it starts");
        let (sender, requests) = mpsc::channel(REQUESTS);
        let mut links = Vec::with_capacity(kept.len());
        let accounts = fleet
            .accounts
            .iter()
            .zip(kept)
            .map(|(account, kept)| {
                let (status, receiver) = watch::channel(AccountStatus::blank());
                let order = kept.order;
                links.push(AccountLink {
                    order,
                    address: Some(account.address),
                    figures: true,
                    status: receiver,
                    requests: sender.clone(),
                });
                AccountEnd {
                    path: kept.path.expect("a console keeps every account's controls"),
                    saved: kept.saved,
                    strategies: kept.strategies,
                    status,
                    flattens: Vec::new(),
                    flattening: false,
                }
            })
            .collect();
        let end = Self { requests: Some(requests), accounts, waiting: Vec::new() };
        for (a, end_account) in end.accounts.iter().enumerate() {
            end_account.status.send_replace(status_of(fleet, end_account, a));
        }
        (end, links)
    }

    /// The links of the accounts that could not start: each shows why it halted, and takes no
    /// request.
    pub fn halted(failures: &[StartFailure]) -> Vec<AccountLink> {
        failures
            .iter()
            .map(|failure| {
                let status = AccountStatus {
                    state: AccountState::Halted,
                    halt: Some(failure.reason.clone()),
                    code: Some(failure.code),
                    label: failure.address.is_none().then(|| failure.label.clone()),
                    ..AccountStatus::blank()
                };
                // Its end is dropped: no runner takes a request for an account it does not trade.
                let (requests, _) = mpsc::channel(1);
                AccountLink {
                    order: failure.order,
                    address: failure.address,
                    figures: false,
                    status: watch::channel(status).1,
                    requests,
                }
            })
            .collect()
    }

    /// The requests to take; `None` once taken.
    pub(super) fn take_requests(&mut self) -> Option<mpsc::Receiver<AccountRequest>> {
        self.requests.take()
    }
}

impl AccountStatus {
    /// A running account with no strategy shown yet.
    fn blank() -> Self {
        Self { state: AccountState::Running, halt: None, code: None, label: None, day_stop: None, strategies: vec![] }
    }
}

impl Runner {
    /// Account `a`'s console end.
    fn console_end(&mut self, a: u32) -> &mut AccountEnd {
        // A request or a flatten for an account reaches the runner only through its console.
        &mut self.console.as_mut().expect("the runner has a console").accounts[a as usize]
    }

    /// Publishes the status of every account whose status changed since the console last heard.
    pub(super) fn publish_changes(&mut self) {
        // Taken out for the loop and put back, so its capacity stays.
        let mut changed = std::mem::take(&mut self.fleet.changed);
        for a in changed.drain(..) {
            self.fleet.accounts[a as usize].changed = false;
            let Some(console) = &self.console else { continue };
            let end = &console.accounts[a as usize];
            let now = status_of(&self.fleet, end, a as usize);
            end.status.send_if_modified(|status| {
                let changed = *status != now;
                *status = now;
                changed
            });
        }
        self.fleet.changed = changed;
    }
}

/// `a`'s status, from the fleet and its console end.
fn status_of(fleet: &Fleet, end: &AccountEnd, a: usize) -> AccountStatus {
    let account = &fleet.accounts[a];
    let state = match (&account.failed, end.flattening || !end.flattens.is_empty(), account.running) {
        (Some(_), _, _) => AccountState::Halted,
        (None, true, _) => AccountState::Flattening,
        (None, false, true) => AccountState::Running,
        (None, false, false) => AccountState::Stopped,
    };
    let strategies = account
        .assignments
        .clone()
        .zip(&end.strategies)
        .enumerate()
        .map(|(id, (index, (market, bar)))| {
            let asg = &fleet.assignments[index as usize];
            let series = &fleet.series[asg.series as usize];
            let shared = &series.strategies[asg.strategy as usize];
            StrategyStatus {
                id,
                kind: kind_of(&shared.params),
                market: market.clone(),
                bar: *bar,
                running: asg.running,
                params: bounds::to_json(&shared.params),
                bounds: asg.bounds.clone(),
                warmed_up: series.warm == Warm::Done,
                target: shared.target.map(|target| match target {
                    Target::Long => "long",
                    Target::Short => "short",
                    Target::Flat => "flat",
                }),
            }
        })
        .collect();
    let day_stop = account.controls.day_state().filter(|day| day.tripped).map(|day| DayStop {
        // A stop carried into a later day, while a position is held, ends at the end of that day.
        until_ms: day.day.max(fleet.log_ts.div_euclid(MS_PER_DAY)).saturating_add(1).saturating_mul(MS_PER_DAY),
        loss: day.start_equity - fleet.equity_of(a as u32).unwrap_or(account.balance),
    });
    let code = account.failed.as_ref().map(|_| "account_stopped");
    AccountStatus { state, halt: account.failed.clone(), code, label: None, day_stop, strategies }
}

#[cfg(test)]
mod tests;
