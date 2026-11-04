//! The SMA crossover driven through the crate's public API: marks into bars, bars into targets,
//! targets into orders, against answers worked out by hand.

use engine_core::Fixed;
use engine_core::types::{Millis, Side};
use trader_core::bars::BarBuilder;
use trader_core::strategy::{OrderIntent, ParamsError, SmaCrossover, Target, order_for};

const MINUTE: Millis = 60_000;

fn fx(text: &str) -> Fixed {
    text.parse().unwrap()
}

/// Fast 2 over slow 3 on closes 10, 11, 12, 11, 10, 10, 13: the slow average is full from the third
/// close. Third: fast 11.5 > slow 11, long. Fourth: 11.5 > 11.333…, long. Fifth: 10.5 < 11, short.
/// Sixth: 10 < 10.333…, short. Seventh: 11.5 > 11, long.
#[test]
fn targets_follow_the_averages() {
    let mut strategy = SmaCrossover::new(2, 3).unwrap();
    let targets: Vec<_> =
        ["10", "11", "12", "11", "10", "10", "13"].into_iter().map(|c| strategy.on_close(fx(c))).collect();
    use Target::*;
    assert_eq!(targets, [None, None, Some(Long), Some(Long), Some(Short), Some(Short), Some(Long)]);
    assert_eq!(strategy.averages(), (Some(fx("11.5")), Some(fx("11"))));
}

/// Equal averages mean flat: closes 5, 5, 5 give fast 5 and slow 5.
#[test]
fn equal_averages_are_flat() {
    let mut strategy = SmaCrossover::new(1, 3).unwrap();
    let last = ["5", "5", "5"].into_iter().map(|c| strategy.on_close(fx(c))).last().unwrap();
    assert_eq!(last, Some(Target::Flat));
}

/// The comparison is exact: fast 1 over slow 3 on 1, 1, 1.000000001 has a slow average that
/// truncates to 1.000000000 but is above 1, so the fast close above it is long.
#[test]
fn compares_without_rounding() {
    let mut strategy = SmaCrossover::new(1, 3).unwrap();
    let last = ["1", "1", "1.000000001"].into_iter().map(|c| strategy.on_close(fx(c))).last().unwrap();
    assert_eq!(strategy.averages().1, Some(fx("1")));
    assert_eq!(last, Some(Target::Long));
}

#[test]
fn periods_are_validated() {
    assert_eq!(SmaCrossover::new(0, 3).err(), Some(ParamsError::Periods));
    assert_eq!(SmaCrossover::new(3, 3).err(), Some(ParamsError::Periods));
    assert_eq!(SmaCrossover::new(4, 3).err(), Some(ParamsError::Periods));
    assert_eq!(SmaCrossover::new(1, 10_001).err(), Some(ParamsError::Periods));
    assert!(SmaCrossover::new(9_999, 10_000).is_ok());
}

/// Every move between the three targets, from flat, long and short positions of the size and of
/// another size, with the order it needs.
#[test]
fn orders_move_the_position_to_its_target() {
    let size = fx("0.5");
    let order = |side, qty: &str, reduce_only| Some(OrderIntent { side, qty: fx(qty), reduce_only });
    let cases = [
        (Target::Long, "0", order(Side::Buy, "0.5", false)),
        (Target::Short, "0", order(Side::Sell, "0.5", false)),
        (Target::Flat, "0", None),
        (Target::Long, "0.5", None),
        (Target::Short, "0.5", order(Side::Sell, "1", false)),
        (Target::Flat, "0.5", order(Side::Sell, "0.5", true)),
        (Target::Long, "-0.5", order(Side::Buy, "1", false)),
        (Target::Short, "-0.5", None),
        (Target::Flat, "-0.5", order(Side::Buy, "0.5", true)),
        (Target::Long, "0.75", order(Side::Sell, "0.25", true)),
        (Target::Long, "0.25", order(Side::Buy, "0.25", false)),
        (Target::Short, "-0.75", order(Side::Buy, "0.25", true)),
    ];
    for (target, position, expected) in cases {
        assert_eq!(order_for(target, size, fx(position)), expected, "{target:?} from {position}");
    }
}

/// A recorded run: one mark per command, three per minute, through one-minute bars into a 1-over-2
/// crossover. The minute closes are 100.2, 100.5, 100.1, 100.4, so the targets are long (100.5 >
/// 100.35), short (100.1 < 100.3), and long (100.4 > 100.25), and the orders from flat buy 1,
/// sell 2, and buy 2.
#[test]
fn marks_to_orders() {
    let marks = [
        (0, "100"),
        (20_000, "100.1"),
        (40_000, "100.2"),
        (MINUTE, "100.4"),
        (MINUTE + 30_000, "100.5"),
        (2 * MINUTE, "100.3"),
        (2 * MINUTE + 59_999, "100.1"),
        (3 * MINUTE + 1, "100.4"),
        (4 * MINUTE, "100.4"),
    ];
    let mut bars = BarBuilder::new(MINUTE, 0);
    let mut strategy = SmaCrossover::new(1, 2).unwrap();
    let mut position = Fixed::ZERO;
    let mut orders = Vec::new();
    for (ts, mark) in marks {
        let Some(bar) = bars.on_command(ts, Some(fx(mark))) else { continue };
        let Some(target) = strategy.on_close(bar.close) else { continue };
        if let Some(order) = order_for(target, Fixed::ONE, position) {
            position += if order.side == Side::Buy { order.qty } else { -order.qty };
            orders.push((bar.start, order.side, order.qty));
        }
    }
    assert_eq!(
        orders,
        [(MINUTE, Side::Buy, fx("1")), (2 * MINUTE, Side::Sell, fx("2")), (3 * MINUTE, Side::Buy, fx("2"))]
    );
}
