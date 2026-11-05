//! Many accounts trading on one log. At startup the bot unlocks each account's key and reads the
//! markets and the accounts from the node; from then on it follows the public log once, over one
//! stream connection: each `Prices` command moves its market's index mark, each completed bar goes
//! to the strategies that read it, and the accounts' fills move their positions. A target an
//! account's position is not at queues a signed market order (`run`), unless the bar is history the
//! stream is replaying, or an earlier order of the account could still enter the log. An account
//! that fails stops alone; the others carry on.

mod fills;
pub mod fleet;
mod orders;
mod run;
mod startup;
pub use startup::stop_signal;
mod stop;
mod warm;

use engine_core::market::{MarketSpec, QuoteCurrency};
use engine_core::session::Schedule;
use tracing::{error, info};

use crate::config::Config;
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
pub async fn addresses(
    cfg: &Config,
    env: impl Fn(&str) -> Option<String>,
) -> anyhow::Result<Vec<engine_core::types::AccountId>> {
    let mut addresses = Vec::with_capacity(cfg.plans.len());
    for plan in &cfg.plans {
        addresses.push(crate::keys::AccountKey::load(&plan.key, &env).await?.address());
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
pub async fn run(
    cfg: Config,
    env: impl Fn(&str) -> Option<String> + Sync,
    stop: impl Future<Output = ()>,
) -> anyhow::Result<()> {
    let started = match startup::start(&cfg, &env).await {
        Ok(started) => started,
        Err(e) => {
            if let Some(CallError::WrongSigner { .. }) = e.downcast_ref::<CallError>() {
                error!(alert = "node_signer_mismatch", "{e:#}");
            }
            return Err(e);
        }
    };
    let startup::Started { fleet, node, mut stream, domain, failed } = started;
    let cap = cfg.node.limits.connections * 4;
    let mut runner = Runner::new(fleet, node, domain, cap);
    let result = match runner.follow(&mut stream, stop).await {
        Ok(()) => runner.stop(&mut stream).await,
        // What the bot saw is not what it trades on: it changes nothing, and leaves the accounts to
        // their owners.
        Err(Halt::WrongSigner) => Err(Halt::WrongSigner),
        Err(halt) => {
            error!(alert = "bot_halted", "{halt}; the accounts are left as they are");
            Err(halt)
        }
    };
    if let Err(Halt::WrongSigner) = &result {
        error!(alert = "node_signer_mismatch", "{}", Halt::WrongSigner);
    }
    let (requests, retry_later) = runner.node.counts();
    info!(requests, retry_later, "requests to the node");
    result?;
    let stopped = runner.fleet.accounts.iter().filter(|a| a.failed.is_some()).count();
    anyhow::ensure!(
        failed.is_empty() && stopped == 0,
        "{} accounts could not start and {stopped} stopped trading",
        failed.len()
    );
    Ok(())
}

/// Checks the bot can follow `spec`'s index from the log: a market quoted in USD, priced from spot
/// quotes, and never closed.
/// Error cases:
/// - another quote currency, futures contracts, or trading hours
pub(crate) fn supported(spec: &MarketSpec) -> anyhow::Result<()> {
    anyhow::ensure!(
        spec.quote_currency == QuoteCurrency::Usd && spec.contracts.is_empty() && spec.schedule == Schedule::AlwaysOpen,
        "{} is not a USD market priced from spot quotes that never closes, the only markets the bot trades",
        spec.symbol
    );
    Ok(())
}
