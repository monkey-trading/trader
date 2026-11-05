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

/// Whether bars of `length` ms can trade in `schedule`'s hours: some epoch-aligned bar and the one before
/// it lie inside open time, and the market is still open when the bar completes, so the bar is no gap
/// and is decided while the market is open. Four weeks from 2026-01-05 are searched, for the weekly
/// windows, and dated closures that fall in them can only hide a fit.
pub fn bar_can_trade(schedule: &Schedule, length: Millis) -> bool {
    const MONDAY_2026_01_05: Millis = 1_767_571_200_000;
    const SEARCHED: Millis = 4 * 7 * 24 * 60 * 60 * 1000;
    if length <= 0 {
        return false;
    }
    let first = MONDAY_2026_01_05.div_euclid(length) * length + length;
    (first..MONDAY_2026_01_05 + SEARCHED)
        .step_by(usize::try_from(length).unwrap_or(usize::MAX))
        .any(|start| schedule.open_throughout(start - length, start + length))
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
