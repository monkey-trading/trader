//! How large a position the bot opens: a fixed USD notional or a fraction of equity, held to a
//! leverage cap below the market's own.

use engine_core::Fixed;
use engine_core::fixed::Rounding;
use engine_core::market::MarketSpec;
use serde::Deserialize;

/// The most a configured fraction of equity may be, in bps: ten times equity.
pub const MAX_FRACTION_BPS: i64 = 100_000;

/// The largest leverage cap a config may name.
pub const MAX_LEVERAGE: i64 = 1_000;

/// Why a sizing section is refused.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum SizingError {
    #[error("the notional must be above 0 and at most {}", Fixed::MAX_INPUT)]
    Notional,
    #[error("the fraction must be from 1 to {MAX_FRACTION_BPS} bps of equity")]
    Fraction,
    #[error("the leverage cap must be above 0 and at most {MAX_LEVERAGE}")]
    Leverage,
    #[error("the leverage cap {cap} is not below the market's own, {market}")]
    AboveMarket { cap: Fixed, market: Fixed },
}

/// A `[sizing]` section: how big each position is, and the most leverage an account takes on, as the
/// positions' notional over equity.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "by", rename_all = "snake_case", deny_unknown_fields)]
pub enum Sizing {
    /// A position of `notional` USD.
    FixedNotional { notional: Fixed, max_leverage: Fixed },
    /// A position of `bps` of the account's equity at the time it opens.
    EquityFraction { bps: i64, max_leverage: Fixed },
}

impl Sizing {
    /// Checks the section against itself: every figure in range.
    /// Error cases:
    /// - see `SizingError`
    pub fn check(&self) -> Result<(), SizingError> {
        match *self {
            Self::FixedNotional { notional, .. } if !notional.is_positive() || notional > Fixed::MAX_INPUT => {
                return Err(SizingError::Notional);
            }
            Self::EquityFraction { bps, .. } if !(1..=MAX_FRACTION_BPS).contains(&bps) => {
                return Err(SizingError::Fraction);
            }
            _ => {}
        }
        let cap = self.max_leverage();
        if !cap.is_positive() || cap > Fixed::from_int(MAX_LEVERAGE) {
            return Err(SizingError::Leverage);
        }
        Ok(())
    }

    /// Checks the section against itself and against `spec`, the market it sizes: the leverage cap
    /// must be below what the market's first margin tier allows.
    /// Error cases:
    /// - see `SizingError`
    pub fn validate(&self, spec: &MarketSpec) -> Result<(), SizingError> {
        self.check()?;
        let cap = self.max_leverage();
        match market_leverage(spec) {
            Some(market) if cap >= market => Err(SizingError::AboveMarket { cap, market }),
            _ => Ok(()),
        }
    }

    /// The leverage cap.
    pub fn max_leverage(&self) -> Fixed {
        match *self {
            Self::FixedNotional { max_leverage, .. } | Self::EquityFraction { max_leverage, .. } => max_leverage,
        }
    }

    /// The units of a new position at `mark` for an account with `equity`: the configured notional
    /// or fraction, cut to the leverage cap times equity, and rounded down to a whole nano-unit so
    /// the notional never exceeds either. Zero when equity or the mark is not positive. At most
    /// `Fixed::MAX_INPUT`, the most an order takes.
    pub fn qty(&self, mark: Fixed, equity: Fixed) -> Fixed {
        if !equity.is_positive() || !mark.is_positive() {
            return Fixed::ZERO;
        }
        let wanted = match *self {
            Self::FixedNotional { notional, .. } => notional,
            Self::EquityFraction { bps, .. } => equity.mul_bps_rounded(bps, Rounding::Down),
        };
        let capped = wanted.min(equity.mul_rounded(self.max_leverage(), Rounding::Down));
        capped.div_rounded(mark, Rounding::Down).min(Fixed::MAX_INPUT)
    }
}

/// The leverage a market allows at its smallest sizes: 10,000 over the first margin tier's bps. `None`
/// when that tier asks for no margin.
pub fn market_leverage(spec: &MarketSpec) -> Option<Fixed> {
    let bps = spec.margin_tiers.first().map(|tier| tier.bps).filter(|bps| *bps > 0)?;
    Some(Fixed::from_int(10_000).div_int(bps))
}

#[cfg(test)]
mod tests;
