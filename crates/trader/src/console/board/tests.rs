use engine_core::account::Position;

use super::*;

/// The node's figures pass through; unrealised profit is equity less balance, the margin level is
/// equity over the requirement in bps, flat positions drop, and symbols come from the market list.
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
    };
    let entitlement = Entitlement { entitled: TokenUnits(5), newest_entitled: TokenUnits(9) };
    let figures = Figures::new(account, (1_000, 42), Some(entitlement), fixed("-3.25"), &symbols());
    assert_eq!(figures.unrealised_pnl, fixed("250.5"));
    assert_eq!(figures.margin_level_bps, Some(25_010));
    assert_eq!(figures.realised_pnl, fixed("-3.25"));
    assert_eq!(
        figures.positions,
        [
            PositionView { market: 1, symbol: Some("BTC-USD".into()), qty: fixed("-0.5"), entry: fixed("100") },
            PositionView { market: 7, symbol: None, qty: fixed("2"), entry: fixed("3") },
        ]
    );
    assert_eq!(figures.monkey, Monkey { entitled: TokenUnits(5), newest_entitled: TokenUnits(9), claimed: None });
    assert_eq!((figures.time_ms, figures.head_seq), (1_000, 42));
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
    }
}

fn symbols() -> BTreeMap<MarketId, String> {
    BTreeMap::from([(1, "BTC-USD".to_owned()), (2, "ETH-USD".to_owned())])
}
