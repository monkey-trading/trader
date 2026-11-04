//! Bollinger bands: the mean of the last `period` closes and a multiple of their population
//! standard deviation either side of it, from running sums, in constant time per close.

use engine_core::Fixed;

/// The longest period a band may have. With closes at most `MAX_CLOSE`, `period` times the sum of
/// squares of a full window stays below the `i128` range.
pub const MAX_BAND_PERIOD: usize = 1_000;

/// The largest close a band takes, 10^7 units: its square times `MAX_BAND_PERIOD` squared stays
/// inside `i128`. A mark above it is refused as input before it reaches a band.
pub const MAX_CLOSE: Fixed = Fixed::from_int(10_000_000);

/// The widest multiple of the standard deviation a band may use.
pub const MAX_WIDTH: i64 = 10;

/// The bands' three edges. The lower edge rounds down and the upper up, so rounding never narrows
/// the band.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edges {
    pub lower: Fixed,
    /// The mean, truncated toward zero.
    pub middle: Fixed,
    pub upper: Fixed,
}

/// A fixed window of closes with the sums the bands come from.
#[derive(Clone, Debug)]
pub struct Bands {
    period: usize,
    window: Vec<i128>,
    next: usize,
    sum: i128,
    sum_squares: i128,
}

impl Bands {
    /// Bands over `period` closes.
    ///
    /// # Panics
    /// When `period` is below 2 or above `MAX_BAND_PERIOD`; callers validate periods where they enter.
    pub fn new(period: usize) -> Self {
        assert!((2..=MAX_BAND_PERIOD).contains(&period), "a band period is validated before it reaches here");
        Self { period, window: Vec::with_capacity(period), next: 0, sum: 0, sum_squares: 0 }
    }

    /// Adds `close`, dropping the oldest once the window is full.
    ///
    /// # Panics
    /// When `close` is not in `(0, MAX_CLOSE]`; the strategy refuses such a mark first.
    pub fn push(&mut self, close: Fixed) {
        assert!(close.is_positive() && close <= MAX_CLOSE, "a close is range-checked before it reaches here");
        let raw = close.raw();
        if self.window.len() < self.period {
            self.window.push(raw);
        } else {
            let old = std::mem::replace(&mut self.window[self.next], raw);
            self.sum -= old;
            self.sum_squares -= old * old;
            self.next = (self.next + 1) % self.period;
        }
        self.sum += raw;
        self.sum_squares += raw * raw;
    }

    /// The edges at `width` standard deviations (at most `MAX_WIDTH` units) either side of the
    /// mean, once the window is full. The deviation rounds up.
    pub fn edges(&self, width: Fixed) -> Option<Edges> {
        if self.window.len() < self.period {
            return None;
        }
        // `period` is at most `MAX_BAND_PERIOD`, so it fits an `i128` exactly.
        let n = self.period as i128;
        // n times the standard deviation squared, times n: never negative, by Cauchy-Schwarz.
        let spread = n * self.sum_squares - self.sum * self.sum;
        assert!(spread >= 0, "the sums of one window cannot give a negative variance");
        let root = spread.isqrt();
        // n times the deviation, rounded up.
        let n_deviation = if root * root < spread { root + 1 } else { root };
        let reach = (width.raw() * n_deviation + Fixed::ONE.raw() - 1).div_euclid(Fixed::ONE.raw());
        Some(Edges {
            lower: Fixed::from_raw((self.sum - reach).div_euclid(n)),
            middle: Fixed::from_raw(self.sum / n),
            upper: Fixed::from_raw(-(-(self.sum + reach)).div_euclid(n)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fx(text: &str) -> Fixed {
        text.parse().unwrap()
    }

    /// Closes 2, 4, 4, 4, 5, 5, 7, 9 have mean 5 and population deviation 2, so at two deviations the
    /// edges are 1 and 9. The window of 4 over the last four (5, 5, 7, 9) has mean 6.5 and deviation
    /// sqrt(2.75) = 1.658312395...; one deviation up rounds to 8.158312396.
    #[test]
    fn known_answers() {
        let mut bands = Bands::new(8);
        [2, 4, 4, 4, 5, 5, 7, 9].into_iter().for_each(|c| bands.push(Fixed::from_int(c)));
        assert_eq!(bands.edges(fx("2")), Some(Edges { lower: fx("1"), middle: fx("5"), upper: fx("9") }));
        let mut short = Bands::new(4);
        [2, 4, 4, 4, 5, 5, 7, 9].into_iter().for_each(|c| short.push(Fixed::from_int(c)));
        let edges = short.edges(fx("1")).unwrap();
        assert_eq!(edges.middle, fx("6.5"));
        assert_eq!(edges.upper, fx("8.158312396"));
        assert_eq!(edges.lower, fx("4.841687604"));
    }

    /// No edges before the window is full, and a window of equal closes has zero width.
    #[test]
    fn needs_a_full_window() {
        let mut bands = Bands::new(3);
        bands.push(fx("10"));
        bands.push(fx("10"));
        assert_eq!(bands.edges(fx("2")), None);
        bands.push(fx("10"));
        assert_eq!(bands.edges(fx("2")), Some(Edges { lower: fx("10"), middle: fx("10"), upper: fx("10") }));
    }

    /// The sums follow the window as old closes leave: after 1, 2, 3 and 4 the 3-window holds 2, 3, 4.
    #[test]
    fn the_window_slides() {
        let mut bands = Bands::new(3);
        [1, 2, 3, 4].into_iter().for_each(|c| bands.push(Fixed::from_int(c)));
        // Mean 3, deviation sqrt(2/3) = 0.816496580...; the edges at one
        // deviation, rounded outward, are 3 - 0.816496581 and 3 + 0.816496581.
        let edges = bands.edges(fx("1")).unwrap();
        assert_eq!((edges.lower, edges.middle, edges.upper), (fx("2.183503419"), fx("3"), fx("3.816496581")));
    }

    /// The largest closes in the longest window do not overflow.
    #[test]
    fn the_largest_window_fits() {
        let mut bands = Bands::new(MAX_BAND_PERIOD);
        for step in 0..MAX_BAND_PERIOD {
            bands.push(if step % 2 == 0 { MAX_CLOSE } else { Fixed::from_raw(1) });
        }
        let edges = bands.edges(Fixed::from_int(MAX_WIDTH)).unwrap();
        assert_eq!(edges.middle, Fixed::from_raw(MAX_CLOSE.raw() / 2));
    }

    #[test]
    #[should_panic(expected = "range-checked")]
    fn a_close_above_the_bound_is_refused() {
        Bands::new(2).push(MAX_CLOSE + Fixed::from_raw(1));
    }
}
