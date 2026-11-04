//! The simple moving average: the mean of the last `period` values, updated in constant time.

use engine_core::Fixed;

/// The longest period an average may have. It bounds the window's memory and keeps the cross
/// products `SmaCrossover` compares far inside `i128`: a sum of this many valid inputs times this
/// period is below 10^27 raw units.
pub const MAX_PERIOD: usize = 10_000;

/// The mean of the last `period` values pushed, from a fixed window and a running sum.
#[derive(Clone, Debug)]
pub struct Sma {
    period: usize,
    /// The last `period` values, oldest at `next` once full.
    window: Vec<Fixed>,
    next: usize,
    sum: Fixed,
}

impl Sma {
    /// An average over `period` values.
    ///
    /// # Panics
    /// When `period` is zero or above `MAX_PERIOD`; callers validate periods where they enter.
    pub fn new(period: usize) -> Self {
        assert!((1..=MAX_PERIOD).contains(&period), "an SMA period is validated before it reaches here");
        Self { period, window: Vec::with_capacity(period), next: 0, sum: Fixed::ZERO }
    }

    /// Adds `value`, dropping the oldest once the window is full.
    pub fn push(&mut self, value: Fixed) {
        if self.window.len() < self.period {
            self.window.push(value);
        } else {
            self.sum -= self.window[self.next];
            self.window[self.next] = value;
            self.next = (self.next + 1) % self.period;
        }
        self.sum += value;
    }

    /// The number of values averaged.
    pub fn period(&self) -> usize {
        self.period
    }

    /// The sum of the window, once it holds `period` values.
    pub fn sum(&self) -> Option<Fixed> {
        self.is_ready().then_some(self.sum)
    }

    /// The average, truncated toward zero, once the window holds `period` values.
    pub fn value(&self) -> Option<Fixed> {
        // `period` is at most `MAX_PERIOD`, so it fits an `i64`.
        self.sum().map(|sum| sum.div_int(self.period as i64))
    }

    /// Whether the window holds `period` values.
    pub fn is_ready(&self) -> bool {
        self.window.len() == self.period
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fx(text: &str) -> Fixed {
        text.parse().unwrap()
    }

    /// A three-value average over 1, 2, 3, 4, 5, 6: nothing until three values, then 2, 3, 4, 5.
    #[test]
    fn averages_the_last_period_values() {
        let mut sma = Sma::new(3);
        let mut seen = Vec::new();
        for value in ["1", "2", "3", "4", "5", "6"] {
            sma.push(fx(value));
            seen.push(sma.value());
        }
        assert_eq!(seen, [None, None, Some(fx("2")), Some(fx("3")), Some(fx("4")), Some(fx("5"))]);
    }

    /// Known answers computed by hand: the mean of 100.5, 101.25, 99.75, 102 is 100.875, and after
    /// 98.5 replaces 100.5 it is 100.375.
    #[test]
    fn known_answers_with_fractions() {
        let mut sma = Sma::new(4);
        ["100.5", "101.25", "99.75", "102"].into_iter().for_each(|v| sma.push(fx(v)));
        assert_eq!(sma.value(), Some(fx("100.875")));
        assert_eq!(sma.sum(), Some(fx("403.5")));
        sma.push(fx("98.5"));
        assert_eq!(sma.value(), Some(fx("100.375")));
    }

    /// The mean truncates toward zero at nine decimals: (1 + 1 + 2) / 3 = 1.333333333.
    #[test]
    fn truncates_toward_zero() {
        let mut sma = Sma::new(3);
        ["1", "1", "2"].into_iter().for_each(|v| sma.push(fx(v)));
        assert_eq!(sma.value(), Some(fx("1.333333333")));
    }

    /// A period of one follows the last value.
    #[test]
    fn period_one_is_the_last_value() {
        let mut sma = Sma::new(1);
        sma.push(fx("5"));
        sma.push(fx("7"));
        assert_eq!(sma.value(), Some(fx("7")));
    }

    #[test]
    #[should_panic(expected = "validated")]
    fn zero_period_is_refused() {
        Sma::new(0);
    }

    #[test]
    #[should_panic(expected = "validated")]
    fn period_above_the_bound_is_refused() {
        Sma::new(MAX_PERIOD + 1);
    }
}
