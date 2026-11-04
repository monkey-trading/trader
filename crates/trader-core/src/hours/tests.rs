use super::*;

pub(crate) const HOUR: Millis = 3_600_000;
pub(crate) const DAY: Millis = 24 * HOUR;

/// 2026-11-02 is a Monday; its midnight UTC in milliseconds.
pub(crate) const MONDAY: Millis = 1_793_577_600_000;

/// Monday to Friday 14:30 to 21:00 UTC, as an exchange's session, with Thursday 5 November a holiday.
pub(crate) fn exchange() -> Schedule {
    let windows: Vec<_> = ["Mon", "Tue", "Wed", "Thu", "Fri"]
        .iter()
        .map(|day| serde_json::json!({ "open": format!("{day} 14:30"), "close": format!("{day} 21:00") }))
        .collect();
    let schedule = serde_json::json!({
        "kind": "weekly", "tz": "UTC", "windows": windows, "closed_dates": ["2026-11-05"]
    });
    serde_json::from_value(schedule).unwrap()
}

#[test]
fn an_always_open_market_is_always_open_and_never_gaps() {
    let mut tracker = GapTracker::new();
    assert!(is_open(&Schedule::AlwaysOpen, MONDAY + 3 * HOUR));
    for hours in 0..100 {
        assert!(!tracker.on_bar(&Schedule::AlwaysOpen, MONDAY + hours * HOUR, HOUR));
    }
}

#[test]
fn open_and_closed_by_the_clock() {
    let schedule = exchange();
    assert!(!is_open(&schedule, MONDAY + 14 * HOUR + 29 * 60_000));
    assert!(is_open(&schedule, MONDAY + 14 * HOUR + 30 * 60_000));
    assert!(is_open(&schedule, MONDAY + 21 * HOUR - 1));
    assert!(!is_open(&schedule, MONDAY + 21 * HOUR));
    assert!(!is_open(&schedule, MONDAY + 3 * DAY + 16 * HOUR));
}

/// Bars an hour apart inside one session are no gap. The first bar of the next day follows a close,
/// and so does the first after a Thursday holiday; the second bar after the open is no gap again.
#[test]
fn the_first_bar_after_a_closure_is_a_gap() {
    let schedule = exchange();
    let mut tracker = GapTracker::new();
    assert!(!tracker.on_bar(&schedule, MONDAY + 15 * HOUR, HOUR), "the first bar seen");
    assert!(!tracker.on_bar(&schedule, MONDAY + 16 * HOUR, HOUR));
    assert!(!tracker.on_bar(&schedule, MONDAY + 20 * HOUR, HOUR));
    assert!(tracker.on_bar(&schedule, MONDAY + DAY + 15 * HOUR, HOUR), "Tuesday after Monday's close");
    assert!(!tracker.on_bar(&schedule, MONDAY + DAY + 16 * HOUR, HOUR));
}

/// A session that opens partway through a bar: with 14:30 opens and hourly bars, the 14:00 bar is the gap,
/// and the 15:00 bar after it is not; with four-hour bars the bar spanning the open is the only gap.
#[test]
fn a_closure_is_counted_once_by_the_bar_that_spans_it() {
    let schedule = exchange();
    let mut hourly = GapTracker::new();
    hourly.on_bar(&schedule, MONDAY + 19 * HOUR, HOUR);
    assert!(hourly.on_bar(&schedule, MONDAY + DAY + 14 * HOUR, HOUR));
    assert!(!hourly.on_bar(&schedule, MONDAY + DAY + 15 * HOUR, HOUR));
    assert!(!hourly.on_bar(&schedule, MONDAY + DAY + 20 * HOUR, HOUR), "the last bar of the session");
    let mut four = GapTracker::new();
    four.on_bar(&schedule, MONDAY + 12 * HOUR, 4 * HOUR);
    assert!(four.on_bar(&schedule, MONDAY + DAY + 12 * HOUR, 4 * HOUR));
    assert!(!four.on_bar(&schedule, MONDAY + DAY + 16 * HOUR, 4 * HOUR));
}

#[test]
fn a_holiday_and_a_weekend_gap() {
    let schedule = exchange();
    let mut tracker = GapTracker::new();
    tracker.on_bar(&schedule, MONDAY + 2 * DAY + 20 * HOUR, HOUR);
    assert!(tracker.on_bar(&schedule, MONDAY + 4 * DAY + 15 * HOUR, HOUR), "Friday after the Thursday holiday");
    assert!(!tracker.on_bar(&schedule, MONDAY + 4 * DAY + 16 * HOUR, HOUR));
    assert!(tracker.on_bar(&schedule, MONDAY + 7 * DAY + 15 * HOUR, HOUR), "Monday after the weekend");
}

#[test]
fn a_long_silence_is_a_gap_without_a_scan() {
    let mut tracker = GapTracker::new();
    tracker.on_bar(&Schedule::AlwaysOpen, MONDAY, HOUR);
    assert!(!tracker.on_bar(&Schedule::AlwaysOpen, MONDAY + MAX_SCAN_MS, HOUR));
    assert!(tracker.on_bar(&Schedule::AlwaysOpen, MONDAY + 2 * MAX_SCAN_MS + 1, HOUR));
}
