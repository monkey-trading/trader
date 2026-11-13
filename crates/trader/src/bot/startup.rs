//! What the bot reads before it follows the log: the node's attestation, then the markets, each
//! account's balance and positions, each a verified answer; the keys it unlocks; and the stop
//! signals it listens for.
//! An account that cannot start is reported and left out; the others carry on.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, bail};
use engine_core::Fixed;
use engine_core::eip712::{self, Eip712Domain};
use engine_core::types::{AccountId, MarketId, Millis};
use engine_wire::Deployment;
use futures_util::future::join_all;
use tokio::signal::unix::{SignalKind, signal};
use tokio::time::sleep;
use tracing::{error, info};
use trader_core::controls::{Account as Controlled, Controls};
use trader_core::margin::Holding;

use crate::bot::day::{Saved, SavedPosition, StateFile};
use crate::bot::fleet::{AccountIn, EXPIRES_IN_MS, Fleet, MarketIn, StrategyIn};
use crate::bot::link::{self, Kept, Saved as ConsoleSaved, StartFailure};
use crate::bot::{bar_fits, supported};
use crate::config::{AccountPlan, Config, KeySource};
use crate::keys::AccountKey;
use crate::metrics::Metrics;
use crate::node::api::AccountView;
use crate::node::attestation::attest_node;
use crate::node::{CallError, Node, Verified};
use crate::stream::Stream;

/// How many times an account is read at startup before it is left out, and the wait between tries.
const READ_TRIES: u32 = 3;
const READ_RETRY: std::time::Duration = std::time::Duration::from_secs(1);
/// How many keystores are decrypted at once: key derivation is slow by design.
pub(crate) const UNLOCKS_AT_ONCE: usize = 4;

/// A future that completes on SIGINT or SIGTERM.
/// Error cases:
/// - a signal handler cannot be installed
pub fn stop_signal() -> anyhow::Result<impl Future<Output = ()>> {
    let mut interrupt = signal(SignalKind::interrupt()).context("SIGINT handler")?;
    let mut terminate = signal(SignalKind::terminate()).context("SIGTERM handler")?;
    Ok(async move {
        tokio::select! {
            _ = interrupt.recv() => {}
            _ = terminate.recv() => {}
        }
    })
}

/// What `start` hands to the run.
pub(super) struct Started {
    pub fleet: Fleet,
    pub node: Arc<Node>,
    pub stream: Stream,
    pub domain: Eip712Domain,
    /// The accounts that could not start, and why.
    pub failed: Vec<StartFailure>,
    /// Each started account's controls and what the console names its strategies by, in the fleet's
    /// order.
    pub kept: Vec<Kept>,
    /// The keys of the accounts that did not start but unlocked, which the console's funding acts
    /// for: an account the node has not credited yet starts only once it is funded and declared.
    pub idle: Vec<(usize, Arc<AccountKey>)>,
}

/// The fleet for `cfg`'s accounts, recording into `metrics`, and the log from the seq after the head
/// before the reads, from the node whose attestation verified.
/// Error cases:
/// - the system clock is before 1970
/// - the node's attestation does not verify (see `attest`)
/// - the node cannot be reached, or an answer does not verify
/// - no account can start, and none is left for the console to fund (`fundable`)
pub(super) async fn start(
    cfg: &Config,
    env: &(dyn Fn(&str) -> Option<String> + Sync),
    data_dir: Option<&Path>,
    fundable: bool,
    metrics: &Metrics,
) -> anyhow::Result<Started> {
    let node_cfg = &cfg.node;
    let attested = attest_node(node_cfg).await?;
    let deployment = Deployment { chain_id: node_cfg.chain_id, vault: node_cfg.vault.0 };
    let node = Arc::new(Node::new(
        attested.tls.clone(),
        attested.signer,
        deployment,
        node_cfg.request_timeout,
        &node_cfg.limits,
    ));
    let mut failed: Vec<StartFailure> = Vec::new();

    let mut unlocked: Vec<(usize, &AccountPlan, AccountKey)> = Vec::new();
    let mut keys = Vec::with_capacity(cfg.plans.len());
    for (chunk, plans) in cfg.plans.chunks(UNLOCKS_AT_ONCE).enumerate() {
        let unlocked = join_all(plans.iter().map(|plan| AccountKey::load(&plan.key, env))).await;
        keys.extend((chunk * UNLOCKS_AT_ONCE..).zip(plans.iter().zip(unlocked)));
    }
    let mut seen = BTreeSet::new();
    for (order, (plan, key)) in keys {
        match key {
            Ok(key) if !seen.insert(key.address()) => failed.push(failure(
                order,
                plan,
                Some(key.address()),
                "account_refused",
                format!("{} is listed twice", key.address()),
            )),
            Ok(key) => unlocked.push((order, plan, key)),
            Err(e) => failed.push(failure(order, plan, None, "key_locked", format!("{e:#}"))),
        }
    }
    let Some((_, _, first)) = unlocked.first() else { bail!("no account can start: {}", describe(&failed)) };

    // The log head as it stood before the reads below: their views hold at least that much of the
    // log, so the stream, which starts after it, misses nothing they do not show. What it repeats
    // applies again to the same effect: a fill's position is absolute, and a listing's rules are
    // the same rules.
    let head_seq = node.nonce(first.address()).await?.head_seq;
    let listed = node.markets().await?.body;
    let mut usable: BTreeMap<String, Result<MarketIn, String>> = BTreeMap::new();
    for symbol in unlocked.iter().flat_map(|(_, plan, _)| plan.strategies.iter().map(|s| &s.market)) {
        usable.entry(symbol.clone()).or_insert_with(|| match listed.iter().find(|m| &m.spec.symbol == symbol) {
            None => Err(format!("the node lists no market {symbol}")),
            Some(m) => supported(&m.spec)
                .map(|()| MarketIn { spec: m.spec.clone(), mark: m.mark })
                .map_err(|e| format!("{e:#}")),
        });
    }

    let mut candidates = Vec::new();
    let mut idle: Vec<(usize, Arc<AccountKey>)> = Vec::new();
    for (order, plan, key) in unlocked {
        let key = Arc::new(key);
        match plan.strategies.iter().find_map(|s| usable[&s.market].as_ref().err()) {
            Some(reason) => {
                failed.push(failure(order, plan, Some(key.address()), "market_unsupported", reason.clone()));
                idle.push((order, key));
            }
            None => candidates.push((order, plan, key)),
        }
    }
    let mut views = Vec::with_capacity(candidates.len());
    for chunk in candidates.chunks(node_cfg.limits.connections * 2) {
        views.extend(
            join_all(chunk.iter().map(|(_, _, key)| async {
                (read_account(&node, key.address()).await, node.nonce(key.address()).await)
            }))
            .await,
        );
    }
    let mut accounts = Vec::new();
    let mut kept = Vec::new();
    let mut read_at: u64 = 0;
    for ((order, plan, key), (view, nonce)) in candidates.into_iter().zip(views) {
        let address = key.address();
        let nonce = match nonce {
            Ok(answer) => Some(answer.body.next_nonce),
            Err(CallError::WrongSigner { .. }) => {
                return Err(CallError::WrongSigner { target: "nonce".into() }.into());
            }
            // Unknown: it is read again before the account's first order.
            Err(_) => None,
        };
        let view = match view {
            Ok(view) => view,
            Err(CallError::WrongSigner { .. }) => {
                return Err(CallError::WrongSigner { target: "account".into() }.into());
            }
            Err(CallError::Refused { status: 404, .. }) => {
                let reason = format!("the node has never credited {address}: fund and declare it first");
                failed.push(failure(order, plan, Some(address), "no_balance", reason));
                idle.push((order, key));
                continue;
            }
            Err(e) => {
                failed.push(failure(order, plan, Some(address), "account_unread", e.to_string()));
                idle.push((order, key));
                continue;
            }
        };
        let path = data_dir.map(|dir| link::path(dir, address));
        let saved = match &path {
            Some(path) => match ConsoleSaved::load(path.clone(), plan).await {
                Ok(saved) => saved,
                Err(e) => {
                    failed.push(failure(order, plan, Some(address), "controls_unreadable", format!("{e:#}")));
                    idle.push((order, key));
                    continue;
                }
            },
            None => ConsoleSaved::from_plan(plan),
        };
        read_at = read_at.max(view.time_ms);
        info!(%address, balance = %view.body.balance, head_seq, "account read");
        match account_in(plan, key.clone(), &view, nonce, &usable, head_seq, &cfg.state.dir, &saved).await {
            Ok(account) => {
                accounts.push(account);
                let strategies = plan.strategies.iter().map(|s| (s.market.clone(), s.bar)).collect();
                kept.push(Kept { order, path, saved, strategies });
            }
            Err(e) => {
                failed.push(failure(order, plan, Some(address), e.code, format!("{:#}", e.error)));
                idle.push((order, key));
            }
        }
    }
    for StartFailure { label, reason, .. } in &failed {
        error!(alert = "account_failed", account = label, "{reason}; it does not trade");
    }
    // With the console's funding, accounts that could not start for want of a credit are funded
    // there, so a bot with none trading still serves them.
    if accounts.is_empty() && !(fundable && !idle.is_empty()) {
        bail!("no account can start: {}", describe(&failed));
    }
    let used: BTreeSet<MarketId> = accounts.iter().flat_map(|a| a.strategies.iter().map(|s| s.market)).collect();
    let markets: Vec<MarketIn> =
        usable.values().filter_map(|m| m.as_ref().ok()).filter(|m| used.contains(&m.spec.id)).cloned().collect();
    // An order an earlier process signed before this start expires by now plus its window, plus the
    // lead its estimate of the node's time can have had over the node's: at most an answer's time.
    let lead = Millis::try_from(node_cfg.request_timeout.as_millis()).context("request timeout")?;
    let trading_from =
        Millis::try_from(read_at).context("node time")?.saturating_add(EXPIRES_IN_MS).saturating_add(lead);
    info!(accounts = accounts.len(), failed = failed.len(), markets = markets.len(), "started");
    let mut fleet = Fleet::new(&markets, accounts, trading_from)?;
    fleet.register_metrics(metrics);
    let domain = eip712::domain(node_cfg.chain_id, node_cfg.vault);
    let stream = Stream::new(attested.tls, attested.signer, deployment, node_cfg.stream_timeout, head_seq + 1);
    Ok(Started { fleet, node, stream, domain, failed, kept, idle })
}

/// `address`'s account, read again after a failure that may pass (no answer, or a request to try again
/// later), up to `READ_TRIES` times a second apart.
/// Error cases:
/// - see `CallError`, the last one when every try failed
async fn read_account(node: &Node, address: AccountId) -> Result<Verified<AccountView>, CallError> {
    let mut tries = 1;
    loop {
        match node.account(address).await {
            Err(CallError::Failed { .. } | CallError::RetryLater { .. }) if tries < READ_TRIES => {
                tries += 1;
                sleep(READ_RETRY).await;
            }
            other => return other,
        }
    }
}

/// Why an account could not be made ready, and what kind of reason it is.
struct Refused {
    code: &'static str,
    error: anyhow::Error,
}

impl<E: Into<anyhow::Error>> From<E> for Refused {
    fn from(error: E) -> Self {
        Self { code: "account_refused", error: error.into() }
    }
}

/// The account `plan` names, ready to trade: its positions from `view`, its controls resumed from the
/// state kept for it, that state written again as the start leaves it, and the console's controls
/// `saved`, one per strategy in the plan's order.
/// Error cases:
/// - the account holds a position in a market it does not trade (`position_elsewhere`), or a bar that
///   never fits a market's hours
/// - the sizing, risk limits or state file are refused (see `Controls`, `StateFile`)
#[expect(clippy::too_many_arguments, reason = "what one account's start reads, each from its own source")]
async fn account_in(
    plan: &AccountPlan,
    key: Arc<AccountKey>,
    view: &Verified<AccountView>,
    nonce: Option<u64>,
    usable: &BTreeMap<String, Result<MarketIn, String>>,
    head_seq: u64,
    state_dir: &std::path::Path,
    console: &ConsoleSaved,
) -> Result<AccountIn, Refused> {
    let address = key.address();
    let now = Millis::try_from(view.time_ms).context("node time")?;
    let traded: BTreeSet<MarketId> =
        plan.strategies.iter().filter_map(|s| usable[&s.market].as_ref().ok().map(|m| m.spec.id)).collect();
    if let Some((other, _)) = view.body.positions.iter().find(|(id, p)| !p.qty.is_zero() && !traded.contains(id)) {
        let error = anyhow::anyhow!(
            "{address} holds a position in market {other}: close it first, as this bot does not trade or limit it"
        );
        return Err(Refused { code: "position_elsewhere", error });
    }
    let state_file = StateFile::new(state_dir, address);
    let mut saved = state_file.load().await?;
    saved.migrate(&traded.iter().copied().collect::<Vec<_>>())?;
    let mut controls = Controls::new(plan.risk.clone(), plan.sizing.clone(), saved.day)?;
    let mut strategies = Vec::with_capacity(plan.strategies.len());
    for (s, set) in plan.strategies.iter().zip(&console.strategies) {
        let market = usable[&s.market].as_ref().map_err(|e| anyhow::anyhow!("{e}"))?;
        let bar_ms = Millis::try_from(s.bar.as_millis()).context("bar length")?;
        bar_fits(&market.spec, bar_ms, now)?;
        controls.check_market(&market.spec).with_context(|| format!("sizing for {}", s.market))?;
        let held = view.body.positions.get(&market.spec.id);
        strategies.push(StrategyIn {
            market: market.spec.id,
            bar_ms,
            params: set.params.clone(),
            max_slippage_bps: s.max_slippage_bps,
            position: held.map_or(Fixed::ZERO, |p| p.qty),
            entry: held.map_or(Fixed::ZERO, |p| p.entry),
            running: set.running,
            bounds: s.bounds.clone(),
        });
    }
    // A position keeps the opening time saved for it, if it is on the side and at the entry saved. One
    // with none, as when a crash came between its fill and the save, is held from now: the hold is the
    // conservative guess.
    let opened: BTreeMap<MarketId, Millis> = strategies
        .iter()
        .filter(|s| !s.position.is_zero())
        .map(|s| {
            let kept =
                saved.positions.get(&s.market).is_some_and(|p| p.side == s.position.signum() && p.entry == s.entry);
            (s.market, saved.opened.get(&s.market).copied().filter(|_| kept).unwrap_or(now))
        })
        .collect();
    controls.restore_opened(opened);
    // The day's opening equity is the account's now, unless a restart finds today's already kept; the
    // markets and the account must have been read at the same point of the log to be valued together.
    if view.head_seq == head_seq {
        let holdings: Option<Vec<Holding<'_>>> = strategies
            .iter()
            .filter(|s| !s.position.is_zero())
            .map(|s| {
                let market = usable.values().filter_map(|m| m.as_ref().ok()).find(|m| m.spec.id == s.market)?;
                Some(Holding {
                    market: s.market,
                    qty: s.position,
                    entry: s.entry,
                    mark: market.mark?,
                    spec: &market.spec,
                })
            })
            .collect();
        if let Some(holdings) = holdings {
            controls.observe(now, &Controlled { balance: view.body.balance, holdings: &holdings });
        }
    }
    let kept = Saved {
        day: controls.day_state(),
        opened: controls.opened().clone(),
        positions: strategies
            .iter()
            .filter(|s| !s.position.is_zero())
            .map(|s| (s.market, SavedPosition { side: s.position.signum(), entry: s.entry }))
            .collect(),
        splits: saved.splits,
        ..Saved::default()
    };
    state_file.save(kept.clone()).await.context("keeping the saved state")?;
    Ok(AccountIn {
        key,
        close_on_stop: plan.close_on_stop,
        running: console.running,
        balance: view.body.balance,
        nonce,
        controls,
        state_file,
        saved: kept,
        strategies,
    })
}

/// What names an account in a report before its address is known: its key file.
pub(crate) fn label(plan: &AccountPlan) -> String {
    match &plan.key {
        KeySource::Keystore { path, .. } | KeySource::HexFile { path } => path.display().to_string(),
    }
}

/// `plan`'s account, the `order`th of the config's, as one that could not start for `reason`.
fn failure(
    order: usize,
    plan: &AccountPlan,
    address: Option<AccountId>,
    code: &'static str,
    reason: impl Into<String>,
) -> StartFailure {
    StartFailure { order, label: label(plan), address, code, reason: reason.into() }
}

/// The failures, as one line.
fn describe(failed: &[StartFailure]) -> String {
    failed.iter().map(|f| format!("{}: {}", f.label, f.reason)).collect::<Vec<_>>().join("; ")
}
