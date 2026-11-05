//! The EMA crossover: long while the fast exponential average is above the slow one.

use std::cmp::Ordering;

use engine_core::Fixed;

use crate::ema::Ema;
use crate::sma::MAX_PERIOD;
use crate::strategy::{ParamsError, Target};

/// Long while the fast average of bar closes is above the slow one, short while it is below, and
/// flat while they are equal, the averages compared at nine decimals.
#[derive(Clone, Debug)]
pub struct EmaCrossover {
    fast: Ema,
    slow: Ema,
}

impl EmaCrossover {
    /// A crossover of the `fast` and `slow` period averages.
    /// Error cases:
    /// - `fast` is zero, or not below `slow`
    /// - `slow` is above `MAX_PERIOD`
    pub fn new(fast: usize, slow: usize) -> Result<Self, ParamsError> {
        if fast == 0 || fast >= slow || slow > MAX_PERIOD {
            return Err(ParamsError::Periods);
        }
        Ok(Self { fast: Ema::new(fast), slow: Ema::new(slow) })
    }

    /// Takes a completed bar's close, and returns the target once the slow average exists.
    pub fn on_close(&mut self, close: Fixed) -> Option<Target> {
        self.fast.push(close);
        self.slow.push(close);
        Some(match self.fast.value()?.cmp(&self.slow.value()?) {
            Ordering::Greater => Target::Long,
            Ordering::Less => Target::Short,
            Ordering::Equal => Target::Flat,
        })
    }

    /// The fast and slow averages, once each exists.
    pub fn averages(&self) -> (Option<Fixed>, Option<Fixed>) {
        (self.fast.value(), self.slow.value())
    }
}
