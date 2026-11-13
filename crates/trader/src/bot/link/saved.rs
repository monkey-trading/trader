//! The controls the console set for an account, in a file of its own under the console's data
//! directory, so a restart keeps them: an account the console stopped starts stopped.

use std::path::{Path, PathBuf};

use anyhow::Context;
use engine_core::types::AccountId;
use serde::{Deserialize, Serialize};
use tokio::task::spawn_blocking;
use tracing::{info, warn};

use serde_json::Value;
use trader_core::strategies::StrategyParams;

use crate::bot::link::kind_of;
use crate::config::{AccountPlan, bounds};
use crate::files::write_private;

/// An account's controls.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Saved {
    /// Whether the account's strategies may place orders.
    pub running: bool,
    pub strategies: Vec<SavedStrategy>,
}

/// One strategy's controls.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedStrategy {
    pub market: String,
    pub kind: String,
    pub running: bool,
    pub params: StrategyParams,
    /// The config's own parameters when these were saved: an edit of the config's replaces them.
    pub config_params: StrategyParams,
}

impl Saved {
    /// The config's controls for `plan`'s account: it and each of its strategies running, on the
    /// config's parameters, one entry per strategy in the plan's order.
    pub fn from_plan(plan: &AccountPlan) -> Self {
        let strategies = plan
            .strategies
            .iter()
            .map(|strategy| SavedStrategy {
                market: strategy.market.clone(),
                kind: kind_of(&strategy.params),
                running: true,
                params: strategy.params.clone(),
                config_params: strategy.params.clone(),
            })
            .collect();
        Self { running: true, strategies }
    }

    /// The controls saved at `path` over the config's for `plan`'s account, one entry per strategy in
    /// the plan's order. The account's and each strategy's stops hold. Saved parameters hold while the
    /// config's own are still the ones they replaced and its bounds admit them. A strategy saved for
    /// a market or kind the plan does not have is dropped.
    /// Error cases:
    /// - the file cannot be read, or is not controls of this shape
    pub async fn load(path: PathBuf, plan: &AccountPlan) -> anyhow::Result<Self> {
        let shown = path.display().to_string();
        let text = spawn_blocking(move || match std::fs::read_to_string(&path) {
            Ok(text) => Ok(Some(text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        })
        .await
        .context("the saved controls' task")?
        .with_context(|| format!("reading {shown}"))?;
        let mut controls = Self::from_plan(plan);
        let Some(text) = text else { return Ok(controls) };
        let saved: Saved = serde_json::from_str(&text).with_context(|| {
            format!("{shown} holds no controls the bot can read; delete it to start from the config")
        })?;
        controls.running = saved.running;
        for (strategy, config) in controls.strategies.iter_mut().zip(&plan.strategies) {
            let Some(saved) = saved.strategies.iter().find(|s| s.market == strategy.market && s.kind == strategy.kind)
            else {
                info!(file = shown, market = strategy.market, "no saved controls for this strategy");
                continue;
            };
            strategy.running = saved.running;
            let admitted = saved.params == strategy.params
                || config.bounds.as_ref().is_some_and(|limits| {
                    bounds::admit(&strategy.config_params, &Value::Object(bounds::to_json(&saved.params)), limits)
                        .is_ok()
                });
            if saved.config_params == strategy.config_params && admitted {
                strategy.params = saved.params.clone();
            } else if saved.params != strategy.params {
                warn!(file = shown, market = strategy.market, saved = ?saved.params, "the config's parameters replace the ones the console set");
            }
        }
        if !controls.trading() {
            warn!(file = shown, "the console stopped this account or a strategy of it; they start stopped");
        }
        Ok(controls)
    }

    /// Writes the controls to `path`.
    /// Error cases:
    /// - see `write_private`
    pub async fn save(&self, path: PathBuf) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec_pretty(self).context("encoding the controls")?;
        spawn_blocking(move || write_private(&path, &bytes)).await.context("the saved controls' task")?
    }

    /// Whether the account and every strategy of it may place orders.
    pub fn trading(&self) -> bool {
        self.running && self.strategies.iter().all(|strategy| strategy.running)
    }
}

/// Where `address`'s controls are saved under `data_dir`.
pub fn path(data_dir: &Path, address: AccountId) -> PathBuf {
    data_dir.join("accounts").join(format!("{address}.json"))
}
