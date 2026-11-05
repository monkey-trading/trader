//! An account's equity and margin requirement, worked out as the engine works them out, for the
//! checks that keep an account clear of a close-out.

use engine_core::Fixed;
use engine_core::account::Position;
use engine_core::fixed::Rounding;
use engine_core::market::{MarketSpec, QuoteCurrency};
use engine_core::protocol::ProtocolVersion;
use engine_core::types::MarketId;

/// One open position, valued at the market's mark.
#[derive(Clone, Copy, Debug)]
pub struct Holding<'a> {
    pub market: MarketId,
    /// Signed: positive long, negative short.
    pub qty: Fixed,
    pub entry: Fixed,
    pub mark: Fixed,
    pub spec: &'a MarketSpec,
}

impl Holding<'_> {
    /// The position's notional in USD, rounded up as the engine rounds a charge.
    pub fn notional(&self) -> Fixed {
        self.qty.abs().mul_rounded(self.mark, Rounding::Up)
    }
}

/// The account's equity (cash and the positions' profit or loss at their marks) and its margin
/// requirement (each market's tiers applied to its position's notional, summed), both in USD,
/// rounded as the engine rounds them: the profit down, the requirement up. `None` when a holding is
/// in a market quoted outside USD, whose conversion the bot does not follow.
pub fn equity_and_margin(balance: Fixed, holdings: &[Holding<'_>]) -> Option<(Fixed, Fixed)> {
    holdings.iter().try_fold((balance, Fixed::ZERO), |(equity, required), holding| {
        if holding.spec.quote_currency != QuoteCurrency::Usd {
            return None;
        }
        let position = Position { qty: holding.qty, entry: holding.entry };
        let profit = position.unrealised(holding.mark, ProtocolVersion::LATEST);
        let margin = holding.spec.margin_for(holding.notional(), Rounding::Up);
        Some((equity + profit, required + margin))
    })
}

#[cfg(test)]
pub(crate) mod tests;
