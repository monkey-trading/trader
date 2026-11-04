//! The day's loss, kept in a file so a restart does not hand an account the loss limit stopped a new
//! day, and the bot's reading of equity against that limit.

use std::io::Write;
use std::path::PathBuf;

use anyhow::Context;
use engine_core::Fixed;
use engine_core::market::MarketSpec;
use engine_core::types::{AccountId, MarketId, Millis};
use tracing::{info, warn};
use trader_core::controls::Account;
use trader_core::margin::Holding;
use trader_core::risk::{DayState, DayStatus};

use crate::bot::{Bot, Halt, REPLAY_LAG_MS};

/// How long, in sequenced time, after a try at closing the position the daily loss limit tries again.
const FLATTEN_RETRY_MS: Millis = 5_000;

/// The file holding one account's day state.
#[derive(Clone, Debug)]
pub(super) struct DayFile {
    path: PathBuf,
}

impl DayFile {
    /// The file for `account` in `dir`.
    pub(super) fn new(dir: &std::path::Path, account: AccountId) -> Self {
        Self { path: dir.join(format!("{account}.json")) }
    }

    /// The saved state, or `None` when no file exists.
    /// Error cases:
    /// - the file cannot be read, or is not a day state: the bot cannot tell whether the limit stopped
    ///   the account today, so it does not trade until someone repairs or removes the file
    pub(super) async fn load(&self) -> anyhow::Result<Option<DayState>> {
        let path = self.path.clone();
        let read = tokio::task::spawn_blocking(move || std::fs::read_to_string(path)).await?;
        match read {
            Ok(text) => {
                let state = serde_json::from_str(&text).with_context(|| format!("parsing {}", self.path.display()))?;
                Ok(Some(state))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading {}", self.path.display())),
        }
    }

    /// Writes `state` whole: to a new file, flushed, and renamed over the old one.
    /// Error cases:
    /// - the directory or file cannot be written
    pub(super) async fn save(&self, state: DayState) -> anyhow::Result<()> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let dir = path.parent().context("the state file has a directory")?;
            std::fs::create_dir_all(dir)?;
            let temporary = path.with_extension("json.tmp");
            let mut file = std::fs::File::create(&temporary)?;
            file.write_all(serde_json::to_string(&state)?.as_bytes())?;
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
    /// keeps the day's state in its file. The first reading past the daily loss limit flattens the
    /// account, `lag` ms after the command it follows.
    /// Error cases:
    /// - an answer signed by another key than the signer
    pub(super) async fn check_day(&mut self, ts: Millis, lag: Millis) -> Result<(), Halt> {
        let Some(mark) = self.mark else { return Ok(()) };
        let holding = holding(&self.spec, self.market, self.position, self.entry, mark);
        let holdings = holding.as_slice();
        let account = Account { balance: self.balance, holdings };
        let Some(status) = self.controls.observe(ts, &account) else { return Ok(()) };
        self.save_day().await;
        if status == (DayStatus::Stopped { newly: true }) {
            let state = self.controls.day_state();
            warn!(
                alert = "daily_loss_limit",
                ?state,
                "the daily loss limit is reached: flattening and stopping until the next UTC day"
            );
        }
        // While the account is stopped and still holds a position, closing it is tried again every
        // `FLATTEN_RETRY_MS` of sequenced time, so a close the engine declined, or one held back by an
        // order in flight, does not wait for the next bar.
        let stopped = matches!(status, DayStatus::Stopped { .. });
        if stopped && !self.position.is_zero() && ts >= self.flatten_after && lag <= REPLAY_LAG_MS {
            self.flatten_after = ts.saturating_add(FLATTEN_RETRY_MS);
            self.flatten(ts).await?;
        }
        Ok(())
    }

    /// Writes the day's state to its file when it changed. A failed write is an alert, and the state
    /// is written again at the next change.
    async fn save_day(&mut self) {
        let state = self.controls.day_state();
        if state == self.saved_day {
            return;
        }
        let Some(state) = state else { return };
        match self.day_file.save(state).await {
            Ok(()) => {
                self.saved_day = Some(state);
                if self.day_unsaved {
                    self.day_unsaved = false;
                    info!(alert_resolved = "day_state_unsaved", "the day's state is saved");
                }
            }
            Err(e) => {
                if !self.day_unsaved {
                    self.day_unsaved = true;
                    warn!(alert = "day_state_unsaved", "{e:#}: a restart would not know the day's loss");
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
