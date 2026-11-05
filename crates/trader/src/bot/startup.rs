//! What the bot reads before it follows the log: the markets, each account's balance and
//! positions, each a verified answer; the keys it unlocks; and the stop signals it listens for.
//! An account that cannot start is reported and left out; the others carry on.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use anyhow::{Context, bail};
use engine_core::Fixed;
use engine_core::eip712::{self, Eip712Domain};
use engine_core::types::{MarketId, Millis};
use engine_wire::Deployment;
use futures_util::future::join_all;
use tokio::signal::unix::{SignalKind, signal};
use tracing::{error, info};

use crate::bot::fleet::{AccountIn, EXPIRES_IN_MS, Fleet, MarketIn, StrategyIn};
use crate::bot::supported;
use crate::config::{AccountPlan, Config, KeySource};
use crate::keys::AccountKey;
use crate::node::api::AccountView;
use crate::node::{CallError, Node, Tls};
use crate::stream::Stream;

/// How many keystores are decrypted at once: key derivation is slow by design.
const UNLOCKS_AT_ONCE: usize = 8;

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
                .map(|()| MarketIn { id: m.spec.id, rules: m.spec.price_rules, mark: m.mark })
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
        views.extend(join_all(chunk.iter().map(|(_, key)| node.account(key.address()))).await);
    }
    let mut accounts = Vec::new();
    let mut read_at: u64 = 0;
    for ((plan, key), view) in candidates.into_iter().zip(views) {
        let address = key.address();
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
        if view.body.balance <= Fixed::ZERO {
            failed.push((label(plan), format!("{address} holds no balance")));
            continue;
        }
        read_at = read_at.max(view.time_ms);
        info!(%address, balance = %view.body.balance, head_seq, "account read");
        accounts.push(account_in(plan, key, &view.body, &usable)?);
    }
    for (account, reason) in &failed {
        error!(alert = "account_failed", account, "{reason}; it does not trade");
    }
    if accounts.is_empty() {
        bail!("no account can start: {}", describe(&failed));
    }
    let used: BTreeSet<MarketId> = accounts.iter().flat_map(|a| a.strategies.iter().map(|s| s.market)).collect();
    let markets: Vec<MarketIn> =
        usable.values().filter_map(|m| m.as_ref().ok()).filter(|m| used.contains(&m.id)).cloned().collect();
    // An order an earlier process signed before this start expires by now plus its window.
    let trading_from = Millis::try_from(read_at).context("node time")?.saturating_add(EXPIRES_IN_MS);
    info!(accounts = accounts.len(), failed = failed.len(), markets = markets.len(), "started");
    let fleet = Fleet::new(&markets, accounts, trading_from)?;
    let domain = eip712::domain(node_cfg.chain_id, node_cfg.vault);
    let stream = Stream::new(tls, node_cfg.signer.0, deployment, node_cfg.stream_timeout, head_seq + 1);
    Ok(Started { fleet, node, stream, domain, failed })
}

/// The account `plan` names, with its positions from `view`.
/// Error cases:
/// - a bar length is out of range
fn account_in(
    plan: &AccountPlan,
    key: Arc<AccountKey>,
    view: &AccountView,
    usable: &BTreeMap<String, Result<MarketIn, String>>,
) -> anyhow::Result<AccountIn> {
    let strategies = plan
        .strategies
        .iter()
        .map(|s| {
            let market = usable[&s.market].as_ref().map_err(|e| anyhow::anyhow!("{e}"))?.id;
            Ok(StrategyIn {
                market,
                bar_ms: Millis::try_from(s.bar.as_millis()).context("bar length")?,
                fast: s.fast,
                slow: s.slow,
                qty: s.qty,
                max_slippage_bps: s.max_slippage_bps,
                position: view.positions.get(&market).map_or(Fixed::ZERO, |p| p.qty),
            })
        })
        .collect::<anyhow::Result<_>>()?;
    Ok(AccountIn { key, close_on_stop: plan.close_on_stop, balance: view.balance, strategies })
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
