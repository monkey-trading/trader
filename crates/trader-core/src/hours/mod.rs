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

/// Whether bars of `length` ms can trade in `schedule`'s hours, as the bot decides a bar: some
/// epoch-aligned bar is inside open time from its start to the end of its window, where it completes,
/// and the bar before it saw open time, so it is no gap. Only the weekly windows count: dated closures,
/// closed dates and special sessions are left out, so the answer does not depend on when it is asked.
/// The bar grid lines up with the week again after `length / gcd(length, week)` weeks, and that many are
/// searched. Four such spans, a quarter apart, are searched, as the clocks of a market's time zone
/// move for longer than half a year at a time, and each must hold one.
pub fn bar_can_trade(schedule: &Schedule, length: Millis, from: Millis) -> bool {
    const WEEK: Millis = 7 * 24 * 60 * 60 * 1000;
    const MINUTE: Millis = 60_000;
    if length <= 0 {
        return false;
    }
    let schedule = weekly_windows(schedule);
    let step = usize::try_from(length).unwrap_or(usize::MAX);
    let span = (length / gcd(length, WEEK)).saturating_mul(WEEK);
    let fits = |week: Millis| {
        let first = week.div_euclid(length) * length + length;
        (first..week.saturating_add(span)).step_by(step).any(|start| {
            schedule.open_throughout(start, start + length)
                && (start - length..start).step_by(MINUTE as usize).any(|at| schedule.is_open(at))
        })
    };
    (0..4).all(|quarter| fits(from + quarter * 13 * WEEK))
}

/// `schedule` without its dated closures, closed dates and special sessions.
fn weekly_windows(schedule: &Schedule) -> Schedule {
    match schedule {
        Schedule::Weekly { tz, windows, .. } => Schedule::Weekly {
            tz: *tz,
            windows: windows.clone(),
            closed_dates: Vec::new(),
            closures: Vec::new(),
            special_sessions: Vec::new(),
        },
        Schedule::AlwaysOpen => Schedule::AlwaysOpen,
    }
}

fn gcd(a: Millis, b: Millis) -> Millis {
    if b == 0 { a } else { gcd(b, a % b) }
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
