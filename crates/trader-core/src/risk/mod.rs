//! Per-account risk limits: how many positions, how much notional, how much may be lost in a day,
//! how much margin to keep clear, and how long a position is held before a strategy closes it.

mod day;

use std::time::Duration;

use engine_core::Fixed;
use serde::Deserialize;

pub use day::{DayLoss, DayState, DayStatus, LossLimit};

/// The most positions a config may allow open at once.
pub const MAX_OPEN_POSITIONS: usize = 1_000;

/// The most a margin buffer may be, in bps of the requirement: ten times it.
pub const MAX_BUFFER_BPS: i64 = 100_000;

/// The longest minimum hold a config may name.
pub const MAX_MIN_HOLD: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Why a risk section is refused.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum RiskError {
    #[error("max_open_positions must be from 1 to {MAX_OPEN_POSITIONS}")]
    OpenPositions,
    #[error("max_total_notional must be above 0 and at most {}", Fixed::MAX_INPUT)]
    TotalNotional,
    #[error("daily_loss must be above 0 USD, or from 1 to 10000 bps of the day's opening equity")]
    DailyLoss,
    #[error("margin_buffer_bps must be from 0 to {MAX_BUFFER_BPS}")]
    MarginBuffer,
    #[error("min_hold must be at most {} hours", MAX_MIN_HOLD.as_secs() / 3600)]
    MinHold,
}

/// A `[risk]` section. The limits hold per account, across its markets.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RiskParams {
    /// No order opens a position in another market once this many are open.
    pub max_open_positions: usize,
    /// No order takes the sum of the positions' notional, in USD, above this.
    pub max_total_notional: Fixed,
    /// At this loss in a UTC day the account is flattened and stopped until the next day.
    pub daily_loss: LossLimit,
    /// No order that adds to a position leaves equity less than the requirement plus this many bps
    /// of it. The engine closes positions when equity falls below half the requirement.
    #[serde(default = "default_margin_buffer_bps")]
    pub margin_buffer_bps: i64,
    /// A position is held this long, in sequenced time, before a strategy may close or reverse it.
    #[serde(default = "default_min_hold", with = "humantime_serde")]
    pub min_hold: Duration,
}

impl RiskParams {
    /// Checks every limit is in range.
    /// Error cases:
    /// - see `RiskError`
    pub fn validate(&self) -> Result<(), RiskError> {
        if !(1..=MAX_OPEN_POSITIONS).contains(&self.max_open_positions) {
            return Err(RiskError::OpenPositions);
        }
        if !self.max_total_notional.is_positive() || self.max_total_notional > Fixed::MAX_INPUT {
            return Err(RiskError::TotalNotional);
        }
        if !self.daily_loss.is_valid() {
            return Err(RiskError::DailyLoss);
        }
        if !(0..=MAX_BUFFER_BPS).contains(&self.margin_buffer_bps) {
            return Err(RiskError::MarginBuffer);
        }
        if self.min_hold > MAX_MIN_HOLD {
            return Err(RiskError::MinHold);
        }
        Ok(())
    }
}

/// A margin buffer of 100%: equity at least twice the requirement, four times where the engine closes.
fn default_margin_buffer_bps() -> i64 {
    10_000
}

/// An hour. The engine's profit fee is to fall with holding time (monkey-trading/engine#614), so a
/// longer hold is the conservative default until its schedule lands.
fn default_min_hold() -> Duration {
    Duration::from_secs(3600)
}

#[cfg(test)]
mod tests;
