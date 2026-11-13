//! The markets' marks a stop quotes its closes at, read once for every account's stop.

use std::collections::BTreeMap;

use engine_core::Fixed;
use engine_core::types::MarketId;
use tokio::sync::Mutex;
use tokio::time::Instant;
use tracing::warn;

use crate::bot::stop::{Failed, QUOTE_AGE};
use crate::node::{CallError, Node};

/// Each traded market's mark, where there is one.
pub(super) type Marks = BTreeMap<MarketId, Option<Fixed>>;

/// The markets' marks as the node gives them, read again when the last read is older than
/// `QUOTE_AGE`, shared by every account's stop.
pub(super) struct Quotes {
    /// The log's last mark of each traded market, which stands in for one the node does not give.
    pub(super) last: Marks,
    pub(super) read: Mutex<Option<(Instant, Marks)>>,
}

impl Quotes {
    /// The marks of the traded markets as the node gives them, and when they were read: again when the
    /// last read is older than `QUOTE_AGE`; the log's last mark stands in for one the node does not give.
    /// Error cases:
    /// - an answer signed by another key than the signer
    pub(super) async fn get(&self, node: &Node) -> Result<(Marks, Instant), Failed> {
        let mut held = self.read.lock().await;
        if let Some((at, marks)) = held.as_ref().filter(|(at, _)| at.elapsed() < QUOTE_AGE) {
            return Ok((marks.clone(), *at));
        }
        let marks = match node.markets().await {
            Ok(listed) => self
                .last
                .iter()
                .map(|(id, mark)| (*id, listed.body.iter().find(|m| m.spec.id == *id).and_then(|m| m.mark).or(*mark)))
                .collect(),
            Err(CallError::WrongSigner { .. }) => return Err(Failed::WrongSigner),
            Err(e) => {
                warn!("the markets' marks were not read: {e}");
                self.last.clone()
            }
        };
        let read_at = Instant::now();
        *held = Some((read_at, marks.clone()));
        Ok((marks, read_at))
    }
}
