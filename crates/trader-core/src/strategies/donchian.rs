//! The Donchian channel breakout: buy a close above the highest of the last `entry` closes, sell
//! one below the lowest, and exit when the close crosses the other side of a shorter channel.

use engine_core::Fixed;

use crate::extremes::{Extreme, SlidingExtreme};
use crate::sma::MAX_PERIOD;
use crate::strategies::{Signals, next_target};
use crate::strategy::{ParamsError, Target};

/// Long on a close above the highest of the previous `entry` closes, short on one below their
/// lowest. A long goes flat when a close falls below the lowest of the previous `exit` closes, and
/// a short when one rises above their highest. Otherwise the target stays what it was.
#[derive(Clone, Debug)]
pub struct DonchianBreakout {
    entry_high: SlidingExtreme,
    entry_low: SlidingExtreme,
    exit_high: SlidingExtreme,
    exit_low: SlidingExtreme,
    target: Target,
}

impl DonchianBreakout {
    /// A breakout of the last `entry` closes that exits on the last `exit` closes.
    /// Error cases:
    /// - `entry` is zero or above `MAX_PERIOD`
    /// - `exit` is zero or above `entry`
    pub fn new(entry: usize, exit: usize) -> Result<Self, ParamsError> {
        if entry == 0 || entry > MAX_PERIOD || exit == 0 || exit > entry {
            return Err(ParamsError::ExitPeriod);
        }
        Ok(Self {
            entry_high: SlidingExtreme::new(entry, Extreme::Highest),
            entry_low: SlidingExtreme::new(entry, Extreme::Lowest),
            exit_high: SlidingExtreme::new(exit, Extreme::Highest),
            exit_low: SlidingExtreme::new(exit, Extreme::Lowest),
            target: Target::Flat,
        })
    }

    /// Takes a completed bar's close, and returns the target once `entry` closes came before it.
    pub fn on_close(&mut self, close: Fixed) -> Option<Target> {
        // The channels hold the closes before this one, so a close is compared with them first.
        let channels =
            self.entry_high.value().zip(self.entry_low.value()).zip(self.exit_high.value()).zip(self.exit_low.value());
        self.entry_high.push(close);
        self.entry_low.push(close);
        self.exit_high.push(close);
        self.exit_low.push(close);
        let (((entry_high, entry_low), exit_high), exit_low) = channels?;
        self.target = next_target(
            self.target,
            Signals {
                enter_long: close > entry_high,
                enter_short: close < entry_low,
                exit_long: close < exit_low,
                exit_short: close > exit_high,
            },
        );
        Some(self.target)
    }
}
