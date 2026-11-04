//! The highest or lowest of the last `period` values, in amortised constant time per value.

use std::collections::VecDeque;

use engine_core::Fixed;

use crate::sma::MAX_PERIOD;

/// Which end of the window a `SlidingExtreme` tracks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Extreme {
    Highest,
    Lowest,
}

/// The highest or lowest of the last `period` values pushed. It keeps the candidates in order, so
/// a push removes the values it beats and the one that left the window, and the answer is the front.
#[derive(Clone, Debug)]
pub struct SlidingExtreme {
    period: usize,
    which: Extreme,
    pushed: u64,
    /// Candidates with the index each was pushed at; the front is the extreme of the window.
    candidates: VecDeque<(u64, Fixed)>,
}

impl SlidingExtreme {
    /// The extreme over windows of `period` values.
    ///
    /// # Panics
    /// When `period` is zero or above `MAX_PERIOD`; callers validate periods where they enter.
    pub fn new(period: usize, which: Extreme) -> Self {
        assert!((1..=MAX_PERIOD).contains(&period), "a window period is validated before it reaches here");
        Self { period, which, pushed: 0, candidates: VecDeque::with_capacity(period) }
    }

    /// Adds `value`, dropping the oldest from the window once it holds `period` values.
    pub fn push(&mut self, value: Fixed) {
        let beaten = |held: Fixed| match self.which {
            Extreme::Highest => held <= value,
            Extreme::Lowest => held >= value,
        };
        while self.candidates.back().is_some_and(|(_, held)| beaten(*held)) {
            self.candidates.pop_back();
        }
        while self.candidates.front().is_some_and(|(at, _)| at + self.period as u64 <= self.pushed) {
            self.candidates.pop_front();
        }
        self.candidates.push_back((self.pushed, value));
        self.pushed += 1;
    }

    /// The extreme of the last `period` values, once `period` have been pushed.
    pub fn value(&self) -> Option<Fixed> {
        if self.pushed < self.period as u64 {
            return None;
        }
        self.candidates.front().map(|(_, value)| *value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fx(text: &str) -> Fixed {
        text.parse().unwrap()
    }

    fn run(which: Extreme, period: usize, values: &[&str]) -> Vec<Option<Fixed>> {
        let mut window = SlidingExtreme::new(period, which);
        values
            .iter()
            .map(|v| {
                window.push(fx(v));
                window.value()
            })
            .collect()
    }

    /// Highest of the last 3 over 5, 1, 4, 2, 2, 0, 9: nothing until three values, then 5, 4, 4, 2, 9.
    #[test]
    fn highest_of_the_window() {
        let seen = run(Extreme::Highest, 3, &["5", "1", "4", "2", "2", "0", "9"]);
        let expected = [None, None, Some(fx("5")), Some(fx("4")), Some(fx("4")), Some(fx("2")), Some(fx("9"))];
        assert_eq!(seen, expected);
    }

    /// Lowest of the last 3 over 5, 1, 4, 2, 2, 0, 9: nothing, nothing, 1, 1, 2, 0, 0.
    #[test]
    fn lowest_of_the_window() {
        let seen = run(Extreme::Lowest, 3, &["5", "1", "4", "2", "2", "0", "9"]);
        let expected = [None, None, Some(fx("1")), Some(fx("1")), Some(fx("2")), Some(fx("0")), Some(fx("0"))];
        assert_eq!(seen, expected);
    }

    /// An extreme leaves the window after exactly `period` pushes, and equal values keep the newest.
    #[test]
    fn the_extreme_expires_after_the_period() {
        let seen = run(Extreme::Highest, 2, &["9", "1", "1", "1"]);
        assert_eq!(seen, [None, Some(fx("9")), Some(fx("1")), Some(fx("1"))]);
        let seen = run(Extreme::Lowest, 1, &["3", "3", "-2", "5"]);
        assert_eq!(seen, [Some(fx("3")), Some(fx("3")), Some(fx("-2")), Some(fx("5"))]);
    }

    /// The candidates never outgrow the window, however the values run.
    #[test]
    fn candidates_stay_within_the_period() {
        let mut window = SlidingExtreme::new(4, Extreme::Highest);
        for step in 0..100 {
            window.push(Fixed::from_int(100 - step));
            assert!(window.candidates.len() <= 4);
        }
    }
}
