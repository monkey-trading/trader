//! Bars of one market at one length, and the averages computed from them. Every strategy that reads
//! the same market at the same bar length shares one series, so a bar's closing price is averaged
//! once however many accounts trade on it.

use engine_core::types::Millis;

use crate::bars::BarBuilder;
use crate::sma::Sma;
use crate::strategy::{Target, crossover_target};

/// The bars of one market and bar length, and one average for each distinct period asked for.
#[derive(Clone, Debug)]
pub struct Series {
    length: Millis,
    /// The bars from the log, once the series has warmed up.
    pub live: Option<BarBuilder>,
    averages: Vec<Sma>,
}

impl Series {
    /// A series of bars `length` ms long with an average of each of `periods`, repeats counted once.
    ///
    /// # Panics
    /// When `length` is not positive or a period is outside `Sma`'s bounds; both are validated where
    /// they enter.
    pub fn new(length: Millis, periods: &[usize]) -> Self {
        let mut distinct: Vec<usize> = periods.to_vec();
        distinct.sort_unstable();
        distinct.dedup();
        assert!(length > 0, "a bar length is validated before it reaches here");
        Self { length, live: None, averages: distinct.into_iter().map(Sma::new).collect() }
    }

    /// The bar length in milliseconds.
    pub fn length(&self) -> Millis {
        self.length
    }

    /// The slot of the average over `period`, if the series has one.
    pub fn slot(&self, period: usize) -> Option<usize> {
        self.averages.iter().position(|sma| sma.period() == period)
    }

    /// The longest period of the series' averages.
    pub fn longest(&self) -> usize {
        self.averages.iter().map(Sma::period).max().unwrap_or(0)
    }

    /// Takes a completed bar's close into every average.
    pub fn push_close(&mut self, close: engine_core::Fixed) {
        for sma in &mut self.averages {
            sma.push(close);
        }
    }

    /// The crossover target of the averages in slots `fast` and `slow`, once both are full.
    pub fn crossover(&self, fast: usize, slow: usize) -> Option<Target> {
        crossover_target(self.averages.get(fast)?, self.averages.get(slow)?)
    }

    /// The averages of slots `fast` and `slow`, for logging.
    pub fn averages(&self, fast: usize, slow: usize) -> (Option<engine_core::Fixed>, Option<engine_core::Fixed>) {
        (self.averages.get(fast).and_then(Sma::value), self.averages.get(slow).and_then(Sma::value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::SmaCrossover;

    /// Strategies with different periods read one series, and each gets the target its own
    /// `SmaCrossover` gives on the same closes.
    #[test]
    fn shared_averages_match_each_strategys_own() {
        let mut series = Series::new(60_000, &[2, 5, 3, 5]);
        assert_eq!(series.averages.len(), 3, "a repeated period is averaged once");
        let pairs = [(2, 5), (3, 5), (2, 3)];
        let mut own: Vec<SmaCrossover> = pairs.iter().map(|&(f, s)| SmaCrossover::new(f, s).unwrap()).collect();
        let slots: Vec<(usize, usize)> =
            pairs.iter().map(|&(f, s)| (series.slot(f).unwrap(), series.slot(s).unwrap())).collect();
        for close in ["10", "11", "12", "9", "8", "8.5", "13", "14", "7", "7"] {
            let close = close.parse().unwrap();
            series.push_close(close);
            for ((strategy, &(fast, slow)), pair) in own.iter_mut().zip(&slots).zip(pairs) {
                let expected = strategy.on_close(close);
                assert_eq!(series.crossover(fast, slow), expected, "{pair:?} at {close}");
            }
        }
        assert_eq!(series.longest(), 5);
    }
}
