use super::*;
use crate::margin::tests::{fx, spec};

fn fixed(notional: &str, leverage: &str) -> Sizing {
    Sizing::FixedNotional { notional: fx(notional), max_leverage: fx(leverage) }
}

fn fraction(bps: i64, leverage: &str) -> Sizing {
    Sizing::EquityFraction { bps, max_leverage: fx(leverage) }
}

/// A fixed notional of 1,000 at a mark of 40,000 is 0.025 units, and the units round down: at 3,000 it
/// is 0.333333333, whose notional is a nano-unit short of 1,000 rather than over.
#[test]
fn a_fixed_notional_in_units() {
    assert_eq!(fixed("1000", "10").qty(fx("40000"), fx("5000")), fx("0.025"));
    assert_eq!(fixed("1000", "10").qty(fx("3000"), fx("5000")), fx("0.333333333"));
}

/// A fraction takes its share of equity: 20% of 5,000 is 1,000; 12.5% (1,250 bps) of 4,000 is 500.
#[test]
fn a_fraction_of_equity() {
    assert_eq!(fraction(2_000, "10").qty(fx("40000"), fx("5000")), fx("0.025"));
    assert_eq!(fraction(1_250, "10").qty(fx("50"), fx("4000")), fx("10"));
}

/// The leverage cap cuts either method: 3x of 1,000 equity is 3,000, below a notional of 10,000 and a
/// fraction of 500%.
#[test]
fn the_leverage_cap_cuts_the_position() {
    assert_eq!(fixed("10000", "3").qty(fx("100"), fx("1000")), fx("30"));
    assert_eq!(fraction(50_000, "3").qty(fx("100"), fx("1000")), fx("30"));
    assert_eq!(fixed("2000", "3").qty(fx("100"), fx("1000")), fx("20"));
    assert_eq!(fixed("10000", "0.5").qty(fx("100"), fx("1000")), fx("5"));
}

/// No equity or no mark means no position, and a huge position is cut to the order bound.
#[test]
fn degenerate_inputs_size_to_zero_or_the_bound() {
    assert_eq!(fixed("1000", "10").qty(fx("100"), Fixed::ZERO), Fixed::ZERO);
    assert_eq!(fixed("1000", "10").qty(fx("100"), fx("-5")), Fixed::ZERO);
    assert_eq!(fixed("1000", "10").qty(Fixed::ZERO, fx("1000")), Fixed::ZERO);
    let rich = Fixed::MAX_INPUT;
    assert_eq!(fixed("1000000000", "1000").qty(Fixed::from_raw(1), rich), Fixed::MAX_INPUT);
}

/// The section is checked against itself and its market: the market's 100x (1% first tier) bounds the
/// cap strictly, and a market asking no margin bounds nothing.
#[test]
fn validation() {
    let market = spec(1);
    assert_eq!(fixed("1000", "5").validate(&market), Ok(()));
    assert_eq!(fixed("1000", "99.999999999").validate(&market), Ok(()));
    assert_eq!(
        fixed("1000", "100").validate(&market),
        Err(SizingError::AboveMarket { cap: fx("100"), market: fx("100") })
    );
    assert_eq!(fixed("0", "5").validate(&market), Err(SizingError::Notional));
    assert_eq!(fixed("1000000001", "5").validate(&market), Err(SizingError::Notional));
    assert_eq!(fraction(0, "5").validate(&market), Err(SizingError::Fraction));
    assert_eq!(fraction(MAX_FRACTION_BPS + 1, "5").validate(&market), Err(SizingError::Fraction));
    assert_eq!(fraction(MAX_FRACTION_BPS, "5").validate(&market), Ok(()));
    assert_eq!(fixed("1000", "0").validate(&market), Err(SizingError::Leverage));
    assert_eq!(fixed("1000", "1001").validate(&market), Err(SizingError::Leverage));
    let mut free = spec(2);
    free.margin_tiers[0].bps = 0;
    assert_eq!(market_leverage(&free), None);
    assert_eq!(fixed("1000", "1000").validate(&free), Ok(()));
}

/// A section names its method and refuses what it does not know.
#[test]
fn the_config_section() {
    let parse = |text: &str| toml::from_str::<Sizing>(text);
    assert_eq!(parse("by = 'fixed_notional'\nnotional = '250'\nmax_leverage = 5").unwrap(), fixed("250", "5"));
    assert_eq!(parse("by = 'equity_fraction'\nbps = 500\nmax_leverage = '2.5'").unwrap(), fraction(500, "2.5"));
    assert!(parse("by = 'equity_fraction'\nbps = 500").is_err());
    assert!(parse("by = 'fixed_notional'\nnotional = '250'\nmax_leverage = 5\nbps = 5").is_err());
    assert!(parse("by = 'kelly'\nmax_leverage = 5").is_err());
}
