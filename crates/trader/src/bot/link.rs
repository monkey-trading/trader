//! The account's side of the console: the status it publishes, the requests the console sends it
//! (start, stop, new periods, flatten), and the controls those requests set, saved so a restart
//! keeps them. Only the account's task changes its controls; the console asks it to.

mod saved;
pub use saved::{Saved, SavedStrategy};

use std::path::PathBuf;
use std::time::Duration;

use engine_core::command::CloseEntry;
use engine_core::types::{AccountId, Millis};
use serde::Serialize;
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{Instrument, error, info, info_span, warn};
use trader_core::strategy::{SmaCrossover, Target};

use crate::bot::{Bot, Halt};
use crate::config::{Config, SmaBounds, SmaParams};

/// The most requests waiting for an account; the console refuses more until it takes them.
pub const REQUESTS: usize = 8;
/// The kind of strategy the bot runs.
const SMA_CROSSOVER: &str = "sma_crossover";

/// Where an account stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountState {
    /// Reading the node before it follows the log.
    Starting,
    /// Following the log; its running strategies place orders.
    Running,
    /// Following the log, placing no order.
    Stopped,
    /// Stopped, and closing its positions once the log has passed its last order.
    Flattening,
    /// Stopped on its own: see the bot's `Halt`.
    Halted,
}

/// What the console shows of an account's own state.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AccountStatus {
    pub state: AccountState,
    /// Why the account halted.
    pub halt: Option<String>,
    pub strategies: Vec<StrategyStatus>,
}

/// One strategy of an account.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StrategyStatus {
    pub id: usize,
    pub kind: &'static str,
    pub market: String,
    #[serde(with = "humantime_serde")]
    pub bar: Duration,
    pub running: bool,
    pub params: SmaParams,
    /// The periods the console may set; `None` when it may set none.
    pub bounds: Option<SmaBounds>,
    /// Whether the strategy has warmed up from the marks history and decides on each bar.
    pub warmed_up: bool,
    /// The position the strategy last wanted: `long`, `short` or `flat`.
    pub target: Option<&'static str>,
}

/// A request from the console, answered once the account has acted on it.
#[derive(Debug)]
pub struct AccountRequest {
    /// The id of the console request that sent it, which the account's log lines for it carry.
    pub id: u64,
    pub control: Control,
    pub reply: Reply,
}

/// What the console asks of an account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    /// Lets the account's strategies place orders, or stops them.
    Run { running: bool },
    /// Lets one strategy place orders, or stops it.
    RunStrategy { strategy: usize, running: bool },
    /// Sets a strategy's periods, within the config's bounds; it warms up again from the history.
    SetParams { strategy: usize, params: SmaParams },
    /// Stops the account, then cancels its resting orders and closes every position.
    Flatten,
}

/// Where the answer to a request goes.
pub type Reply = oneshot::Sender<Result<(), Refusal>>;

/// Why a request did not do what it asked.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("the account has no strategy {0}")]
    NoStrategy(usize),
    /// The request asks for what the config does not allow.
    #[error("{0}")]
    Invalid(String),
    /// The account could not do it, or not all of it.
    #[error("{0}")]
    Failed(String),
}

/// The console's end of an account.
#[derive(Clone, Debug)]
pub struct AccountLink {
    pub address: AccountId,
    pub status: watch::Receiver<AccountStatus>,
    pub requests: mpsc::Sender<AccountRequest>,
}

/// The account's end: its controls, the status it publishes, and the requests it takes.
pub struct Link {
    saved: Saved,
    /// Where the controls are saved; `None` without a console, which sets none.
    path: Option<PathBuf>,
    market: String,
    bar: Duration,
    bounds: Option<SmaBounds>,
    status: watch::Sender<AccountStatus>,
    requests: Option<mpsc::Receiver<AccountRequest>>,
    started: bool,
    halt: Option<String>,
    target: Option<Target>,
    /// Flatten requests waiting for the log to pass the account's last order, by console request id.
    flattens: Vec<(u64, Reply)>,
}

impl Link {
    /// The account's end with the config's controls and no console.
    pub fn detached(cfg: &Config) -> Self {
        Self::new(cfg, Saved::from_config(cfg), None).0
    }

    /// Both ends for `address`, with the controls saved under `data_dir`, or the config's when none
    /// are saved.
    /// Error cases:
    /// - the saved controls cannot be read (see `Saved::load`)
    pub async fn open(cfg: &Config, address: AccountId, data_dir: PathBuf) -> anyhow::Result<(Self, AccountLink)> {
        let path = saved::path(&data_dir, address);
        let saved = Saved::load(path.clone(), cfg).await?;
        let (link, console) = Self::new(cfg, saved, Some(path));
        Ok((link, AccountLink { address, status: console.0, requests: console.1 }))
    }

    /// The account's end, and the console's receiver and sender.
    fn new(
        cfg: &Config,
        saved: Saved,
        path: Option<PathBuf>,
    ) -> (Self, (watch::Receiver<AccountStatus>, mpsc::Sender<AccountRequest>)) {
        let (requests_tx, requests) = mpsc::channel(REQUESTS);
        let link = Self {
            saved,
            path,
            market: cfg.strategy.market.clone(),
            bar: cfg.strategy.bar,
            bounds: cfg.strategy.bounds,
            status: watch::Sender::new(AccountStatus { state: AccountState::Starting, halt: None, strategies: vec![] }),
            requests: Some(requests),
            started: false,
            halt: None,
            target: None,
            flattens: Vec::new(),
        };
        link.status.send_replace(link.status_now(false));
        let status = link.status.subscribe();
        (link, (status, requests_tx))
    }

    /// The strategy's periods.
    pub fn params(&self) -> SmaParams {
        self.saved.strategies[0].params
    }

    /// Whether the account and its strategy may place orders.
    pub fn trading(&self) -> bool {
        self.saved.trading()
    }

    /// The requests to take; `None` once taken.
    pub(super) fn take_requests(&mut self) -> Option<mpsc::Receiver<AccountRequest>> {
        self.requests.take()
    }

    /// Marks the account halted, for `reason`, and publishes it.
    pub(super) fn halted(&mut self, reason: String, warmed_up: bool) {
        self.halt = Some(reason);
        self.publish(warmed_up);
    }

    /// Publishes the status, when it changed.
    pub(super) fn publish(&self, warmed_up: bool) {
        let now = self.status_now(warmed_up);
        self.status.send_if_modified(|status| {
            let changed = *status != now;
            *status = now;
            changed
        });
    }

    /// The status as it stands.
    fn status_now(&self, warmed_up: bool) -> AccountStatus {
        let state = match (self.started, &self.halt, self.flattens.is_empty(), self.saved.running) {
            (_, Some(_), _, _) => AccountState::Halted,
            (false, None, _, _) => AccountState::Starting,
            (true, None, false, _) => AccountState::Flattening,
            (true, None, true, true) => AccountState::Running,
            (true, None, true, false) => AccountState::Stopped,
        };
        let strategy = &self.saved.strategies[0];
        let target = self.target.map(|target| match target {
            Target::Long => "long",
            Target::Short => "short",
            Target::Flat => "flat",
        });
        AccountStatus {
            state,
            halt: self.halt.clone(),
            strategies: vec![StrategyStatus {
                id: 0,
                kind: SMA_CROSSOVER,
                market: self.market.clone(),
                bar: self.bar,
                running: strategy.running,
                params: strategy.params,
                bounds: self.bounds,
                warmed_up,
                target,
            }],
        }
    }

    /// Saves `next` as the controls, and takes them once saved.
    /// Error cases:
    /// - the controls cannot be written (see `Saved::save`)
    async fn save(&mut self, next: Saved) -> anyhow::Result<()> {
        if let Some(path) = &self.path {
            next.save(path.clone()).await?;
        }
        self.saved = next;
        Ok(())
    }
}

impl Bot {
    /// Marks the account started and publishes its status.
    pub(super) fn started(&mut self) {
        self.link.started = true;
        self.publish();
    }

    /// Notes the strategy's latest target.
    pub(super) fn targeted(&mut self, target: Option<Target>) {
        if target.is_some() && target != self.link.target {
            self.link.target = target;
            self.publish();
        }
    }

    /// Publishes the account's status.
    pub(super) fn publish(&self) {
        self.link.publish(self.bars.is_some());
    }

    /// Acts on a request from the console and answers it; a flatten waits, unanswered, while an
    /// order the account signed may still enter the log.
    /// Error cases:
    /// - an answer signed by another key than the signer
    pub(super) async fn on_request(&mut self, request: AccountRequest) -> Result<(), Halt> {
        let AccountRequest { id, control, reply } = request;
        let span = info_span!("console_control", id, ?control);
        async {
            info!("console request");
            match control {
                Control::Run { running } => {
                    let mut next = self.link.saved.clone();
                    next.running = running;
                    answer(reply, self.set_controls(next, !running).await);
                }
                Control::RunStrategy { strategy, running } => {
                    let result = match self.strategy_controls(strategy) {
                        Ok(mut next) => {
                            next.strategies[strategy].running = running;
                            self.set_controls(next, !running).await
                        }
                        Err(refusal) => Err(refusal),
                    };
                    answer(reply, result);
                }
                Control::SetParams { strategy, params } => {
                    let result = self.set_params(strategy, params).await;
                    answer(reply, result);
                }
                Control::Flatten => {
                    let mut next = self.link.saved.clone();
                    next.running = false;
                    if let Err(refusal) = self.set_controls(next, true).await {
                        answer(reply, Err(refusal));
                    } else {
                        self.link.flattens.push((id, reply));
                        info!("flattening once the log has passed the account's last order");
                    }
                }
            }
            self.publish();
        }
        .instrument(span)
        .await;
        self.flatten_when_settled().await
    }

    /// Flattens the account for the waiting requests once the log has passed every order it signed,
    /// so no fill of one is still to come.
    /// Error cases:
    /// - an answer signed by another key than the signer
    pub(super) async fn flatten_when_settled(&mut self) -> Result<(), Halt> {
        if self.link.flattens.is_empty() || self.log_ts <= self.order_barrier() {
            return Ok(());
        }
        let ids: Vec<u64> = self.link.flattens.iter().map(|(id, _)| *id).collect();
        let span = info_span!("console_flatten", requests = ?ids);
        async {
            let result = self.flatten().await?;
            for (_, reply) in std::mem::take(&mut self.link.flattens) {
                answer(reply, result.clone());
            }
            self.publish();
            Ok(())
        }
        .instrument(span)
        .await
    }

    /// The controls with strategy `id` in them.
    /// Error cases:
    /// - the account has no such strategy
    fn strategy_controls(&self, id: usize) -> Result<Saved, Refusal> {
        if id >= self.link.saved.strategies.len() {
            return Err(Refusal::NoStrategy(id));
        }
        Ok(self.link.saved.clone())
    }

    /// Saves and takes `next`. A stop (`stopping`) holds even when it cannot be saved, since a stop
    /// is the safe way to be wrong; anything else that cannot be saved changes nothing.
    /// Error cases:
    /// - the controls cannot be written
    async fn set_controls(&mut self, next: Saved, stopping: bool) -> Result<(), Refusal> {
        let Err(e) = self.link.save(next.clone()).await else { return Ok(()) };
        if stopping {
            self.link.saved = next;
            error!(alert = "console_controls_unsaved", "the account stopped, but the stop was not saved: {e:#}");
            return Err(Refusal::Failed(format!(
                "the account stopped, but the stop was not saved, so a restart would start it again: {e:#}"
            )));
        }
        warn!("the console's controls were not saved, so nothing changed: {e:#}");
        Err(Refusal::Failed(format!("nothing changed: the controls could not be saved: {e:#}")))
    }

    /// Sets strategy `id`'s periods within the config's bounds, and starts its warm-up again from
    /// the marks history: its averages are over bars of the old periods.
    /// Error cases:
    /// - the account has no such strategy, the config sets no bounds, or the periods lie outside them
    ///   or are ones the strategy refuses
    /// - the controls cannot be saved
    async fn set_params(&mut self, id: usize, params: SmaParams) -> Result<(), Refusal> {
        let mut next = self.strategy_controls(id)?;
        let Some(bounds) = self.link.bounds else {
            return Err(Refusal::Invalid("the config sets no bounds for the periods".into()));
        };
        if !bounds.admit(params) {
            return Err(Refusal::Invalid(format!(
                "the fast period must lie within {:?} and the slow within {:?}",
                bounds.fast, bounds.slow
            )));
        }
        let strategy = SmaCrossover::new(params.fast, params.slow).map_err(|e| Refusal::Invalid(e.to_string()))?;
        if next.strategies[id].params == params {
            return Ok(());
        }
        next.strategies[id].params = params;
        self.set_controls(next, false).await?;
        info!(fast = params.fast, slow = params.slow, "new periods; warming up again");
        self.strategy = strategy;
        // The periods are at most `MAX_PERIOD`, which `SmaCrossover::new` checked.
        self.history_bars = i64::try_from(params.slow).expect("a period fits an i64");
        self.bars = None;
        self.exact_from = None;
        self.pending_moves.clear();
        self.warm_up_after = Millis::MIN;
        self.link.target = None;
        Ok(())
    }

    /// Cancels the account's resting orders and closes each of its positions at the node's mark,
    /// then reads the account to confirm nothing is left.
    /// Error cases:
    /// - an answer signed by another key than the signer
    async fn flatten(&mut self) -> Result<Result<(), Refusal>, Halt> {
        info!("flattening the account");
        let unread = |e: Halt| Refusal::Failed(format!("the account cannot be read: {e}"));
        let view = match self.read_account().await {
            Ok(view) => view,
            Err(Halt::WrongSigner) => return Err(Halt::WrongSigner),
            Err(e) => return Ok(Err(unread(e))),
        };
        if !view.orders.is_empty() {
            self.cancel_resting().await?;
        }
        let held: Vec<_> = view.positions.iter().filter(|(_, p)| !p.qty.is_zero()).map(|(m, _)| *m).collect();
        if !held.is_empty() {
            let marks = self.node_marks().await?;
            let quoted = |market| marks.get(&market).copied().or(self.mark.filter(|_| market == self.market));
            let closes: Vec<_> =
                held.iter().filter_map(|&market| Some(CloseEntry { market, quoted: quoted(market)? })).collect();
            if !closes.is_empty() {
                self.close_positions(closes).await?;
            }
        }
        let left = match self.read_account().await {
            Ok(view) => view,
            Err(Halt::WrongSigner) => return Err(Halt::WrongSigner),
            Err(e) => return Ok(Err(unread(e))),
        };
        let positions = left.positions.values().filter(|p| !p.qty.is_zero()).count();
        if left.orders.is_empty() && positions == 0 {
            info!("flattened");
            return Ok(Ok(()));
        }
        let reason = format!("{} resting orders and positions in {positions} markets are left", left.orders.len());
        warn!(alert = "flatten_incomplete", "{reason}");
        Ok(Err(Refusal::Failed(reason)))
    }
}

/// Answers a request; a console that stopped waiting has nothing to hear it.
fn answer(reply: Reply, result: Result<(), Refusal>) {
    if let Err(refusal) = &result {
        info!(%refusal, "console request refused");
    }
    let _ = reply.send(result);
}

#[cfg(test)]
mod tests;
