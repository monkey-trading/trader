//! Market hours: whether a market is open, and whether a bar is the first after it was shut.

use engine_core::session::Schedule;
use engine_core::types::Millis;

/// The longest span between two bars that is checked minute by minute for a closure. A longer one is a
/// gap without the check: it holds a closure or a stretch with no price either way.
pub const MAX_SCAN_MS: Millis = 14 * 24 * 60 * 60 * 1000;

/// Whether the venue is open at `ts`.
pub fn is_open(schedule: &Schedule, ts: Millis) -> bool {
    schedule.is_open(ts)
}

/// Tells the first bar after a closure from the rest, from the starts of consecutive bars.
#[derive(Clone, Debug, Default)]
pub struct GapTracker {
    previous: Option<Millis>,
}

impl GapTracker {
    /// A tracker that has seen no bar.
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes the start of the next bar, which is `length` ms long and does not come before the last,
    /// and returns whether the market was shut at any time from the end of the bar before it to the
    /// end of this one: a closure is counted once, by the bar that spans it. The first bar seen is no
    /// gap.
    pub fn on_bar(&mut self, schedule: &Schedule, start: Millis, length: Millis) -> bool {
        let previous = self.previous.replace(start);
        let Some(previous) = previous.filter(|previous| *previous < start) else { return false };
        start - previous > MAX_SCAN_MS || !schedule.open_throughout(previous + length, start + length - 1)
    }
}

#[cfg(test)]
pub(crate) mod tests;
