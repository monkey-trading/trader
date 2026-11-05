//! The exponential moving average: the mean of the first `period` values, then each value pulled
//! toward the average by `2 / (period + 1)`.

use engine_core::Fixed;
use engine_core::fixed::Rounding;

use crate::sma::MAX_PERIOD;

/// The average of the values pushed, seeded with the simple mean of the first `period` of them
/// and updated in constant time after that.
#[derive(Clone, Debug)]
pub struct Ema {
    period: usize,
    seen: usize,
    /// The sum of the seed values, until the average exists.
    seed_sum: Fixed,
    value: Fixed,
}

impl Ema {
    /// An average over `period` values.
    ///
    /// # Panics
    /// When `period` is zero or above `MAX_PERIOD`; callers validate periods where they enter.
    pub fn new(period: usize) -> Self {
        assert!((1..=MAX_PERIOD).contains(&period), "an EMA period is validated before it reaches here");
        Self { period, seen: 0, seed_sum: Fixed::ZERO, value: Fixed::ZERO }
    }

    /// Adds `value`. Each step after the seed moves the average by `(value - average) * 2 /
    /// (period + 1)`, truncated toward zero.
    pub fn push(&mut self, value: Fixed) {
        if self.seen < self.period {
            self.seed_sum += value;
            self.seen += 1;
            if self.seen == self.period {
                // `period` is at most `MAX_PERIOD`, so it fits an `i64`.
                self.value = self.seed_sum.div_int(self.period as i64);
            }
            return;
        }
        let weight = Fixed::from_int(2);
        let span = Fixed::from_int(self.period as i64 + 1);
        self.value += (value - self.value).mul_div(weight, span, Rounding::TowardZero);
    }

    /// The average, once `period` values have been pushed.
    pub fn value(&self) -> Option<Fixed> {
        (self.seen == self.period).then_some(self.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fx(text: &str) -> Fixed {
        text.parse().unwrap()
    }

    /// Period 3 over 2, 4, 6, 8, 10: the seed is 4, then 4 + (8 - 4) / 2 = 6 and 6 + (10 - 6) / 2 = 8.
    #[test]
    fn seeds_with_the_mean_then_follows_the_values() {
        let mut ema = Ema::new(3);
        let seen: Vec<_> = ["2", "4", "6", "8", "10"]
            .into_iter()
            .map(|v| {
                ema.push(fx(v));
                ema.value()
            })
            .collect();
        assert_eq!(seen, [None, None, Some(fx("4")), Some(fx("6")), Some(fx("8"))]);
    }

    /// Period 4 has weight 2/5: seed (10 + 11 + 12 + 13) / 4 = 11.5, then 11.5 + (14 - 11.5) * 0.4 =
    /// 12.5 and 12.5 + (12 - 12.5) * 0.4 = 12.3.
    #[test]
    fn known_answers_with_fractions() {
        let mut ema = Ema::new(4);
        ["10", "11", "12", "13"].into_iter().for_each(|v| ema.push(fx(v)));
        assert_eq!(ema.value(), Some(fx("11.5")));
        ema.push(fx("14"));
        assert_eq!(ema.value(), Some(fx("12.5")));
        ema.push(fx("12"));
        assert_eq!(ema.value(), Some(fx("12.3")));
    }

    /// The step truncates toward zero, above and below the average: period 2 has weight 2/3, so a
    /// step of 1 moves 0.666666666 up and 0.666666666 down.
    #[test]
    fn steps_truncate_toward_zero() {
        let mut up = Ema::new(2);
        ["0", "0", "1"].into_iter().for_each(|v| up.push(fx(v)));
        assert_eq!(up.value(), Some(fx("0.666666666")));
        let mut down = Ema::new(2);
        ["0", "0", "-1"].into_iter().for_each(|v| down.push(fx(v)));
        assert_eq!(down.value(), Some(fx("-0.666666666")));
    }

    /// A period of one follows the last value exactly.
    #[test]
    fn period_one_is_the_last_value() {
        let mut ema = Ema::new(1);
        ema.push(fx("5"));
        ema.push(fx("7.25"));
        assert_eq!(ema.value(), Some(fx("7.25")));
    }

    #[test]
    #[should_panic(expected = "validated")]
    fn zero_period_is_refused() {
        Ema::new(0);
    }
}
