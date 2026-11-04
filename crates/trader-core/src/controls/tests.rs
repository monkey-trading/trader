use std::time::Duration;

use super::*;
use crate::hours::tests::{DAY, HOUR, MONDAY, exchange};
use crate::margin::tests::{fx, spec};
use crate::risk::LossLimit;

const TS: Millis = MONDAY + 15 * HOUR;

fn controls(buffer_bps: i64, min_hold: Duration) -> Controls {
    let risk = RiskParams {
        max_open_positions: 2,
        max_total_notional: fx("5000"),
        daily_loss: LossLimit::Usd(fx("100")),
        margin_buffer_bps: buffer_bps,
        min_hold,
    };
    Controls::new(risk, Sizing::FixedNotional { notional: fx("1000"), max_leverage: fx("90") }, None).unwrap()
}

fn holding<'a>(spec: &'a MarketSpec, qty: &str, entry: &str, mark: &str) -> Holding<'a> {
    Holding { market: spec.id, qty: fx(qty), entry: fx(entry), mark: fx(mark), spec }
}

fn bar<'a>(spec: &'a MarketSpec, mark: &str, target: Target) -> BarTarget<'a> {
    BarTarget { ts: TS, market: spec.id, spec, mark: fx(mark), target, gap: false, close_first: false }
}

fn order(side: Side, qty: &str, reduce_only: bool) -> Decision {
    Decision::Order(OrderIntent { side, qty: fx(qty), reduce_only })
}

fn account<'a>(balance: &str, holdings: &'a [Holding<'a>]) -> Account<'a> {
    Account { balance: fx(balance), holdings }
}

/// A flat account sizes a new position by the sizing: 1,000 USD at a mark of 100 is 10 units, long or
/// short; the flat target at no position is already met.
#[test]
fn opens_a_sized_position() {
    let market = spec(1);
    let mut controls = controls(10_000, Duration::ZERO);
    let flat = account("5000", &[]);
    assert_eq!(controls.decide(&flat, &bar(&market, "100", Target::Long)), order(Side::Buy, "10", false));
    assert_eq!(controls.decide(&flat, &bar(&market, "100", Target::Short)), order(Side::Sell, "10", false));
    assert_eq!(controls.decide(&flat, &bar(&market, "100", Target::Flat)), Decision::AtTarget);
}

/// A position on the target's side is kept at its size, whatever equity has done to what sizing
/// would give now: a long of 4 with a target of long places nothing, with sizing saying 10.
#[test]
fn a_position_on_the_target_side_is_kept() {
    let market = spec(1);
    let mut controls = controls(10_000, Duration::ZERO);
    let held = [holding(&market, "4", "100", "100")];
    assert_eq!(controls.decide(&account("5000", &held), &bar(&market, "100", Target::Long)), Decision::AtTarget);
}

/// Closing and reversing follow the target: a long of 4 goes flat with a reduce-only sell, and reverses
/// with a sell of the 4 plus a new 10.
#[test]
fn closes_and_reverses() {
    let market = spec(1);
    let mut controls = controls(10_000, Duration::ZERO);
    let held = [holding(&market, "4", "100", "100")];
    let acct = account("5000", &held);
    assert_eq!(controls.decide(&acct, &bar(&market, "100", Target::Flat)), order(Side::Sell, "4", true));
    assert_eq!(controls.decide(&acct, &bar(&market, "100", Target::Short)), order(Side::Sell, "14", false));
    let close_first = BarTarget { close_first: true, ..bar(&market, "100", Target::Short) };
    assert_eq!(controls.decide(&acct, &close_first), order(Side::Sell, "4", true));
}

/// A position closes only once it has been held for the minimum: opened at TS, held an hour, it may
/// be closed from TS + 1h exactly. Reversing it is refused the same, and adding to it is not.
#[test]
fn a_position_is_held_for_the_minimum() {
    let market = spec(1);
    let mut controls = controls(10_000, Duration::from_secs(3600));
    controls.on_fill(TS, 1, Fixed::ZERO, fx("4"));
    let held = [holding(&market, "4", "100", "100")];
    let acct = account("5000", &held);
    let at = |ts, target| BarTarget { ts, ..bar(&market, "100", target) };
    let refused = Decision::Refused(Refusal::MinHold { until: TS + HOUR });
    assert_eq!(controls.decide(&acct, &at(TS + HOUR - 1, Target::Flat)), refused);
    assert_eq!(controls.decide(&acct, &at(TS + HOUR - 1, Target::Short)), refused);
    assert_eq!(controls.decide(&acct, &at(TS + HOUR, Target::Flat)), order(Side::Sell, "4", true));
    // A fill that only adds keeps the opening time; one that reverses restarts it.
    controls.on_fill(TS + 10, 1, fx("4"), fx("6"));
    assert_eq!(controls.decide(&acct, &at(TS + HOUR, Target::Flat)), order(Side::Sell, "4", true));
    controls.on_fill(TS + 20, 1, fx("6"), fx("-3"));
    let short = [holding(&market, "-3", "100", "100")];
    assert_eq!(
        controls.decide(&account("5000", &short), &at(TS + HOUR + 19, Target::Long)),
        Decision::Refused(Refusal::MinHold { until: TS + 20 + HOUR })
    );
    // Back to flat forgets it: the next position starts its own hold.
    controls.on_fill(TS + 30, 1, fx("-3"), Fixed::ZERO);
    controls.on_fill(TS + 40, 1, Fixed::ZERO, fx("2"));
    let held = [holding(&market, "2", "100", "100")];
    assert_eq!(
        controls.decide(&account("5000", &held), &at(TS + 41, Target::Flat)),
        Decision::Refused(Refusal::MinHold { until: TS + 40 + HOUR })
    );
}

/// A position the bot did not open this run, found at startup, may be closed at once.
#[test]
fn a_position_found_at_startup_counts_as_held() {
    let market = spec(1);
    let mut controls = controls(10_000, Duration::from_secs(3600));
    let held = [holding(&market, "4", "100", "100")];
    assert_eq!(
        controls.decide(&account("5000", &held), &bar(&market, "100", Target::Flat)),
        order(Side::Sell, "4", true)
    );
    controls.on_fill(TS, 1, Fixed::ZERO, fx("4"));
    controls.forget_opening(1);
    assert_eq!(
        controls.decide(&account("5000", &held), &bar(&market, "100", Target::Flat)),
        order(Side::Sell, "4", true)
    );
}

/// A market shut at the bar's time gets no order, to open or to close; the first bar after a closure
/// gets none either, and the next does.
#[test]
fn no_order_while_the_market_is_shut_or_the_bar_is_a_gap() {
    let mut market = spec(1);
    market.schedule = exchange();
    let mut controls = controls(10_000, Duration::ZERO);
    let held = [holding(&market, "4", "100", "100")];
    let closed = BarTarget { ts: MONDAY + 3 * HOUR, ..bar(&market, "100", Target::Flat) };
    assert_eq!(controls.decide(&account("5000", &held), &closed), Decision::Refused(Refusal::MarketClosed));
    assert_eq!(
        controls
            .decide(&account("5000", &[]), &BarTarget { ts: MONDAY + 21 * HOUR, ..bar(&market, "100", Target::Long) }),
        Decision::Refused(Refusal::MarketClosed)
    );
    let gap = BarTarget { gap: true, ..bar(&market, "100", Target::Flat) };
    assert_eq!(controls.decide(&account("5000", &held), &gap), Decision::Refused(Refusal::GapBar));
    assert_eq!(
        controls.decide(&account("5000", &held), &bar(&market, "100", Target::Flat)),
        order(Side::Sell, "4", true)
    );
}

/// A loss of the limit from the day's first reading flattens an account with positions, and stops one
/// without any; the next UTC day trades again. A reading between bars, from `observe`, trips it too.
#[test]
fn the_daily_loss_limit_flattens_and_stops_until_the_next_day() {
    let market = spec(1);
    let mut controls = controls(10_000, Duration::ZERO);
    let held = [holding(&market, "10", "100", "100")];
    // The day opens at 1,000 equity.
    assert_eq!(controls.decide(&account("1000", &held), &bar(&market, "100", Target::Long)), Decision::AtTarget);
    // The mark falls to 90: equity 900, a loss of 100. Between bars the reading trips it.
    let down = [holding(&market, "10", "100", "90")];
    assert_eq!(controls.observe(TS + 1, &account("1000", &down)), Some(DayStatus::Stopped { newly: true }));
    assert_eq!(controls.decide(&account("1000", &down), &bar(&market, "90", Target::Long)), Decision::Flatten);
    assert_eq!(controls.decide(&account("1000", &down), &bar(&market, "90", Target::Short)), Decision::Flatten);
    // Flat afterwards at 900 cash: still stopped for the day.
    assert_eq!(
        controls.decide(&account("900", &[]), &bar(&market, "90", Target::Long)),
        Decision::Refused(Refusal::DailyLossStopped)
    );
    // The next day's first reading opens a new day.
    let next = BarTarget { ts: TS + DAY, ..bar(&market, "90", Target::Long) };
    assert_eq!(controls.decide(&account("900", &[]), &next), order(Side::Buy, "11.111111111", false));
}

/// Positions across markets are flattened together: a stop with a position in either market flattens.
#[test]
fn the_stop_sees_every_market() {
    let (one, two) = (spec(1), spec(2));
    let mut controls = controls(10_000, Duration::ZERO);
    let held = [holding(&one, "10", "100", "100"), holding(&two, "-1", "50", "50")];
    controls.observe(TS, &account("1000", &held));
    let down = [holding(&one, "10", "100", "100"), holding(&two, "-1", "50", "160")];
    assert_eq!(controls.decide(&account("1000", &down), &bar(&one, "100", Target::Flat)), Decision::Flatten);
}

/// At most `max_open_positions` positions: with two open, a third market gets no order, a reversal in
/// an open one does, and a close always does.
#[test]
fn the_position_count_is_limited() {
    let (one, two, three) = (spec(1), spec(2), spec(3));
    let mut controls = controls(10_000, Duration::ZERO);
    let held = [holding(&one, "1", "100", "100"), holding(&two, "-1", "100", "100")];
    let acct = account("5000", &held);
    assert_eq!(controls.decide(&acct, &bar(&three, "100", Target::Long)), Decision::Refused(Refusal::TooManyPositions));
    assert_eq!(controls.decide(&acct, &bar(&one, "100", Target::Short)), order(Side::Sell, "11", false));
    assert_eq!(controls.decide(&acct, &bar(&one, "100", Target::Flat)), order(Side::Sell, "1", true));
    let one_held = [held[0]];
    assert_eq!(
        controls.decide(&account("5000", &one_held), &bar(&three, "100", Target::Long)),
        order(Side::Buy, "10", false)
    );
}

/// Total notional is limited: with 4,500 open of 5,000, a new 1,000 is refused and a new 500 would not
/// be (sizing 1,000 is the only size here, so the limit is lowered through the position's mark).
#[test]
fn the_total_notional_is_limited() {
    let (one, two) = (spec(1), spec(2));
    let mut controls = controls(0, Duration::ZERO);
    let held = [holding(&one, "45", "100", "100")];
    assert_eq!(
        controls.decide(&account("5000", &held), &bar(&two, "100", Target::Long)),
        Decision::Refused(Refusal::TotalNotional)
    );
    let held = [holding(&one, "40", "100", "100")];
    assert_eq!(
        controls.decide(&account("5000", &held), &bar(&two, "100", Target::Long)),
        order(Side::Buy, "10", false)
    );
    let held = [holding(&one, "40.000000001", "100", "100")];
    assert_eq!(
        controls.decide(&account("5000", &held), &bar(&two, "100", Target::Long)),
        Decision::Refused(Refusal::TotalNotional)
    );
}

/// The margin buffer: a 1,000 USD position needs 10 of margin at 1%, so a buffer of 100% needs 20 of
/// equity. 20 passes and a nano-USD less is refused; the engine alone would refuse below 10. The leverage
/// cap is below the market's, so at no buffer the cap binds before the margin check could.
#[test]
fn the_margin_buffer_is_kept() {
    let market = spec(1);
    let mut controls = controls(10_000, Duration::ZERO);
    assert_eq!(controls.decide(&account("20", &[]), &bar(&market, "100", Target::Long)), order(Side::Buy, "10", false));
    assert_eq!(
        controls.decide(&account("19.999999999", &[]), &bar(&market, "100", Target::Long)),
        Decision::Refused(Refusal::Margin { equity: fx("19.999999999"), needed: fx("20") })
    );
    // A smaller buffer moves the boundary: 25% needs 12.5.
    let mut smaller = self::controls(2_500, Duration::ZERO);
    assert_eq!(
        smaller.decide(&account("12.5", &[]), &bar(&market, "100", Target::Long)),
        order(Side::Buy, "10", false)
    );
    assert_eq!(
        smaller.decide(&account("12.499999999", &[]), &bar(&market, "100", Target::Long)),
        Decision::Refused(Refusal::Margin { equity: fx("12.499999999"), needed: fx("12.5") })
    );
}

/// Margin is shared: the requirement counts the account's other positions, and the buffer is judged on
/// the whole. 500 of existing notional needs 5, the new 1,000 needs 10, so 30 at a 100% buffer.
#[test]
fn the_buffer_counts_every_position() {
    let (one, two) = (spec(1), spec(2));
    let mut controls = controls(10_000, Duration::ZERO);
    let held = [holding(&one, "5", "100", "100")];
    assert_eq!(controls.decide(&account("30", &held), &bar(&two, "100", Target::Long)), order(Side::Buy, "10", false));
    assert_eq!(
        controls.decide(&account("29.999999999", &held), &bar(&two, "100", Target::Long)),
        Decision::Refused(Refusal::Margin { equity: fx("29.999999999"), needed: fx("30") })
    );
}

/// Cash must be positive to add exposure, though equity is not: unrealised profit of 50 on a balance of
/// zero or below does not open anything. An order that only reduces is never held back by cash.
#[test]
fn cash_stays_positive() {
    let (one, two) = (spec(1), spec(2));
    let mut controls = controls(0, Duration::ZERO);
    let held = [holding(&one, "1", "50", "100")];
    assert_eq!(
        controls.decide(&account("0", &held), &bar(&two, "100", Target::Long)),
        Decision::Refused(Refusal::NoCash)
    );
    assert_eq!(
        controls.decide(&account("-5", &held), &bar(&one, "100", Target::Short)),
        Decision::Refused(Refusal::NoCash)
    );
    assert_eq!(controls.decide(&account("-5", &held), &bar(&one, "100", Target::Flat)), order(Side::Sell, "1", true));
    assert_eq!(
        controls.decide(&account("0.000000001", &held), &bar(&two, "100", Target::Long)),
        order(Side::Buy, "10", false)
    );
}

/// No equity means no size: nothing to open from flat, and a long is closed rather than reversed.
#[test]
fn no_equity_opens_nothing_and_closes_what_is_held() {
    let market = spec(1);
    let mut controls = controls(0, Duration::ZERO);
    assert_eq!(
        controls.decide(&account("0", &[]), &bar(&market, "100", Target::Long)),
        Decision::Refused(Refusal::NoSize)
    );
    let broke = [holding(&market, "1", "200", "100")];
    assert_eq!(
        controls.decide(&account("50", &broke), &bar(&market, "100", Target::Short)),
        order(Side::Sell, "1", true)
    );
}

/// An order below the market's minimum notional is refused when it adds exposure, not when it closes.
#[test]
fn a_minimum_order_notional() {
    let mut market = spec(1);
    market.min_order_notional = fx("1001");
    let mut controls = controls(0, Duration::ZERO);
    assert_eq!(
        controls.decide(&account("5000", &[]), &bar(&market, "100", Target::Long)),
        Decision::Refused(Refusal::BelowMinimum)
    );
    let held = [holding(&market, "1", "100", "100")];
    assert_eq!(
        controls.decide(&account("5000", &held), &bar(&market, "100", Target::Flat)),
        order(Side::Sell, "1", true)
    );
}

/// A holding in a market quoted outside USD cannot be valued, so no order is placed.
#[test]
fn an_unvalued_holding_gives_no_order() {
    let mut dax = spec(3);
    dax.quote_currency = engine_core::market::QuoteCurrency::Eur { fx_market: 9 };
    let market = spec(1);
    let mut controls = controls(0, Duration::ZERO);
    let held = [holding(&dax, "1", "100", "100")];
    assert_eq!(
        controls.decide(&account("5000", &held), &bar(&market, "100", Target::Long)),
        Decision::Refused(Refusal::Unvalued)
    );
    assert_eq!(controls.observe(TS, &account("5000", &held)), None);
}

/// The sizing is checked against each market the account trades, and the risk section on construction.
#[test]
fn construction_and_the_market_check() {
    let mut risk = RiskParams {
        max_open_positions: 0,
        max_total_notional: fx("5000"),
        daily_loss: LossLimit::Usd(fx("100")),
        margin_buffer_bps: 0,
        min_hold: Duration::ZERO,
    };
    let sizing = Sizing::FixedNotional { notional: fx("1000"), max_leverage: fx("100") };
    assert_eq!(Controls::new(risk.clone(), sizing.clone(), None).err(), Some(RiskError::OpenPositions));
    risk.max_open_positions = 1;
    let controls = Controls::new(risk, sizing, None).unwrap();
    assert!(matches!(controls.check_market(&spec(1)), Err(SizingError::AboveMarket { .. })));
}
