//! What the bot keeps in a file for each account so a restart does not hand an account the loss limit
//! stopped a new day, or a position it held a shorter hold: the day's loss, when each position
//! opened and on which side and at what entry price, and the markets that split.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Context;
use engine_core::Fixed;
use engine_core::types::{AccountId, MarketId, Millis};
use serde::{Deserialize, Serialize};
use trader_core::risk::DayState;

/// A position the opening times belong to: its side, and its entry price (another entry is another
/// position, whatever its side).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedPosition {
    /// 1 long, -1 short, 0 none.
    pub side: i64,
    pub entry: Fixed,
}

/// What the bot keeps across a restart.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    pub day: Option<DayState>,
    pub opened: BTreeMap<MarketId, Millis>,
    /// The position each of `opened` belongs to.
    #[serde(default)]
    pub positions: BTreeMap<MarketId, SavedPosition>,
    /// When each market split, if the bot halted at one: prices before it are in other units.
    #[serde(default)]
    pub splits: BTreeMap<MarketId, Millis>,
}

/// The file holding one account's saved state.
#[derive(Clone, Debug)]
pub struct StateFile {
    path: PathBuf,
}

impl StateFile {
    /// The file for `account` in `dir`.
    pub fn new(dir: &Path, account: AccountId) -> Self {
        Self { path: dir.join(format!("{account}.json")) }
    }

    /// The saved state, or the empty one when no file exists.
    /// Error cases:
    /// - the file cannot be read, or is not a saved state: the bot cannot tell whether the limit stopped
    ///   the account today, so it does not trade the account until someone repairs or removes the file
    pub async fn load(&self) -> anyhow::Result<Saved> {
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
    pub async fn save(&self, saved: Saved) -> anyhow::Result<()> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let dir = path.parent().context("the state file has a directory")?;
            std::fs::create_dir_all(dir)?;
            let temporary = path.with_extension("json.tmp");
            let mut file = std::fs::File::create(&temporary)?;
            file.write_all(serde_json::to_string(&saved)?.as_bytes())?;
            file.sync_all()?;
            std::fs::rename(&temporary, &path)?;
            // The rename is in the directory, which is synced too, or a crash can bring the old file back.
            std::fs::File::open(dir)?.sync_all()?;
            anyhow::Ok(())
        })
        .await?
        .with_context(|| format!("writing {}", self.path.display()))
    }
}
