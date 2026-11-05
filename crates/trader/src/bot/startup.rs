//! What the bot reads before it follows the log: the markets, each account's balance and
//! positions, each a verified answer; the keys it unlocks; and the stop signals it listens for.
//! An account that cannot start is reported and left out; the others carry on.

use std::collections::{BTreeMap, BTreeSet};
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
use crate::bot::{bar_fits, supported};
use crate::config::{AccountPlan, Config, KeySource};
use crate::keys::AccountKey;
use crate::node::api::AccountView;
use crate::node::{CallError, Node, Tls, Verified};
use crate::stream::Stream;

/// How many times an account is read at startup before it is left out, and the wait between tries.
const READ_TRIES: u32 = 3;
const READ_RETRY: std::time::Duration = std::time::Duration::from_secs(1);
/// How many keystores are decrypted at once: key derivation is slow by design.
const UNLOCKS_AT_ONCE: usize = 4;

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
    pub failed: Vec<(String, String)>,
}

/// The fleet for `cfg`'s accounts, and the log from the seq after the head before the reads.
/// Error cases:
/// - the node cannot be reached, or an answer does not verify
/// - no account can start
pub(super) async fn start(cfg: &Config, env: &(dyn Fn(&str) -> Option<String> + Sync)) -> anyhow::Result<Started> {
    let node_cfg = &cfg.node;
    let tls = Tls::new(node_cfg.url.clone(), node_cfg.tls_key_sha256)?;
    let deployment = Deployment { chain_id: node_cfg.chain_id, vault: node_cfg.vault.0 };
    let node =
        Arc::new(Node::new(tls.clone(), node_cfg.signer.0, deployment, node_cfg.request_timeout, &node_cfg.limits));
    let mut failed: Vec<(String, String)> = Vec::new();

    let mut unlocked: Vec<(&AccountPlan, AccountKey)> = Vec::new();
    let mut keys = Vec::with_capacity(cfg.plans.len());
    for chunk in cfg.plans.chunks(UNLOCKS_AT_ONCE) {
        let unlocked = join_all(chunk.iter().map(|plan| AccountKey::load(&plan.key, env))).await;
        keys.extend(chunk.iter().zip(unlocked));
    }
    let mut seen = BTreeSet::new();
    for (plan, key) in keys {
        match key {
            Ok(key) if !seen.insert(key.address()) => {
                failed.push((label(plan), format!("{} is listed twice", key.address())))
            }
            Ok(key) => unlocked.push((plan, key)),
            Err(e) => failed.push((label(plan), format!("{e:#}"))),
        }
    }
    let Some((_, first)) = unlocked.first() else { bail!("no account can start: {}", describe(&failed)) };

    // The log head as it stood before the reads below: their views hold at least that much of the
    // log, so the stream, which starts after it, misses nothing they do not show. What it repeats
    // applies again to the same effect: a fill's position is absolute, and a listing's rules are
    // the same rules.
    let head_seq = node.nonce(first.address()).await?.head_seq;
    let listed = node.markets().await?.body;
    let mut usable: BTreeMap<String, Result<MarketIn, String>> = BTreeMap::new();
    for symbol in unlocked.iter().flat_map(|(plan, _)| plan.strategies.iter().map(|s| &s.market)) {
        usable.entry(symbol.clone()).or_insert_with(|| match listed.iter().find(|m| &m.spec.symbol == symbol) {
            None => Err(format!("the node lists no market {symbol}")),
            Some(m) => supported(&m.spec)
                .map(|()| MarketIn { spec: m.spec.clone(), mark: m.mark })
                .map_err(|e| format!("{e:#}")),
        });
    }

    let mut candidates = Vec::new();
    for (plan, key) in unlocked {
        match plan.strategies.iter().find_map(|s| usable[&s.market].as_ref().err()) {
            Some(reason) => failed.push((label(plan), reason.clone())),
            None => candidates.push((plan, Arc::new(key))),
        }
    }
    let mut views = Vec::with_capacity(candidates.len());
    for chunk in candidates.chunks(node_cfg.limits.connections * 2) {
        views.extend(
            join_all(
                chunk.iter().map(|(_, key)| async {
                    (read_account(&node, key.address()).await, node.nonce(key.address()).await)
                }),
            )
            .await,
        );
    }
    let mut accounts = Vec::new();
    let mut read_at: u64 = 0;
    for ((plan, key), (view, nonce)) in candidates.into_iter().zip(views) {
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
                failed.push((label(plan), format!("the node has never credited {address}: fund and declare it first")));
                continue;
            }
            Err(e) => {
                failed.push((label(plan), format!("{e}")));
                continue;
            }
        };
        read_at = read_at.max(view.time_ms);
        info!(%address, balance = %view.body.balance, head_seq, "account read");
        match account_in(plan, key, &view, nonce, &usable, head_seq, &cfg.state.dir).await {
            Ok(account) => accounts.push(account),
            Err(e) => failed.push((label(plan), format!("{e:#}"))),
        }
    }
    for (account, reason) in &failed {
        error!(alert = "account_failed", account, "{reason}; it does not trade");
    }
    if accounts.is_empty() {
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
    let fleet = Fleet::new(&markets, accounts, trading_from)?;
    let domain = eip712::domain(node_cfg.chain_id, node_cfg.vault);
    let stream = Stream::new(tls, node_cfg.signer.0, deployment, node_cfg.stream_timeout, head_seq + 1);
    Ok(Started { fleet, node, stream, domain, failed })
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

/// The account `plan` names, ready to trade: its positions from `view`, its controls resumed from the
/// state kept for it, and that state written again as the start leaves it.
/// Error cases:
/// - the account holds a position in a market it does not trade, or a bar that never fits a market's hours
/// - the sizing, risk limits or state file are refused (see `Controls`, `StateFile`)
async fn account_in(
    plan: &AccountPlan,
    key: Arc<AccountKey>,
    view: &Verified<AccountView>,
    nonce: Option<u64>,
    usable: &BTreeMap<String, Result<MarketIn, String>>,
    head_seq: u64,
    state_dir: &std::path::Path,
) -> anyhow::Result<AccountIn> {
    let address = key.address();
    let now = Millis::try_from(view.time_ms).context("node time")?;
    let traded: BTreeSet<MarketId> =
        plan.strategies.iter().filter_map(|s| usable[&s.market].as_ref().ok().map(|m| m.spec.id)).collect();
    if let Some((other, _)) = view.body.positions.iter().find(|(id, p)| !p.qty.is_zero() && !traded.contains(id)) {
        bail!("{address} holds a position in market {other}: close it first, as this bot does not trade or limit it");
    }
    let state_file = StateFile::new(state_dir, address);
    let mut saved = state_file.load().await?;
    saved.migrate(&traded.iter().copied().collect::<Vec<_>>())?;
    let mut controls = Controls::new(plan.risk.clone(), plan.sizing.clone(), saved.day)?;
    let mut strategies = Vec::with_capacity(plan.strategies.len());
    for s in &plan.strategies {
        let market = usable[&s.market].as_ref().map_err(|e| anyhow::anyhow!("{e}"))?;
        let bar_ms = Millis::try_from(s.bar.as_millis()).context("bar length")?;
        bar_fits(&market.spec, bar_ms, now)?;
        controls.check_market(&market.spec).with_context(|| format!("sizing for {}", s.market))?;
        let held = view.body.positions.get(&market.spec.id);
        strategies.push(StrategyIn {
            market: market.spec.id,
            bar_ms,
            params: s.params.clone(),
            max_slippage_bps: s.max_slippage_bps,
            position: held.map_or(Fixed::ZERO, |p| p.qty),
            entry: held.map_or(Fixed::ZERO, |p| p.entry),
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
        balance: view.body.balance,
        nonce,
        controls,
        state_file,
        saved: kept,
        strategies,
    })
}

/// What names an account in a report before its address is known: its key file.
fn label(plan: &AccountPlan) -> String {
    match &plan.key {
        KeySource::Keystore { path, .. } | KeySource::HexFile { path } => path.display().to_string(),
    }
}

/// The failures, as one line.
fn describe(failed: &[(String, String)]) -> String {
    failed.iter().map(|(account, reason)| format!("{account}: {reason}")).collect::<Vec<_>>().join("; ")
}
