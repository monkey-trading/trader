//! What the bot keeps in a file so a restart does not hand an account the loss limit stopped a new
//! day, or a position it held a shorter hold, and the bot's reading of equity against that limit.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;

use anyhow::Context;
use engine_core::Fixed;
use engine_core::market::MarketSpec;
use engine_core::types::{AccountId, MarketId, Millis};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use trader_core::controls::Account;
use trader_core::margin::Holding;
use trader_core::risk::{DayState, DayStatus};

use crate::bot::{Bot, Halt, REPLAY_LAG_MS};

/// How long, in sequenced time, after a try at closing the position the daily loss limit tries again.
const FLATTEN_RETRY_MS: Millis = 5_000;

/// What the bot keeps across a restart: the day's loss, when its position opened and on which side, and
/// when a split halted it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Saved {
    pub(super) day: Option<DayState>,
    pub(super) opened: BTreeMap<MarketId, Millis>,
    /// The side of the position `opened` belongs to: 1 long, -1 short, 0 none.
    #[serde(default)]
    pub(super) side: i64,
    /// When the market split, if the bot halted at one: prices before it are in other units.
    #[serde(default)]
    pub(super) split_at: Option<Millis>,
}

/// The file holding one account's saved state.
#[derive(Clone, Debug)]
pub(super) struct StateFile {
    path: PathBuf,
}

impl StateFile {
    /// The file for `account` in `dir`.
    pub(super) fn new(dir: &std::path::Path, account: AccountId) -> Self {
        Self { path: dir.join(format!("{account}.json")) }
    }

    /// The saved state, or the empty one when no file exists.
    /// Error cases:
    /// - the file cannot be read, or is not a saved state: the bot cannot tell whether the limit stopped
    ///   the account today, so it does not trade until someone repairs or removes the file
    pub(super) async fn load(&self) -> anyhow::Result<Saved> {
        let path = self.path.clone();
        let read = tokio::task::spawn_blocking(move || std::fs::read_to_string(path)).await?;
        match read {
            Ok(text) => serde_json::from_str(&text).with_context(|| format!("parsing {}", self.path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Saved::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", self.path.display())),
        }
    }

    /// Writes `saved` whole: to a new file, flushed, and renamed over the old one.
    /// Error cases:
    /// - the directory or file cannot be written
    pub(super) async fn save(&self, saved: Saved) -> anyhow::Result<()> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let dir = path.parent().context("the state file has a directory")?;
            std::fs::create_dir_all(dir)?;
            let temporary = path.with_extension("json.tmp");
            let mut file = std::fs::File::create(&temporary)?;
            file.write_all(serde_json::to_string(&saved)?.as_bytes())?;
            file.sync_all()?;
            std::fs::rename(&temporary, &path)?;
            anyhow::Ok(())
        })
        .await?
        .with_context(|| format!("writing {}", self.path.display()))
    }
}

impl Bot {
    /// Reads the account's equity at the sequenced time `ts`, as the mark moves or a fill lands, and
    /// keeps the day's state and the positions' opening times in their file. A reading is taken only
    /// while the account's balance and entry are those of the position the log gives.
    pub(super) async fn check_day(&mut self, ts: Millis) -> Result<(), Halt> {
        self.save_state().await;
        let Some(mark) = self.mark.filter(|_| !self.account_behind) else { return Ok(()) };
        let holding = holding(&self.spec, self.market, self.position, self.entry, mark);
        let holdings = holding.as_slice();
        let account = Account { balance: self.balance, holdings };
        let Some(status) = self.controls.observe(ts, &account) else { return Ok(()) };
        match status {
            DayStatus::Stopped { newly } => {
                if newly || !self.loss_alert {
                    let state = self.controls.day_state();
                    warn!(
                        alert = "daily_loss_limit",
                        ?state,
                        "the daily loss limit is reached: flattening and stopping until the next UTC day"
                    );
                }
                self.loss_alert = true;
            }
            DayStatus::Trading if self.loss_alert => {
                self.loss_alert = false;
                info!(alert_resolved = "daily_loss_limit", "a new UTC day: trading again");
            }
            DayStatus::Trading => {}
        }
        self.save_state().await;
        Ok(())
    }

    /// While the daily loss limit has stopped the account and it still holds a position, closes it,
    /// again every `FLATTEN_RETRY_MS` of sequenced time, on any command: a close the engine declined,
    /// or one held back by an order in flight, does not wait for a move of the mark or the next bar.
    /// Error cases:
    /// - an answer signed by another key than the signer
    pub(super) async fn retry_flatten(&mut self, ts: Millis, lag: Millis) -> Result<(), Halt> {
        let due = self.controls.is_stopped() && !self.position.is_zero() && ts >= self.flatten_after;
        if due && lag <= REPLAY_LAG_MS && !self.account_behind {
            self.flatten_after = ts.saturating_add(FLATTEN_RETRY_MS);
            self.flatten(ts).await?;
        }
        Ok(())
    }

    /// Writes the saved state to its file when it changed. A failed write is an alert, and the state
    /// is written again at the next change.
    pub(super) async fn save_state(&mut self) {
        let (day, side) = (self.controls.day_state(), self.position.signum());
        let saved = &self.saved_state;
        if day == saved.day
            && side == saved.side
            && self.split_at == saved.split_at
            && self.controls.opened() == &saved.opened
        {
            return;
        }
        let state = Saved { day, opened: self.controls.opened().clone(), side, split_at: self.split_at };
        match self.state_file.save(state.clone()).await {
            Ok(()) => {
                self.saved_state = state;
                if self.state_unsaved {
                    self.state_unsaved = false;
                    info!(alert_resolved = "day_state_unsaved", "the saved state is written");
                }
            }
            Err(e) => {
                if !self.state_unsaved {
                    self.state_unsaved = true;
                    warn!(alert = "day_state_unsaved", "{e:#}: a restart would not know the day's loss or the holds");
                }
            }
        }
    }
}

/// The account's position of `qty` in `market` at `mark`, if it holds one.
pub(super) fn holding(
    spec: &MarketSpec,
    market: MarketId,
    qty: Fixed,
    entry: Fixed,
    mark: Fixed,
) -> Option<Holding<'_>> {
    (!qty.is_zero()).then_some(Holding { market, qty, entry, mark, spec })
}
