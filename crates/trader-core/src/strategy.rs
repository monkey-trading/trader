//! The SMA crossover strategy and the order that moves a position to its target.

use engine_core::Fixed;
use engine_core::types::Side;

use crate::sma::{MAX_PERIOD, Sma};

/// The position a strategy wants in its market.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Long,
    Short,
    Flat,
}

/// Why a strategy's parameters are refused.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ParamsError {
    #[error("the fast period must be at least 1 and below the slow period, which is at most {MAX_PERIOD}")]
    Periods,
}

/// Long while the fast average of bar closes is above the slow one, short while it is below, and
/// flat while they are equal.
#[derive(Clone, Debug)]
pub struct SmaCrossover {
    fast: Sma,
    slow: Sma,
}

/// An order that moves a position toward its target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OrderIntent {
    pub side: Side,
    pub qty: Fixed,
    /// The order only shrinks the position, never opens or flips it.
    pub reduce_only: bool,
}

impl SmaCrossover {
    /// A crossover of the `fast` and `slow` period averages.
    /// Error cases:
    /// - `fast` is zero, or not below `slow`
    /// - `slow` is above `MAX_PERIOD`
    pub fn new(fast: usize, slow: usize) -> Result<Self, ParamsError> {
        if fast == 0 || fast >= slow || slow > MAX_PERIOD {
            return Err(ParamsError::Periods);
        }
        Ok(Self { fast: Sma::new(fast), slow: Sma::new(slow) })
    }

    /// Takes a completed bar's close, and returns the target once the slow average is full.
    pub fn on_close(&mut self, close: Fixed) -> Option<Target> {
        self.fast.push(close);
        self.slow.push(close);
        let (fast, slow) = (self.fast.sum()?, self.slow.sum()?);
        // Compares fast / fast_period with slow / slow_period exactly, as cross products: each
        // period is at most `MAX_PERIOD`, so neither product leaves `i128`.
        let fast_scaled = fast.raw() * self.slow.period() as i128;
        let slow_scaled = slow.raw() * self.fast.period() as i128;
        Some(match fast_scaled.cmp(&slow_scaled) {
            std::cmp::Ordering::Greater => Target::Long,
            std::cmp::Ordering::Less => Target::Short,
            std::cmp::Ordering::Equal => Target::Flat,
        })
    }

    /// The fast and slow averages, truncated toward zero, once each is full.
    pub fn averages(&self) -> (Option<Fixed>, Option<Fixed>) {
        (self.fast.value(), self.slow.value())
    }
}

/// The order that takes `position` to `target`, where a long or short target is `size` units:
/// `None` when the position is already there.
pub fn order_for(target: Target, size: Fixed, position: Fixed) -> Option<OrderIntent> {
    let wanted = match target {
        Target::Long => size,
        Target::Short => -size,
        Target::Flat => Fixed::ZERO,
    };
    let delta = wanted - position;
    if delta.is_zero() {
        return None;
    }
    let side = if delta.is_positive() { Side::Buy } else { Side::Sell };
    // Shrinking toward zero on the same side, or to zero itself, never opens or flips anything.
    let reduce_only = wanted.signum() * position.signum() >= 0 && wanted.abs() < position.abs();
    Some(OrderIntent { side, qty: delta.abs(), reduce_only })
}
