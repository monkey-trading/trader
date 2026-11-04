//! The relative strength index with Wilder's smoothing, updated in constant time.

use std::cmp::Ordering;

use engine_core::Fixed;

use crate::sma::MAX_PERIOD;

/// The average gain and average loss of the last closes, from which the index is the gain's share
/// of both, from 0 to 100. The first `period` changes seed the averages with their simple means;
/// each later change moves them by `(average * (period - 1) + change) / period`, truncated.
#[derive(Clone, Debug)]
pub struct Rsi {
    period: usize,
    previous: Option<Fixed>,
    changes: usize,
    gain: Fixed,
    loss: Fixed,
}

impl Rsi {
    /// An index over `period` changes.
    ///
    /// # Panics
    /// When `period` is zero or above `MAX_PERIOD`; callers validate periods where they enter.
    pub fn new(period: usize) -> Self {
        assert!((1..=MAX_PERIOD).contains(&period), "an RSI period is validated before it reaches here");
        Self { period, previous: None, changes: 0, gain: Fixed::ZERO, loss: Fixed::ZERO }
    }

    /// Adds a close. The change from the close before it is a gain or a loss.
    pub fn push(&mut self, close: Fixed) {
        let Some(previous) = self.previous.replace(close) else { return };
        let change = close - previous;
        let (gain, loss) = if change.is_positive() { (change, Fixed::ZERO) } else { (Fixed::ZERO, -change) };
        // `period` is at most `MAX_PERIOD`, so it fits an `i64`.
        let period = self.period as i64;
        if self.changes < self.period {
            self.gain += gain;
            self.loss += loss;
            self.changes += 1;
            if self.changes == self.period {
                self.gain = self.gain.div_int(period);
                self.loss = self.loss.div_int(period);
            }
        } else {
            self.gain = (self.gain.mul_int(period - 1) + gain).div_int(period);
            self.loss = (self.loss.mul_int(period - 1) + loss).div_int(period);
        }
    }

    /// Whether `period` changes have been seen.
    pub fn is_ready(&self) -> bool {
        self.changes == self.period
    }

    /// The index compared with `level`, exactly, as cross products rather than a rounded quotient;
    /// 50 while no close has moved. `None` until `period` changes have been seen. `level` is at most
    /// 100 and the averages at most `Fixed::MAX_INPUT`, so neither product leaves `i128`.
    pub fn compare(&self, level: Fixed) -> Option<Ordering> {
        if !self.is_ready() {
            return None;
        }
        let total = self.gain + self.loss;
        if total.is_zero() {
            return Some(Fixed::from_int(50).cmp(&level));
        }
        // 100 * gain / total against level, both sides times the scale and the total.
        Some((self.gain.raw() * 100 * Fixed::ONE.raw()).cmp(&(level.raw() * total.raw())))
    }

    /// The index from 0 to 100, truncated toward zero; 50 while no close has moved.
    pub fn value(&self) -> Option<Fixed> {
        if !self.is_ready() {
            return None;
        }
        let total = self.gain + self.loss;
        if total.is_zero() {
            return Some(Fixed::from_int(50));
        }
        Some(Fixed::from_int(100).mul_div(self.gain, total, engine_core::fixed::Rounding::TowardZero))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fx(text: &str) -> Fixed {
        text.parse().unwrap()
    }

    fn run(period: usize, closes: &[&str]) -> Vec<Option<Fixed>> {
        let mut rsi = Rsi::new(period);
        closes
            .iter()
            .map(|c| {
                rsi.push(fx(c));
                rsi.value()
            })
            .collect()
    }

    /// Period 2 over 10, 12, 11, 13, 12: the changes are +2, -1, +2, -1. Seed averages: gain 1,
    /// loss 0.5, index 100 * 1 / 1.5 = 66.666666666. Then gain (1 + 2) / 2 = 1.5, loss (0.5 + 0) / 2 =
    /// 0.25, index 100 * 1.5 / 1.75 = 85.714285714. Then gain 0.75, loss (0.25 + 1) / 2 = 0.625,
    /// index 100 * 0.75 / 1.375 = 54.545454545.
    #[test]
    fn follows_wilders_smoothing() {
        let seen = run(2, &["10", "12", "11", "13", "12"]);
        let expected = [None, None, Some(fx("66.666666666")), Some(fx("85.714285714")), Some(fx("54.545454545"))];
        assert_eq!(seen, expected);
    }

    /// Only gains give 100, only losses give 0, and a flat run gives 50.
    #[test]
    fn extremes_and_a_flat_run() {
        assert_eq!(run(3, &["1", "2", "3", "4"]).last(), Some(&Some(fx("100"))));
        assert_eq!(run(3, &["4", "3", "2", "1"]).last(), Some(&Some(fx("0"))));
        assert_eq!(run(3, &["5", "5", "5", "5"]).last(), Some(&Some(fx("50"))));
    }

    /// The comparison with a level is exact where the truncated index is not: 66.666666666 is below
    /// 200 / 3, which the truncated index cannot say, and equal to 50 on a flat run.
    #[test]
    fn compares_without_rounding() {
        let mut rsi = Rsi::new(2);
        ["10", "12", "11"].into_iter().for_each(|c| rsi.push(fx(c)));
        assert_eq!(rsi.compare(fx("66.666666667")), Some(Ordering::Less));
        assert_eq!(rsi.compare(fx("66.666666666")), Some(Ordering::Greater));
        let mut flat = Rsi::new(2);
        ["5", "5", "5"].into_iter().for_each(|c| flat.push(fx(c)));
        assert_eq!(flat.compare(fx("50")), Some(Ordering::Equal));
        assert_eq!(flat.compare(fx("30")), Some(Ordering::Greater));
        assert_eq!(Rsi::new(2).compare(fx("50")), None);
    }

    #[test]
    #[should_panic(expected = "validated")]
    fn zero_period_is_refused() {
        Rsi::new(0);
    }
}
