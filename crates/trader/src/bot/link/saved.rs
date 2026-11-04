//! The controls the console set for an account, in a file of its own under the console's data
//! directory, so a restart keeps them: an account the console stopped starts stopped.

use std::path::{Path, PathBuf};

use anyhow::Context;
use engine_core::types::AccountId;
use serde::{Deserialize, Serialize};
use tokio::task::spawn_blocking;
use tracing::{info, warn};

use crate::bot::link::SMA_CROSSOVER;
use crate::config::{Config, SmaParams};
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
    pub params: SmaParams,
    /// The config's own periods when these were saved: an edit of the config's periods replaces them.
    pub config_params: SmaParams,
}

impl Saved {
    /// The config's controls: the account and its strategy running, on the config's periods.
    pub fn from_config(cfg: &Config) -> Self {
        let params = cfg.strategy.params();
        Self {
            running: true,
            strategies: vec![SavedStrategy {
                market: cfg.strategy.market.clone(),
                kind: SMA_CROSSOVER.into(),
                running: true,
                params,
                config_params: params,
            }],
        }
    }

    /// The controls saved at `path`, over the config's. The account's and the strategy's stops
    /// hold. Saved periods hold while the config's own are still the ones they replaced and its
    /// bounds admit them. A strategy saved for another market or kind is not the config's.
    /// Error cases:
    /// - the file cannot be read, or is not controls of this shape
    pub async fn load(path: PathBuf, cfg: &Config) -> anyhow::Result<Self> {
        let shown = path.display().to_string();
        let text = spawn_blocking(move || match std::fs::read_to_string(&path) {
            Ok(text) => Ok(Some(text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        })
        .await
        .context("the saved controls' task")?
        .with_context(|| format!("reading {shown}"))?;
        let mut controls = Self::from_config(cfg);
        let Some(text) = text else { return Ok(controls) };
        let saved: Saved = serde_json::from_str(&text).with_context(|| {
            format!("{shown} holds no controls the bot can read; delete it to start from the config")
        })?;
        controls.running = saved.running;
        let strategy = &mut controls.strategies[0];
        match saved.strategies.into_iter().find(|s| s.market == strategy.market && s.kind == strategy.kind) {
            None => info!(file = shown, "no saved controls for the config's strategy"),
            Some(saved) => {
                strategy.running = saved.running;
                let admitted = cfg.strategy.bounds.is_some_and(|bounds| bounds.admit(saved.params));
                if saved.config_params == strategy.config_params && admitted {
                    strategy.params = saved.params;
                } else if saved.params != strategy.params {
                    warn!(file = shown, saved = ?saved.params, "the config's periods replace the ones the console set");
                }
            }
        }
        if !controls.trading() {
            warn!(file = shown, "the console stopped this account or its strategy; it starts stopped");
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

    /// Whether the account and its strategy may place orders.
    pub fn trading(&self) -> bool {
        self.running && self.strategies.iter().all(|strategy| strategy.running)
    }
}

/// Where `address`'s controls are saved under `data_dir`.
pub(super) fn path(data_dir: &Path, address: AccountId) -> PathBuf {
    data_dir.join("accounts").join(format!("{address}.json"))
}
