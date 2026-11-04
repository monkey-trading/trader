//! The MACD signal-line crossover: long while the MACD line is above its signal line.

use std::cmp::Ordering;

use engine_core::Fixed;

use crate::ema::Ema;
use crate::sma::MAX_PERIOD;
use crate::strategy::{ParamsError, Target};

/// The MACD line is the fast exponential average of bar closes less the slow one, and the signal
/// line is an exponential average of the MACD line. The target is long while the line is above the
/// signal, short while it is below, and flat while they are equal.
#[derive(Clone, Debug)]
pub struct Macd {
    fast: Ema,
    slow: Ema,
    signal: Ema,
}

impl Macd {
    /// A MACD of the `fast` and `slow` periods with a signal line of `signal` periods.
    /// Error cases:
    /// - `fast` is zero, or not below `slow`
    /// - `slow` is above `MAX_PERIOD`
    /// - `signal` is zero or above `MAX_PERIOD`
    pub fn new(fast: usize, slow: usize, signal: usize) -> Result<Self, ParamsError> {
        if fast == 0 || fast >= slow || slow > MAX_PERIOD {
            return Err(ParamsError::Periods);
        }
        if signal == 0 || signal > MAX_PERIOD {
            return Err(ParamsError::Period);
        }
        Ok(Self { fast: Ema::new(fast), slow: Ema::new(slow), signal: Ema::new(signal) })
    }

    /// Takes a completed bar's close, and returns the target once the signal line exists.
    pub fn on_close(&mut self, close: Fixed) -> Option<Target> {
        self.fast.push(close);
        self.slow.push(close);
        let line = self.fast.value()? - self.slow.value()?;
        self.signal.push(line);
        Some(match line.cmp(&self.signal.value()?) {
            Ordering::Greater => Target::Long,
            Ordering::Less => Target::Short,
            Ordering::Equal => Target::Flat,
        })
    }

    /// The MACD line and its signal, once each exists. The line is the one the last close made.
    pub fn lines(&self) -> (Option<Fixed>, Option<Fixed>) {
        (self.fast.value().zip(self.slow.value()).map(|(fast, slow)| fast - slow), self.signal.value())
    }
}
