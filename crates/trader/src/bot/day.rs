//! What the bot keeps in a file for each account so a restart does not hand an account the loss limit
//! stopped a new day, or a position it held a shorter hold: the day's loss, when each position
//! opened and on which side and at what entry price, and the markets that split.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

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
    /// The fields of a file written for one market, before accounts traded several: read, never written.
    #[serde(default, skip_serializing)]
    pub side: i64,
    #[serde(default, skip_serializing)]
    pub entry: Fixed,
    #[serde(default, skip_serializing)]
    pub split_at: Option<Millis>,
}

impl Saved {
    /// Moves a file written for one market to the per-market maps, for an account that trades `markets`.
    /// Error cases:
    /// - the file names no market (it holds no position) and the account trades several: which market
    ///   its split belongs to is not known, so the file is left for someone to remove
    pub fn migrate(&mut self, markets: &[MarketId]) -> anyhow::Result<()> {
        let (side, entry, split_at) =
            (std::mem::take(&mut self.side), std::mem::take(&mut self.entry), self.split_at.take());
        if side == 0 && split_at.is_none() {
            return Ok(());
        }
        // The file names its market by the opening time of its position, when it holds one.
        let mut named = self.opened.keys();
        let market = match (named.next(), named.next()) {
            (Some(&only), None) if markets.contains(&only) => only,
            // A position of a market the account no longer trades is not its own.
            (Some(_), None) => return Ok(()),
            _ => match markets {
                [only] => *only,
                _ => anyhow::bail!(
                    "the saved state was written for one market, and this account trades {}: move the file aside to start it afresh",
                    markets.len()
                ),
            },
        };
        if side != 0 {
            self.positions.entry(market).or_insert(SavedPosition { side, entry });
        }
        if let Some(at) = split_at {
            self.splits.entry(market).or_insert(at);
        }
        Ok(())
    }
}

/// The file holding one account's saved state.
#[derive(Clone, Debug)]
pub struct StateFile {
    path: PathBuf,
    /// Writes are numbered as they are asked for, and a write that an older one overtakes is skipped, so
    /// the file holds the newest snapshot asked for whatever order the writes run in.
    writes: Arc<(AtomicU64, Mutex<u64>)>,
}

impl StateFile {
    /// The file for `account` in `dir`.
    pub fn new(dir: &Path, account: AccountId) -> Self {
        Self { path: dir.join(format!("{account}.json")), writes: Arc::default() }
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
        let (writes, number) = (Arc::clone(&self.writes), self.writes.0.fetch_add(1, Ordering::SeqCst) + 1);
        tokio::task::spawn_blocking(move || {
            let mut newest = writes.1.lock().unwrap_or_else(|e| e.into_inner());
            if *newest > number {
                return anyhow::Ok(());
            }
            let dir = path.parent().context("the state file has a directory")?;
            std::fs::create_dir_all(dir)?;
            let temporary = path.with_extension("json.tmp");
            let mut file = std::fs::File::create(&temporary)?;
            file.write_all(serde_json::to_string(&saved)?.as_bytes())?;
            file.sync_all()?;
            std::fs::rename(&temporary, &path)?;
            // The rename is in the directory, which is synced too, or a crash can bring the old file back.
            std::fs::File::open(dir)?.sync_all()?;
            *newest = number;
            anyhow::Ok(())
        })
        .await?
        .with_context(|| format!("writing {}", self.path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy(opened: &[MarketId]) -> Saved {
        Saved {
            opened: opened.iter().map(|&m| (m, 5)).collect(),
            side: 1,
            entry: "100".parse().unwrap(),
            split_at: Some(9),
            ..Saved::default()
        }
    }

    #[test]
    fn a_one_market_file_moves_to_the_market_it_names() {
        // Named, and still traded beside another market.
        let mut saved = legacy(&[1]);
        saved.migrate(&[1, 2]).unwrap();
        assert_eq!(
            (saved.splits.keys().collect::<Vec<_>>(), saved.positions.keys().collect::<Vec<_>>()),
            (vec![&1], vec![&1])
        );
        // Named, and no longer traded: nothing of it is the account's.
        let mut saved = legacy(&[1]);
        saved.migrate(&[2]).unwrap();
        assert!(saved.splits.is_empty() && saved.positions.is_empty());
        // Unnamed, for the one market the account trades; refused for several.
        let mut saved = legacy(&[]);
        saved.migrate(&[3]).unwrap();
        assert_eq!(saved.splits.get(&3), Some(&9));
        assert!(legacy(&[]).migrate(&[1, 2]).is_err());
    }
}
