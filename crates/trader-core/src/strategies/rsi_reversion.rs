//! RSI reversion: buy when the index is oversold, sell when it is overbought, and go flat when it
//! returns to the exit level.

use std::cmp::Ordering;

use engine_core::Fixed;

use crate::rsi::Rsi;
use crate::sma::MAX_PERIOD;
use crate::strategies::{Signals, next_target};
use crate::strategy::{ParamsError, Target};

/// Long once the index is at or below `oversold`, short once it is at or above `overbought`, and
/// flat again when a long's index reaches `exit` or a short's falls to it. Between the levels the
/// target stays what it was.
#[derive(Clone, Debug)]
pub struct RsiReversion {
    rsi: Rsi,
    oversold: Fixed,
    exit: Fixed,
    overbought: Fixed,
    target: Target,
}

impl RsiReversion {
    /// A reversion over `period` changes with the three levels, in index points.
    /// Error cases:
    /// - `period` is zero or above `MAX_PERIOD`
    /// - the levels are not `0 < oversold <= exit <= overbought < 100`
    pub fn new(period: usize, oversold: Fixed, exit: Fixed, overbought: Fixed) -> Result<Self, ParamsError> {
        if period == 0 || period > MAX_PERIOD {
            return Err(ParamsError::Period);
        }
        let hundred = Fixed::from_int(100);
        if !(Fixed::ZERO < oversold && oversold <= exit && exit <= overbought && overbought < hundred) {
            return Err(ParamsError::Levels);
        }
        Ok(Self { rsi: Rsi::new(period), oversold, exit, overbought, target: Target::Flat })
    }

    /// Takes a completed bar's close, and returns the target once the index exists.
    pub fn on_close(&mut self, close: Fixed) -> Option<Target> {
        self.rsi.push(close);
        let against = |level| self.rsi.compare(level);
        let (low, mid, high) = (against(self.oversold)?, against(self.exit)?, against(self.overbought)?);
        self.target = next_target(
            self.target,
            Signals {
                enter_long: low != Ordering::Greater,
                enter_short: high != Ordering::Less,
                exit_long: mid != Ordering::Less,
                exit_short: mid != Ordering::Greater,
            },
        );
        Some(self.target)
    }

    /// The index, once it exists.
    pub fn index(&self) -> Option<Fixed> {
        self.rsi.value()
    }
}
