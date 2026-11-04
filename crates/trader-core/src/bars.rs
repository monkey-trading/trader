//! Bars of a market's index marks: fixed windows of sequenced time, aligned to the Unix epoch.

use engine_core::Fixed;
use engine_core::types::Millis;

/// One window of marks: its start, and the last mark in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bar {
    pub start: Millis,
    pub close: Fixed,
}

/// Builds bars from marks in sequenced-time order. A window no mark reached makes no bar.
#[derive(Clone, Debug)]
pub struct BarBuilder {
    length: Millis,
    /// Marks before this time belong to bars already built elsewhere, and are ignored.
    from: Millis,
    current: Option<Bar>,
}

impl BarBuilder {
    /// Bars `length` milliseconds long that ignore marks before `from`.
    ///
    /// # Panics
    /// When `length` is not positive; bar lengths are validated where they enter.
    pub fn new(length: Millis, from: Millis) -> Self {
        assert!(length > 0, "a bar length is validated before it reaches here");
        Self { length, from, current: None }
    }

    /// The start of the window that holds `ts`.
    pub fn window_start(&self, ts: Millis) -> Millis {
        ts - ts.rem_euclid(self.length)
    }

    /// Advances sequenced time to `ts`, a command's time, with the mark it left if any. Returns the
    /// bar `ts` completes: the current one, once `ts` is past its window. Times arrive in order,
    /// as commands do; a mark before `from` is ignored.
    pub fn on_command(&mut self, ts: Millis, mark: Option<Fixed>) -> Option<Bar> {
        if ts < self.from {
            return None;
        }
        let start = self.window_start(ts);
        let completed = self.current.take_if(|bar| bar.start < start);
        if let Some(close) = mark {
            match &mut self.current {
                Some(bar) => bar.close = close,
                None => self.current = Some(Bar { start, close }),
            }
        }
        completed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: Millis = 60_000;

    fn fx(text: &str) -> Fixed {
        text.parse().unwrap()
    }

    /// Marks in one window update its close; the first command in the next window completes it.
    #[test]
    fn a_window_closes_at_the_first_command_past_it() {
        let mut bars = BarBuilder::new(MINUTE, 0);
        assert_eq!(bars.on_command(MINUTE + 10, Some(fx("100"))), None);
        assert_eq!(bars.on_command(MINUTE + 59_999, Some(fx("101"))), None);
        let bar = bars.on_command(2 * MINUTE, None);
        assert_eq!(bar, Some(Bar { start: MINUTE, close: fx("101") }));
    }

    /// The mark carried by the command that completes a bar opens the next one.
    #[test]
    fn the_completing_mark_opens_the_next_bar() {
        let mut bars = BarBuilder::new(MINUTE, 0);
        bars.on_command(10, Some(fx("1")));
        assert_eq!(bars.on_command(MINUTE + 5, Some(fx("2"))), Some(Bar { start: 0, close: fx("1") }));
        assert_eq!(bars.on_command(3 * MINUTE, None), Some(Bar { start: MINUTE, close: fx("2") }));
    }

    /// A window with no mark makes no bar, and time alone never opens one.
    #[test]
    fn empty_windows_make_no_bar() {
        let mut bars = BarBuilder::new(MINUTE, 0);
        assert_eq!(bars.on_command(5, None), None);
        assert_eq!(bars.on_command(MINUTE + 5, None), None);
        bars.on_command(2 * MINUTE + 1, Some(fx("7")));
        assert_eq!(bars.on_command(10 * MINUTE, None), Some(Bar { start: 2 * MINUTE, close: fx("7") }));
        assert_eq!(bars.on_command(11 * MINUTE, None), None);
    }

    /// Marks before `from`, which earlier bars already hold, are ignored.
    #[test]
    fn marks_before_from_are_ignored() {
        let mut bars = BarBuilder::new(MINUTE, 2 * MINUTE);
        assert_eq!(bars.on_command(2 * MINUTE - 1, Some(fx("9"))), None);
        bars.on_command(2 * MINUTE, Some(fx("10")));
        assert_eq!(bars.on_command(3 * MINUTE, None), Some(Bar { start: 2 * MINUTE, close: fx("10") }));
    }

    /// Windows align to the epoch, before it included.
    #[test]
    fn windows_align_to_the_epoch() {
        let bars = BarBuilder::new(5 * MINUTE, Millis::MIN);
        assert_eq!(bars.window_start(7 * MINUTE + 3), 5 * MINUTE);
        assert_eq!(bars.window_start(-1), -5 * MINUTE);
    }
}
