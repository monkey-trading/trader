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

/// Which bar lengths the exchange's 14:30 to 21:00 session can hold: bars up to four hours can (the
/// 16:00 four-hour bar follows one that saw the open at 14:30), while six, twelve and twenty-four hours
/// cannot, whatever the alignment. An always-open market holds any.
#[test]
fn bar_lengths_a_session_can_hold() {
    let schedule = exchange();
    let minutes = |n: Millis| n * 60_000;
    for fits in [1, 5, 15, 30, 60, 144, 180, 240] {
        assert!(bar_can_trade(&schedule, minutes(fits), MONDAY), "{fits} minutes");
    }
    for never in [360, 720, 1440] {
        assert!(!bar_can_trade(&schedule, minutes(never), MONDAY), "{never} minutes");
    }
    assert!(bar_can_trade(&Schedule::AlwaysOpen, minutes(1440), MONDAY));
    assert!(!bar_can_trade(&schedule, 0, MONDAY));
}

/// The answer does not depend on the week it is asked in. A market whose only session of one week is
/// closed by a dated closure holds an hour-long bar in every week; five-hour bars on the exchange's
/// session line up with it only on epoch days that are a multiple of five, which some weeks lack.
#[test]
fn the_answer_is_the_same_in_every_week_it_is_asked_in() {
    let windows = serde_json::json!([{ "open": "Mon 09:00", "close": "Mon 17:00" }]);
    let closures = serde_json::json!([{ "from": "2026-11-02T09:00:00", "to": "2026-11-02T17:00:00" }]);
    let schedule = serde_json::json!({ "kind": "weekly", "tz": "UTC", "windows": windows, "closures": closures });
    let schedule: Schedule = serde_json::from_value(schedule).unwrap();
    let exchange = exchange();
    for week in 0..10 {
        let from = MONDAY + week * 7 * DAY;
        assert!(bar_can_trade(&schedule, HOUR, from), "an hour, week {week}");
        assert!(bar_can_trade(&exchange, 5 * HOUR, from), "five hours, week {week}");
    }
}

/// 09:30 to 16:00 in New York with 288-minute bars: the bars start at 00:00, 04:48, 09:36, 14:24 and 19:12
/// UTC, and one holds the session only while the clocks are on daylight saving time (13:30 to 20:00 UTC).
/// Daylight saving time lasts longer than 26 weeks, so a search from its first weeks reaches the winter
/// only in its last span; one from the winter reaches the summer in its first.
#[test]
fn a_length_that_fits_one_clock_regime_is_refused_wherever_the_search_starts() {
    let windows: Vec<_> = ["Mon", "Tue", "Wed", "Thu", "Fri"]
        .iter()
        .map(|day| serde_json::json!({ "open": format!("{day} 09:30"), "close": format!("{day} 16:00") }))
        .collect();
    let schedule = serde_json::json!({ "kind": "weekly", "tz": "America/New_York", "windows": windows });
    let schedule: Schedule = serde_json::from_value(schedule).unwrap();
    let minutes = |n: Millis| n * 60_000;
    // 2026-03-09 is the Monday after the clocks went forward; 2026-11-02 the Monday after they went back.
    for from in [1_773_014_400_000, 1_785_000_000_000, 1_793_577_600_000, 1_800_000_000_000] {
        assert!(!bar_can_trade(&schedule, minutes(288), from), "from {from}");
        assert!(bar_can_trade(&schedule, minutes(60), from), "an hour, from {from}");
    }
}
