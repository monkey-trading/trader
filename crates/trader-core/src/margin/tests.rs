use engine_core::market::{FinancingSpec, PriceRules, Tier};
use engine_core::session::Schedule;
use engine_core::types::AssetClass;

use super::*;

pub(crate) fn fx(text: &str) -> Fixed {
    text.parse().unwrap()
}

/// A USD crypto market with margin of 1% to 2,000 USD of notional and 5% above, so a 100x market
/// at small sizes.
pub(crate) fn spec(id: MarketId) -> MarketSpec {
    MarketSpec {
        id,
        symbol: format!("TEST{id}-USD"),
        asset_class: AssetClass::Crypto,
        quote_currency: QuoteCurrency::Usd,
        schedule: Schedule::AlwaysOpen,
        margin_tiers: vec![Tier { up_to: Some(fx("2000")), bps: 100 }, Tier { up_to: None, bps: 500 }],
        spread_tiers: vec![Tier { up_to: None, bps: 0 }],
        price_rules: PriceRules { max_age_ms: 1_000, max_divergence_bps: 25, min_sources: 2 },
        max_slippage_bps: 100,
        financing: FinancingSpec { benchmark: "SOFR".into(), markup_bps: 500, day_basis: 360 },
        house_cap_bps: 1_000,
        min_order_notional: Fixed::ZERO,
        house_cap_floor: Fixed::ZERO,
        contracts: Vec::new(),
    }
}

/// A long of 10 units entered at 100 and marked at 105 has 50 of profit and 1,050 of notional, which
/// is 10.5 of margin at 1%; a short of 1 entered at 3,000 and marked at 2,900 has 100 of profit and
/// 2,900 of notional, which is 20 at 1% on the first 2,000 and 45 at 5% on the next 900.
#[test]
fn equity_and_margin_follow_the_tiers() {
    let (btc, eth) = (spec(1), spec(2));
    let long = Holding { market: 1, qty: fx("10"), entry: fx("100"), mark: fx("105"), spec: &btc };
    let short = Holding { market: 2, qty: fx("-1"), entry: fx("3000"), mark: fx("2900"), spec: &eth };
    assert_eq!(equity_and_margin(fx("1000"), &[long]), Some((fx("1050"), fx("10.5"))));
    assert_eq!(equity_and_margin(fx("1000"), &[short]), Some((fx("1100"), fx("65"))));
    assert_eq!(equity_and_margin(fx("1000"), &[long, short]), Some((fx("1150"), fx("75.5"))));
    assert_eq!(equity_and_margin(fx("1000"), &[]), Some((fx("1000"), Fixed::ZERO)));
}

/// A loss lowers equity, and the notional and the requirement round up while the profit rounds down:
/// 0.000000001 of a unit at 1.5 has a notional of 0.000000002 and a long's loss of 0.000000001.
#[test]
fn rounding_goes_against_the_account() {
    let btc = spec(1);
    let tiny = Holding { market: 1, qty: fx("0.000000001"), entry: fx("2.5"), mark: fx("1.5"), spec: &btc };
    assert_eq!(tiny.notional(), fx("0.000000002"));
    assert_eq!(equity_and_margin(fx("10"), &[tiny]), Some((fx("9.999999999"), fx("0.000000001"))));
}

/// A holding in a market quoted outside USD is not valued: the conversion is the engine's.
#[test]
fn a_market_quoted_in_another_currency_gives_no_figures() {
    let mut dax = spec(3);
    dax.quote_currency = QuoteCurrency::Eur { fx_market: 9 };
    let holding = Holding { market: 3, qty: fx("1"), entry: fx("100"), mark: fx("100"), spec: &dax };
    assert_eq!(equity_and_margin(fx("1000"), &[holding]), None);
}
