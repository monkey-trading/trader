//! Many accounts trading on one log. At startup the bot unlocks each account's key and reads the
//! markets and the accounts from the node; from then on it follows the public log once, over one
//! stream connection: each `Prices` command moves its market's index mark, each completed bar goes
//! to the strategies that read it, and the accounts' fills move their positions. A target an
//! account's position is not at queues a signed market order (`run`), unless the bar is history the
//! stream is replaying, or an earlier order of the account could still enter the log. An account
//! that fails stops alone; the others carry on.

mod apply;
pub mod day;
mod decide;
mod fills;
mod flatten;
pub mod fleet;
pub mod link;
pub use link::AccountLink;
mod orders;
mod params;
mod reads;
mod run;
mod startup;
pub use startup::stop_signal;
pub(crate) use startup::{UNLOCKS_AT_ONCE, label};
mod stop;
mod warm;

use std::sync::Arc;

use anyhow::bail;
use engine_core::market::{MarketSpec, QuoteCurrency};
use engine_core::types::{AccountId, Millis};
use tokio::sync::oneshot;
use tracing::{Instrument, Span, error, info};

use crate::accounts::{Lifecycle, claim};
use crate::bot::link::ConsoleEnd;
use crate::config::Config;
use crate::console::{Console, Prepared};
use crate::keys::AccountKey;
use crate::node::CallError;

pub use run::Runner;
pub use warm::WarmRequest;

/// Why the bot stopped on its own.
#[derive(Debug, thiserror::Error)]
pub enum Halt {
    /// An answer or frame from the node is not the configured signer's: nothing it says is trusted,
    /// and nothing more is sent to it.
    #[error("the node's answers are not signed by the configured signer")]
    WrongSigner,
    /// The log holds what the bot cannot follow; it stops before acting on a view it cannot confirm.
    #[error("{0:#}")]
    Unfollowable(anyhow::Error),
    /// Every account has stopped trading, each for its own reason; nothing is left to follow the
    /// log for, and the accounts are left as they are.
    #[error("every account has stopped trading: {0}")]
    AllStopped(String),
    /// Stopping left resting orders, or a position it was to close, or could not confirm it did not.
    #[error("the stop is incomplete: {0}")]
    StopIncomplete(String),
}

/// The address of every account the config names, in config order.
/// Error cases:
/// - a key cannot be unlocked
pub async fn addresses(cfg: &Config, env: impl Fn(&str) -> Option<String>) -> anyhow::Result<Vec<AccountId>> {
    let mut addresses = Vec::with_capacity(cfg.plans.len());
    for plan in &cfg.plans {
        addresses.push(AccountKey::load(&plan.key, &env).await?.address());
    }
    Ok(addresses)
}

/// Runs the bot until `stop` completes, then cancels each account's resting orders and, when
/// configured, closes its positions; or until it halts on its own, leaving the accounts as they are.
/// It returns an error when any account could not start or stopped trading, so a supervisor sees it.
/// Error cases:
/// - no key or startup read works (see `startup::start`)
/// - the bot halts (see `Halt`)
/// - an account could not start, stopped trading, or was not left as the stop sets out
/// - the claims cannot be set up (see `Lifecycle::with_node`), or the claims task ends
pub async fn run(
    cfg: Config,
    env: impl Fn(&str) -> Option<String> + Sync,
    stop: impl Future<Output = ()>,
) -> anyhow::Result<()> {
    run_with_console(cfg, env, stop, None).await
}

/// Runs the bot as `run` does, with its console when `console` holds one prepared from the config's
/// section: the console starts once the accounts have, serves them until the bot stops, and its
/// failure stops the bot as `stop` would.
/// Error cases:
/// - see `run`
/// - the console fails
pub async fn run_with_console(
    cfg: Config,
    env: impl Fn(&str) -> Option<String> + Sync,
    stop: impl Future<Output = ()>,
    console: Option<Prepared>,
) -> anyhow::Result<()> {
    let console = console.zip(cfg.console.as_ref());
    let data_dir = console.as_ref().map(|(_, section)| section.data_dir.as_path());
    let started = match startup::start(&cfg, &env, data_dir).await {
        Ok(started) => started,
        Err(e) => {
            if let Some(CallError::WrongSigner { .. }) = e.downcast_ref::<CallError>() {
                error!(alert = "node_signer_mismatch", "{e:#}");
            }
            return Err(e);
        }
    };
    let startup::Started { fleet, node, mut stream, domain, failed, kept } = started;
    // With `[claims]`, a task claims every account's MONKEY alongside the trading: should it end, the
    // bot stops as on a stop signal and fails, or, when the node answered it with another key's
    // signature, halts.
    let mut claims = match &cfg.claims {
        Some(claims) => {
            let lifecycle = Arc::new(Lifecycle::with_node(&cfg, node.clone(), &env).await?);
            let keys = fleet.accounts.iter().map(|account| account.key.clone()).collect();
            let watch = claim::watch(lifecycle, keys, claims.threshold.0, claims.interval);
            Some(tokio::spawn(watch.instrument(Span::current())))
        }
        None => None,
    };
    let cap = cfg.node.limits.connections;
    let mut runner = Runner::new(fleet, Arc::clone(&node), domain, cap);
    let (console_failed, mut failed_console) = oneshot::channel::<anyhow::Error>();
    // The console's tasks live in this watcher, which ends them when the bot is done with them.
    let mut watcher = None;
    if let Some((prepared, section)) = console {
        let (end, mut links) = ConsoleEnd::new(&runner.fleet, kept);
        runner.console = Some(end);
        links.extend(ConsoleEnd::halted(&failed));
        let mut console = Console::start(section, prepared, node, cfg.node.limits.per_ip, links).await?;
        watcher = Some(tokio::spawn(async move {
            let _ = console_failed.send(console.failed().await);
        }));
    }
    let mut claims_ended = None;
    let mut failure = None;
    let stop = async {
        let claims_task = async {
            match claims.as_mut() {
                Some(task) => task.await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            () = stop => {}
            ended = claims_task => claims_ended = Some(ended),
            Ok(e) = &mut failed_console => {
                if !signed_elsewhere(&e) {
                    error!(alert = "console_failed", "the console stopped, so the bot stops too: {e:#}");
                }
                failure = Some(e);
            }
        }
    };
    let followed = runner.follow(&mut stream, stop).await;
    // The claims end before the stop acts, and an end they came to first is not lost.
    if let Some(task) = claims.take()
        && claims_ended.is_none()
    {
        task.abort();
        match task.await {
            Err(e) if e.is_cancelled() => {}
            ended => claims_ended = Some(ended),
        }
    }
    let claims_halted = matches!(&claims_ended, Some(Ok(e)) if claim::is_wrong_signer(e));
    let result = match followed {
        Ok(()) if claims_halted => Err(Halt::WrongSigner),
        // The console read an answer another key signed: as with one the runner reads.
        Ok(()) if failure.as_ref().is_some_and(signed_elsewhere) => Err(Halt::WrongSigner),
        Ok(()) => {
            if let Some(ended) = &claims_ended {
                error!(alert = "bot_halted", "the MONKEY claims task ended: {ended:?}; stopping");
            }
            stop_watching(&mut runner, &mut stream, &mut failed_console, &mut failure).await
        }
        // What the bot saw is not what it trades on: it changes nothing, and leaves the accounts to
        // their owners.
        Err(Halt::WrongSigner) => Err(Halt::WrongSigner),
        Err(halt) => {
            error!(alert = "bot_halted", "{halt}; the accounts are left as they are");
            Err(halt)
        }
    };
    runner.flush_state().await;
    if let Err(Halt::WrongSigner) = &result {
        let at = failure.as_ref().filter(|e| signed_elsewhere(e)).map(|e| format!(" ({e:#})")).unwrap_or_default();
        error!(alert = "node_signer_mismatch", "{}{at}", Halt::WrongSigner);
    }
    if let Some(watcher) = watcher {
        watcher.abort();
    }
    let (requests, retry_later) = runner.node.counts();
    info!(requests, retry_later, "requests to the node");
    result?;
    if let Some(ended) = claims_ended {
        bail!("the MONKEY claims task ended: {ended:?}");
    }
    if let Some(e) = failure {
        return Err(e.context("the console stopped"));
    }
    let stopped = runner.fleet.accounts.iter().filter(|a| a.failed.is_some()).count();
    anyhow::ensure!(
        failed.is_empty() && stopped == 0,
        "{} accounts could not start and {stopped} stopped trading",
        failed.len()
    );
    Ok(())
}

/// Stops the accounts as `Runner::stop` does, while the console is still watched: an answer another
/// key signed that the console reads meanwhile ends the stop before it sends anything more. Any other
/// failure of the console is kept, and the stop goes on.
/// Error cases:
/// - see `Runner::stop`
/// - the console read an answer another key signed
async fn stop_watching(
    runner: &mut Runner,
    stream: &mut crate::stream::Stream,
    failed_console: &mut oneshot::Receiver<anyhow::Error>,
    failure: &mut Option<anyhow::Error>,
) -> Result<(), Halt> {
    let stopping = runner.stop(stream);
    tokio::pin!(stopping);
    let mut watching = failure.is_none();
    loop {
        tokio::select! {
            result = &mut stopping => return result,
            Ok(e) = &mut *failed_console, if watching => {
                watching = false;
                let signer = signed_elsewhere(&e);
                if !signer {
                    error!(alert = "console_failed", "the console stopped during the stop: {e:#}");
                }
                *failure = Some(e);
                if signer {
                    return Err(Halt::WrongSigner);
                }
            }
        }
    }
}

/// Whether the console stopped on an answer another key than the signer signed.
fn signed_elsewhere(e: &anyhow::Error) -> bool {
    matches!(e.downcast_ref::<CallError>(), Some(CallError::WrongSigner { .. }))
}

/// Checks the bot can follow `spec`'s index from the log: a market quoted in USD and priced from spot
/// quotes, which may keep trading hours.
/// Error cases:
/// - another quote currency, or futures contracts
pub(crate) fn supported(spec: &MarketSpec) -> anyhow::Result<()> {
    anyhow::ensure!(
        spec.quote_currency == QuoteCurrency::Usd && spec.contracts.is_empty(),
        "{} is not a USD market priced from spot quotes, the only markets the bot trades",
        spec.symbol
    );
    Ok(())
}

/// Checks a bar length can trade in the market's weekly hours: some bar is decided while the market is
/// open and is no gap, in every clock regime a search from `from` reaches. A bar of a day, or any other that always spans a closure, would be
/// refused for ever.
/// Error cases:
/// - a market with trading hours that hold no such bar of this length
pub(crate) fn bar_fits(spec: &MarketSpec, bar_ms: Millis, from: Millis) -> anyhow::Result<()> {
    anyhow::ensure!(
        trader_core::hours::bar_can_trade(&spec.schedule, bar_ms, from),
        "{} has trading hours that hold no bar of {} minutes, so the bot would never trade it",
        spec.symbol,
        bar_ms / 60_000
    );
    Ok(())
}
