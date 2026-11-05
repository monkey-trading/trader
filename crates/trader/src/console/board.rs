//! What the console shows of each account and of all of them: the account's own status, from its
//! task, beside the figures the node's verified answers give, and the totals of those figures.
//! Figures the node does not give are derived from its: unrealised profit and loss is equity less
//! balance, the margin level is equity over the margin requirement, realised profit and loss is
//! the sum over the account's fills of what each realised less its profit fee, and the equity curve
//! is the equity the node gave, one point a minute over the last day, kept in memory.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use engine_core::Fixed;
use engine_core::account::{Position, RestingOrder};
use engine_core::emissions::TokenUnits;
use engine_core::fixed::Rounding;
use engine_core::market::QuoteCurrency;
use engine_core::protocol::ProtocolVersion;
use engine_core::types::{AccountId, MarketId};
use serde::Serialize;

use crate::bot::link::AccountStatus;
use crate::node::api::{AccountFigures, Entitlement, HistoryFill, PositionCloseout, Reach};

/// How far apart the points of an equity curve are, and how far back it goes.
pub const CURVE_STEP: Duration = Duration::from_secs(60);
pub const CURVE_SPAN: Duration = Duration::from_secs(24 * 60 * 60);

/// What the console last read of an account from the node.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeView {
    /// The last figures read whole; `None` until the first is.
    pub figures: Option<Arc<Figures>>,
    /// The account's latest fills, newest first.
    pub fills: Arc<Vec<FillView>>,
    /// The account's equity curve, oldest first.
    pub curve: Arc<Vec<CurvePoint>>,
    /// Why the last read failed, when it did.
    pub error: Option<String>,
}

/// An account's figures, as the node gave them at `time_ms` with its log at `head_seq`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Figures {
    pub balance: Fixed,
    pub equity: Fixed,
    /// The margin its positions require.
    pub margin: Fixed,
    /// Equity over the margin requirement, in bps; `None` with nothing required. The engine closes
    /// positions below 5,000.
    pub margin_level_bps: Option<i64>,
    pub unrealised_pnl: Fixed,
    /// Over every fill of the account, after profit fees.
    pub realised_pnl: Fixed,
    pub positions: Vec<PositionView>,
    pub orders: Vec<RestingOrder>,
    pub monkey: Monkey,
    pub time_ms: u64,
    pub head_seq: u64,
}

/// A position in one market.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PositionView {
    pub market: MarketId,
    pub symbol: Option<String>,
    pub qty: Fixed,
    pub entry: Fixed,
    /// The market's mark as the node gave it, if it gave one.
    pub mark: Option<Fixed>,
    /// The profit or loss closing at the mark would realise, in USD, as the engine values it in
    /// the account's equity; `None` without a mark, or an FX index for a market quoted otherwise.
    pub unrealised_pnl: Option<Fixed>,
    /// Where the account closes out as this market moves, if the node said.
    pub closeout: Option<Closeout>,
}

/// Where the account closes out as one position's market moves: `due` already, `at` a price, or
/// `beyond` any move the node considers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Closeout {
    pub reach: Reach,
    pub price: Option<Fixed>,
}

/// A market as the node lists it: its symbol, the currency it is quoted in, its mark, and the FX
/// index a market quoted in another currency than USD is valued at.
#[derive(Clone, Debug, PartialEq)]
pub struct Listing {
    pub symbol: String,
    pub quote: QuoteCurrency,
    pub mark: Option<Fixed>,
    pub fx_mark: Option<Fixed>,
}

/// One point of an equity curve: the account's equity as the node gave it at `time_ms`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct CurvePoint {
    pub time_ms: u64,
    pub equity: Fixed,
}

/// MONKEY, in token base units.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Monkey {
    /// The cumulative entitlement the vault can check a claim against now.
    pub entitled: TokenUnits,
    /// The cumulative entitlement at the node's newest checkpoint.
    pub newest_entitled: TokenUnits,
    /// What the vault has paid: a chain read, which the bot does not make yet, so always `None`.
    pub claimed: Option<TokenUnits>,
}

/// A fill, with its market's symbol.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FillView {
    #[serde(flatten)]
    pub fill: HistoryFill,
    pub symbol: Option<String>,
}

/// One account as the API shows it.
#[derive(Debug, Serialize)]
pub struct AccountView<'a> {
    pub address: AccountId,
    #[serde(flatten)]
    pub status: &'a AccountStatus,
    pub figures: Option<&'a Figures>,
    pub error: Option<&'a str>,
    /// The account's equity curve, oldest first: the whole of it, or its newest point.
    pub curve: &'a [CurvePoint],
}

/// The figures summed over the accounts that have them.
#[derive(Debug, PartialEq, Serialize)]
pub struct Totals {
    pub accounts: usize,
    /// How many accounts the sums hold: those with figures.
    pub reporting: usize,
    pub balance: Fixed,
    pub equity: Fixed,
    pub margin: Fixed,
    pub margin_level_bps: Option<i64>,
    pub unrealised_pnl: Fixed,
    pub realised_pnl: Fixed,
    pub monkey: Monkey,
}

impl Figures {
    /// The figures of a verified account answer read at `time_ms` and `head_seq`, its entitlement
    /// (`None` while no checkpoint holds one), its realised profit and loss, and the markets as the
    /// node lists them.
    pub fn new(
        account: AccountFigures,
        (time_ms, head_seq): (u64, u64),
        entitlement: Option<Entitlement>,
        realised_pnl: Fixed,
        markets: &BTreeMap<MarketId, Listing>,
    ) -> Self {
        let closeout = |market: MarketId| -> Option<Closeout> {
            let PositionCloseout { reach, price, .. } = *account.closeouts.iter().find(|c| c.market == market)?;
            Some(Closeout { reach, price })
        };
        let positions = account
            .positions
            .iter()
            .filter(|(_, position)| !position.qty.is_zero())
            .map(|(market, position)| PositionView {
                market: *market,
                symbol: markets.get(market).map(|listing| listing.symbol.clone()),
                qty: position.qty,
                entry: position.entry,
                mark: markets.get(market).and_then(|listing| listing.mark),
                unrealised_pnl: markets.get(market).and_then(|listing| unrealised(position, listing)),
                closeout: closeout(*market),
            })
            .collect();
        let monkey = entitlement.map_or_else(Monkey::default, |e| Monkey {
            entitled: e.entitled,
            newest_entitled: e.newest_entitled,
            claimed: None,
        });
        Self {
            balance: account.balance,
            equity: account.equity,
            margin: account.margin,
            margin_level_bps: margin_level_bps(account.equity, account.margin),
            unrealised_pnl: account.equity - account.balance,
            realised_pnl,
            positions,
            orders: account.orders.into_values().collect(),
            monkey,
            time_ms,
            head_seq,
        }
    }
}

/// What closing `position` at its market's mark would realise, in USD, rounded down as the engine
/// rounds it in the account's equity; `None` without a mark, or a positive FX index for a market
/// quoted in another currency.
fn unrealised(position: &Position, listing: &Listing) -> Option<Fixed> {
    let pnl = position.unrealised(listing.mark?, ProtocolVersion::LATEST);
    match (listing.quote, listing.fx_mark) {
        (QuoteCurrency::Usd, _) => Some(pnl),
        (quote, Some(fx)) if fx.is_positive() => Some(quote.to_usd(pnl, Some(fx), Rounding::Down)),
        _ => None,
    }
}

/// Adds `point` to `curve`, oldest first: the latest equity in each `CURVE_STEP`, over the last
/// `CURVE_SPAN` before it. A point older than the curve's newest is not taken.
pub fn record(curve: &mut Vec<CurvePoint>, point: CurvePoint) {
    let step = CURVE_STEP.as_millis() as u64;
    match curve.last_mut() {
        Some(last) if point.time_ms < last.time_ms => return,
        Some(last) if last.time_ms / step == point.time_ms / step => *last = point,
        _ => curve.push(point),
    }
    let from = point.time_ms.saturating_sub(CURVE_SPAN.as_millis() as u64);
    let old = curve.iter().take_while(|p| p.time_ms < from).count();
    curve.drain(..old);
}

/// The totals of `figures`, one item per account, `None` for an account with none.
pub fn totals<'a>(figures: impl IntoIterator<Item = Option<&'a Figures>>) -> Totals {
    let mut totals = Totals {
        accounts: 0,
        reporting: 0,
        balance: Fixed::ZERO,
        equity: Fixed::ZERO,
        margin: Fixed::ZERO,
        margin_level_bps: None,
        unrealised_pnl: Fixed::ZERO,
        realised_pnl: Fixed::ZERO,
        monkey: Monkey::default(),
    };
    for figures in figures {
        totals.accounts += 1;
        let Some(figures) = figures else { continue };
        totals.reporting += 1;
        totals.balance += figures.balance;
        totals.equity += figures.equity;
        totals.margin += figures.margin;
        totals.unrealised_pnl += figures.unrealised_pnl;
        totals.realised_pnl += figures.realised_pnl;
        let monkey = &mut totals.monkey;
        monkey.entitled = add_units(monkey.entitled, figures.monkey.entitled);
        monkey.newest_entitled = add_units(monkey.newest_entitled, figures.monkey.newest_entitled);
    }
    totals.margin_level_bps = margin_level_bps(totals.equity, totals.margin);
    totals
}

/// Equity over a margin requirement, in bps, or `None` with nothing required.
fn margin_level_bps(equity: Fixed, margin: Fixed) -> Option<i64> {
    margin.is_positive().then(|| equity.bps_of(margin))
}

/// A sum of MONKEY amounts, which the token's supply cap keeps far below `u128::MAX`.
fn add_units(a: TokenUnits, b: TokenUnits) -> TokenUnits {
    TokenUnits(a.0.saturating_add(b.0))
}

#[cfg(test)]
mod tests;
