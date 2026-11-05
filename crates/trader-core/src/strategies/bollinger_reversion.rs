//! Bollinger band reversion: buy a close below the lower band, sell one above the upper band, and
//! go flat when the close returns to the middle band.

use engine_core::Fixed;

use crate::bands::{Bands, MAX_BAND_PERIOD, MAX_WIDTH};
use crate::strategies::{Signals, next_target};
use crate::strategy::{ParamsError, Target};

/// Long on a close below the lower band, short on one above the upper band, and flat again when a
/// long's close reaches the middle band or a short's falls to it. The bands are the mean of the
/// last `period` closes, this one included, and `width` standard deviations either side. Between
/// the bands the target stays what it was.
#[derive(Clone, Debug)]
pub struct BollingerReversion {
    bands: Bands,
    width: Fixed,
    target: Target,
}

impl BollingerReversion {
    /// A reversion over `period` closes with bands `width` deviations wide.
    /// Error cases:
    /// - `period` is below 2 or above `MAX_BAND_PERIOD`
    /// - `width` is not above 0 and at most `MAX_WIDTH`
    pub fn new(period: usize, width: Fixed) -> Result<Self, ParamsError> {
        if !(2..=MAX_BAND_PERIOD).contains(&period) {
            return Err(ParamsError::BandPeriod);
        }
        if !width.is_positive() || width > Fixed::from_int(MAX_WIDTH) {
            return Err(ParamsError::Width);
        }
        Ok(Self { bands: Bands::new(period), width, target: Target::Flat })
    }

    /// Takes a completed bar's close, which must be in `(0, MAX_CLOSE]`, and returns the target once
    /// the window is full.
    ///
    /// # Panics
    /// When `close` is out of range; `Strategy::on_close` refuses such a mark first.
    pub fn on_close(&mut self, close: Fixed) -> Option<Target> {
        self.bands.push(close);
        let edges = self.bands.edges(self.width)?;
        self.target = next_target(
            self.target,
            Signals {
                enter_long: close < edges.lower,
                enter_short: close > edges.upper,
                exit_long: close >= edges.middle,
                exit_short: close <= edges.middle,
            },
        );
        Some(self.target)
    }
}
