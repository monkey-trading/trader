use super::*;
use crate::margin::tests::fx;

const DAY: Millis = 86_400_000;

use engine_core::types::Millis;

fn tracker(limit: LossLimit) -> DayLoss {
    DayLoss::new(limit, None)
}

/// A 50 USD limit on a day that opens at 1,000: 951 and 950.000000001 are inside it, 950 trips it
/// (the loss equals the limit), and a later recovery does not lift it.
#[test]
fn a_usd_limit_trips_at_the_limit_and_holds_for_the_day() {
    let mut day = tracker(LossLimit::Usd(fx("50")));
    assert_eq!(day.observe(10, fx("1000")), DayStatus::Trading);
    assert_eq!(day.observe(20, fx("950.000000001")), DayStatus::Trading);
    assert_eq!(day.observe(30, fx("950")), DayStatus::Stopped { newly: true });
    assert_eq!(day.observe(40, fx("900")), DayStatus::Stopped { newly: false });
    assert_eq!(day.observe(50, fx("1200")), DayStatus::Stopped { newly: false });
    assert!(day.is_stopped());
}

/// A share of the opening equity: 5% (500 bps) of 2,000 is 100, and the amount rounds down so a
/// loss of exactly the rounded-down amount trips it.
#[test]
fn a_share_limit_is_of_the_days_opening_equity() {
    let mut day = tracker(LossLimit::EquityBps(500));
    day.observe(0, fx("2000"));
    assert_eq!(day.observe(1, fx("1900.000000001")), DayStatus::Trading);
    assert_eq!(day.observe(2, fx("1900")), DayStatus::Stopped { newly: true });
}

/// The next UTC day starts afresh from its first reading; a day is midnight to midnight, and the
/// reading at the boundary belongs to the new day.
#[test]
fn a_new_day_lifts_the_stop() {
    let mut day = tracker(LossLimit::Usd(fx("50")));
    day.observe(DAY - 2, fx("1000"));
    assert_eq!(day.observe(DAY - 1, fx("900")), DayStatus::Stopped { newly: true });
    assert_eq!(day.observe(DAY, fx("900")), DayStatus::Trading);
    assert!(!day.is_stopped());
    assert_eq!(day.state(), Some(DayState { day: 1, start_equity: fx("900"), tripped: false }));
    assert_eq!(day.observe(DAY + 1, fx("849")), DayStatus::Stopped { newly: true });
}

/// A gain never trips the limit, and a limit that rounds to nothing is not tripped by no loss.
#[test]
fn only_a_loss_trips_it() {
    let mut day = tracker(LossLimit::EquityBps(1));
    day.observe(0, fx("0.000000001"));
    assert_eq!(day.observe(1, fx("0.000000001")), DayStatus::Trading);
    assert_eq!(day.observe(2, fx("5")), DayStatus::Trading);
    let mut broke = tracker(LossLimit::Usd(fx("1")));
    assert_eq!(broke.observe(0, fx("-5")), DayStatus::Trading);
    assert_eq!(broke.observe(1, fx("-6")), DayStatus::Stopped { newly: true });
}

/// A state restored from the current day resumes it, stopped included; one from an earlier day is
/// replaced by the first reading; a reading from before the restored day changes nothing.
#[test]
fn a_restored_state_resumes_the_same_day_only() {
    let stopped = DayState { day: 3, start_equity: fx("1000"), tripped: true };
    let mut day = DayLoss::new(LossLimit::Usd(fx("50")), Some(stopped));
    assert!(day.is_stopped());
    assert_eq!(day.observe(3 * DAY + 5, fx("1000")), DayStatus::Stopped { newly: false });
    assert_eq!(day.observe(2 * DAY, fx("1")), DayStatus::Stopped { newly: false });
    assert_eq!(day.state(), Some(stopped));
    assert_eq!(day.observe(4 * DAY, fx("700")), DayStatus::Trading);
    assert_eq!(day.state(), Some(DayState { day: 4, start_equity: fx("700"), tripped: false }));
}

#[test]
fn the_state_round_trips_through_json_as_decimal_strings() {
    let state = DayState { day: 20_000, start_equity: fx("1234.5"), tripped: true };
    let text = serde_json::to_string(&state).unwrap();
    assert_eq!(text, r#"{"day":20000,"start_equity":"1234.5","tripped":true}"#);
    assert_eq!(serde_json::from_str::<DayState>(&text).unwrap(), state);
}

fn params(text: &str) -> Result<RiskParams, toml::de::Error> {
    toml::from_str(text)
}

const BASE: &str = "max_open_positions = 3\nmax_total_notional = '5000'\ndaily_loss = { usd = '100' }\n";

#[test]
fn the_config_section_and_its_defaults() {
    let risk = params(BASE).unwrap();
    assert_eq!(risk.daily_loss, LossLimit::Usd(fx("100")));
    assert_eq!((risk.margin_buffer_bps, risk.min_hold), (10_000, Duration::from_secs(3600)));
    assert_eq!(risk.validate(), Ok(()));
    let tuned = params(&format!("{BASE}margin_buffer_bps = 2500\nmin_hold = '15m'")).unwrap();
    assert_eq!((tuned.margin_buffer_bps, tuned.min_hold), (2_500, Duration::from_secs(900)));
    assert_eq!(
        params("max_open_positions = 3\nmax_total_notional = '5'\ndaily_loss = { equity_bps = 300 }")
            .unwrap()
            .daily_loss,
        LossLimit::EquityBps(300)
    );
}

#[test]
fn a_limit_is_required_and_a_typo_is_refused() {
    assert!(params("max_total_notional = '5000'\ndaily_loss = { usd = '100' }").is_err());
    assert!(params("max_open_positions = 3\ndaily_loss = { usd = '100' }").is_err());
    assert!(params("max_open_positions = 3\nmax_total_notional = '5000'").is_err());
    assert!(params(&format!("{BASE}min_holds = '1h'")).is_err());
    assert!(params("max_open_positions = 3\nmax_total_notional = '5000'\ndaily_loss = { eur = '1' }").is_err());
}

#[test]
fn limits_out_of_range_are_refused() {
    let check = |extra: &str| params(&format!("{BASE}{extra}")).unwrap().validate();
    assert_eq!(check("margin_buffer_bps = -1"), Err(RiskError::MarginBuffer));
    assert_eq!(check("margin_buffer_bps = 100001"), Err(RiskError::MarginBuffer));
    assert_eq!(check("margin_buffer_bps = 100000"), Ok(()));
    assert_eq!(check("margin_buffer_bps = 0"), Ok(()));
    assert_eq!(check("min_hold = '169h'"), Err(RiskError::MinHold));
    assert_eq!(check("min_hold = '168h'"), Ok(()));
    assert_eq!(check("min_hold = '0s'"), Ok(()));
    let with = |positions: usize, notional: &str, loss: LossLimit| RiskParams {
        max_open_positions: positions,
        max_total_notional: fx(notional),
        daily_loss: loss,
        margin_buffer_bps: 0,
        min_hold: Duration::ZERO,
    };
    assert_eq!(with(0, "1", LossLimit::Usd(fx("1"))).validate(), Err(RiskError::OpenPositions));
    assert_eq!(with(1_001, "1", LossLimit::Usd(fx("1"))).validate(), Err(RiskError::OpenPositions));
    assert_eq!(with(1, "0", LossLimit::Usd(fx("1"))).validate(), Err(RiskError::TotalNotional));
    assert_eq!(with(1, "1", LossLimit::Usd(fx("0"))).validate(), Err(RiskError::DailyLoss));
    assert_eq!(with(1, "1", LossLimit::EquityBps(0)).validate(), Err(RiskError::DailyLoss));
    assert_eq!(with(1, "1", LossLimit::EquityBps(10_001)).validate(), Err(RiskError::DailyLoss));
    assert_eq!(with(1_000, "1", LossLimit::EquityBps(10_000)).validate(), Ok(()));
}
