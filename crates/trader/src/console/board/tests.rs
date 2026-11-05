use engine_core::account::Position;

use super::*;
use engine_core::market::QuoteCurrency;

use crate::node::api::Reach;

/// The node's figures pass through; unrealised profit is equity less balance, the margin level is
/// equity over the requirement in bps, flat positions drop, symbols and marks come from the market
/// list, and each position's close-out from the node's.
#[test]
fn figures_derive_from_the_nodes() {
    let account = AccountFigures {
        balance: fixed("1000"),
        equity: fixed("1250.5"),
        margin: fixed("500"),
        positions: BTreeMap::from([
            (1, Position { qty: fixed("-0.5"), entry: fixed("100") }),
            (2, Position { qty: Fixed::ZERO, entry: Fixed::ZERO }),
            (7, Position { qty: fixed("2"), entry: fixed("3") }),
        ]),
        orders: BTreeMap::new(),
        closeouts: vec![
            PositionCloseout { market: 1, reach: Reach::At, price: Some(fixed("2600")) },
            PositionCloseout { market: 7, reach: Reach::Beyond, price: None },
        ],
    };
    let entitlement = Entitlement { entitled: TokenUnits(5), newest_entitled: TokenUnits(9) };
    let figures = Figures::new(account, (1_000, 42), Some(entitlement), fixed("-3.25"), &symbols());
    assert_eq!(figures.unrealised_pnl, fixed("250.5"));
    assert_eq!(figures.margin_level_bps, Some(25_010));
    assert_eq!(figures.realised_pnl, fixed("-3.25"));
    assert_eq!(
        figures.positions,
        [
            PositionView {
                market: 1,
                symbol: Some("BTC-USD".into()),
                qty: fixed("-0.5"),
                entry: fixed("100"),
                mark: Some(fixed("99")),
                unrealised_pnl: Some(fixed("0.5")),
                closeout: Some(Closeout { reach: Reach::At, price: Some(fixed("2600")) }),
            },
            PositionView {
                market: 7,
                symbol: None,
                qty: fixed("2"),
                entry: fixed("3"),
                mark: None,
                unrealised_pnl: None,
                closeout: Some(Closeout { reach: Reach::Beyond, price: None }),
            },
        ]
    );
    assert_eq!(figures.monkey, Monkey { entitled: TokenUnits(5), newest_entitled: TokenUnits(9), claimed: None });
    assert_eq!((figures.time_ms, figures.head_seq), (1_000, 42));
}

/// A position's unrealised profit is the engine's: at the mark, rounded down, in USD at the FX index
/// for a market quoted in another currency, and none without a mark or a positive index.
#[test]
fn unrealised_profit_is_the_engines() {
    let long = Position { qty: fixed("3"), entry: fixed("1.1") };
    let listing = |quote, mark: Option<&str>, fx: Option<&str>| Listing {
        symbol: "X".into(),
        quote,
        mark: mark.map(fixed),
        fx_mark: fx.map(fixed),
    };
    let eur = QuoteCurrency::Eur { fx_market: 9 };
    let jpy = QuoteCurrency::Jpy { fx_market: 9 };
    for (listing, expected) in [
        (listing(QuoteCurrency::Usd, Some("1.2"), None), Some("0.3")),
        (listing(QuoteCurrency::Usd, None, None), None),
        (listing(eur, Some("1.2"), Some("1.5")), Some("0.45")),
        (listing(jpy, Some("1.2"), Some("150")), Some("0.002")),
        (listing(eur, Some("1.2"), None), None),
        (listing(eur, Some("1.2"), Some("0")), None),
    ] {
        assert_eq!(unrealised(&long, &listing), expected.map(fixed), "{listing:?}");
    }
    // Converting at a yen index rounds down: a gain of a third keeps its floor, a loss of a third
    // goes a billionth further, against the holder.
    let at_three = listing(jpy, Some("2"), Some("3"));
    let unit = |qty: &str| Position { qty: fixed(qty), entry: fixed("1") };
    assert_eq!(unrealised(&unit("1"), &at_three), Some(fixed("0.333333333")));
    assert_eq!(unrealised(&unit("-1"), &at_three), Some(fixed("-0.333333334")));
}

/// With nothing required there is no margin level, and with no entitlement yet MONKEY is zero.
#[test]
fn an_idle_account_has_no_margin_level() {
    let figures = Figures::new(idle("10"), (0, 0), None, Fixed::ZERO, &symbols());
    assert_eq!(figures.margin_level_bps, None);
    assert_eq!(figures.monkey, Monkey::default());
    assert!(figures.positions.is_empty());
}

/// Totals sum the accounts that have figures and count every account; the total margin level is
/// over the summed equity and requirement.
#[test]
fn totals_sum_the_accounts_with_figures() {
    let mut first = Figures::new(idle("100"), (0, 0), None, fixed("1"), &symbols());
    (first.equity, first.margin, first.unrealised_pnl) = (fixed("120"), fixed("40"), fixed("20"));
    first.monkey.entitled = TokenUnits(u128::MAX - 1);
    let mut second = Figures::new(idle("50"), (0, 0), None, fixed("-2"), &symbols());
    (second.equity, second.margin, second.unrealised_pnl) = (fixed("40"), fixed("40"), fixed("-10"));
    second.monkey = Monkey { entitled: TokenUnits(7), newest_entitled: TokenUnits(8), claimed: None };
    let totals = totals([Some(&first), None, Some(&second)]);
    assert_eq!(
        totals,
        Totals {
            accounts: 3,
            reporting: 2,
            balance: fixed("150"),
            equity: fixed("160"),
            margin: fixed("80"),
            margin_level_bps: Some(20_000),
            unrealised_pnl: fixed("10"),
            realised_pnl: fixed("-1"),
            monkey: Monkey { entitled: TokenUnits(u128::MAX), newest_entitled: TokenUnits(8), claimed: None },
        }
    );
    assert_eq!(totals.reporting, 2);
    let empty = super::totals([]);
    assert_eq!((empty.accounts, empty.reporting, empty.margin_level_bps), (0, 0, None));
}

/// The curve keeps the latest equity in each minute, oldest first, over the last day, and takes no
/// point older than its newest.
#[test]
fn the_curve_keeps_a_point_a_minute_for_a_day() {
    let at = |time_ms: u64, equity: &str| CurvePoint { time_ms, equity: fixed(equity) };
    let minute = CURVE_STEP.as_millis() as u64;
    let start = 1_800_000_000_000 / minute * minute;
    let mut curve = Vec::new();
    record(&mut curve, at(start + 1_000, "100"));
    record(&mut curve, at(start + 59_000, "101"));
    assert_eq!(curve, [at(start + 59_000, "101")], "the latest in the minute");
    record(&mut curve, at(start + minute, "102"));
    record(&mut curve, at(start + 30_000, "1"));
    assert_eq!(curve, [at(start + 59_000, "101"), at(start + minute, "102")], "nothing from before the newest");
    let day = CURVE_SPAN.as_millis() as u64;
    for n in 2..=day / minute + 1 {
        record(&mut curve, at(start + n * minute, "103"));
    }
    let newest = start + (day / minute + 1) * minute;
    assert_eq!(curve.len() as u64, day / minute + 1);
    assert_eq!(curve.first().map(|p| p.time_ms), Some(newest - day));
    assert_eq!(curve.last().map(|p| p.time_ms), Some(newest));
}

// ── Helpers ──

fn fixed(text: &str) -> Fixed {
    text.parse().unwrap()
}

/// An account holding `balance` and nothing else.
fn idle(balance: &str) -> AccountFigures {
    AccountFigures {
        balance: fixed(balance),
        equity: fixed(balance),
        margin: Fixed::ZERO,
        positions: BTreeMap::new(),
        orders: BTreeMap::new(),
        closeouts: Vec::new(),
    }
}

/// BTC-USD marked at 99, and ETH-USD with no mark.
fn symbols() -> BTreeMap<MarketId, Listing> {
    let usd = |symbol: &str, mark: Option<&str>| Listing {
        symbol: symbol.into(),
        quote: QuoteCurrency::Usd,
        mark: mark.map(fixed),
        fx_mark: None,
    };
    BTreeMap::from([(1, usd("BTC-USD", Some("99"))), (2, usd("ETH-USD", None))])
}
