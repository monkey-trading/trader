//! What the console shows of each account and of all of them: the account's own status, from its
//! task, beside the figures the node's verified answers give, and the totals of those figures.
//! Figures the node does not give are derived from its: unrealised profit and loss is equity less
//! balance, the margin level is equity over the margin requirement, and realised profit and loss is
//! the sum over the account's fills of what each realised less its profit fee.

use std::collections::BTreeMap;
use std::sync::Arc;

use engine_core::Fixed;
use engine_core::account::RestingOrder;
use engine_core::emissions::TokenUnits;
use engine_core::types::{AccountId, MarketId};
use serde::Serialize;

use crate::bot::link::AccountStatus;
use crate::node::api::{AccountFigures, Entitlement, HistoryFill};

/// What the console last read of an account from the node.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeView {
    /// The last figures read whole; `None` until the first is.
    pub figures: Option<Arc<Figures>>,
    /// The account's latest fills, newest first.
    pub fills: Arc<Vec<FillView>>,
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
    /// (`None` while no checkpoint holds one), and its realised profit and loss.
    pub fn new(
        account: AccountFigures,
        (time_ms, head_seq): (u64, u64),
        entitlement: Option<Entitlement>,
        realised_pnl: Fixed,
        symbols: &BTreeMap<MarketId, String>,
    ) -> Self {
        let positions = account
            .positions
            .iter()
            .filter(|(_, position)| !position.qty.is_zero())
            .map(|(market, position)| PositionView {
                market: *market,
                symbol: symbols.get(market).cloned(),
                qty: position.qty,
                entry: position.entry,
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
