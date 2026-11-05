//! The controls on one account's orders: what a strategy's target becomes once sizing, the risk
//! limits, the margin buffer, positive cash, the minimum hold and the market's hours have had their
//! say. A target is a wish; `Controls::decide` returns the order to place, if any, or why none.

use std::collections::BTreeMap;

use engine_core::Fixed;
use engine_core::account::Position;
use engine_core::fixed::Rounding;
use engine_core::market::MarketSpec;
use engine_core::protocol::ProtocolVersion;
use engine_core::types::{MarketId, Millis, Side};

use crate::hours::is_open;
use crate::margin::{Holding, equity_and_margin};
use crate::risk::{DayLoss, DayState, DayStatus, RiskError, RiskParams};
use crate::sizing::{Sizing, SizingError};
use crate::strategy::{OrderIntent, Target, order_for};

/// An account as the controls see it: its cash and every open position at its market's mark.
#[derive(Clone, Copy, Debug)]
pub struct Account<'a> {
    pub balance: Fixed,
    pub holdings: &'a [Holding<'a>],
}

/// A strategy's target for one market after a bar.
#[derive(Clone, Copy, Debug)]
pub struct BarTarget<'a> {
    /// The sequenced time of the command that closed the bar.
    pub ts: Millis,
    pub market: MarketId,
    pub spec: &'a MarketSpec,
    /// The index mark the order is quoted at.
    pub mark: Fixed,
    pub target: Target,
    /// The bar is the first after the market was shut (`hours::GapTracker`).
    pub gap: bool,
    /// A reversal is only a close, as the engine refused the last one whole.
    pub close_first: bool,
    /// The most, in bps, a market order may fill from the mark: equity is judged after that loss.
    pub max_slippage_bps: i64,
}

/// What to do about a target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Place this order.
    Order(OrderIntent),
    /// The position is already where the target wants it.
    AtTarget,
    /// The day's loss limit is reached with positions open: close them all.
    Flatten,
    /// No order, for this reason.
    Refused(Refusal),
}

/// Why no order is placed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// A holding is in a market the bot cannot value, so equity and margin are unknown.
    Unvalued,
    /// The day's loss limit is reached, and the account stays flat until the next UTC day.
    DailyLossStopped,
    MarketClosed,
    /// The bar is the first after a closure; its price may have jumped, and the next bar decides.
    GapBar,
    /// The position was opened too recently to close; it may be closed from `until`.
    MinHold {
        until: Millis,
    },
    /// Equity or the mark leaves no position to open.
    NoSize,
    /// The order is smaller than the market's minimum notional.
    BelowMinimum,
    /// Cash is not positive.
    NoCash,
    TooManyPositions,
    TotalNotional,
    /// The order would leave equity within the buffer of the requirement.
    Margin {
        equity: Fixed,
        needed: Fixed,
    },
}

/// The controls of one account: its limits, its sizing, the day's loss, and when each of its
/// positions was opened.
#[derive(Clone, Debug)]
pub struct Controls {
    risk: RiskParams,
    sizing: Sizing,
    day: DayLoss,
    /// When each position opened, in sequenced time; a position found at startup has no entry, and
    /// counts as held long enough.
    opened: BTreeMap<MarketId, Millis>,
}

impl Controls {
    /// Controls under `risk` and `sizing`, resuming `restored` day state.
    /// Error cases:
    /// - `risk` is out of range (see `RiskParams::validate`)
    pub fn new(risk: RiskParams, sizing: Sizing, restored: Option<DayState>) -> Result<Self, RiskError> {
        risk.validate()?;
        let day = DayLoss::new(risk.daily_loss, restored);
        Ok(Self { risk, sizing, day, opened: BTreeMap::new() })
    }

    /// Checks the sizing against a market the account trades.
    /// Error cases:
    /// - the leverage cap is not below the market's own, or the sizing is out of range
    pub fn check_market(&self, spec: &MarketSpec) -> Result<(), SizingError> {
        self.sizing.validate(spec)
    }

    /// The day's state, to keep across a restart.
    pub fn day_state(&self) -> Option<DayState> {
        self.day.state()
    }

    /// Reads the account's equity at sequenced time `ts`, as marks move or fills land, so a loss is
    /// caught when it happens and not at the next bar. `None` when a holding cannot be valued.
    pub fn observe(&mut self, ts: Millis, account: &Account<'_>) -> Option<DayStatus> {
        let (equity, _) = equity_and_margin(account.balance, account.holdings)?;
        let flat = account.holdings.iter().all(|holding| holding.qty.is_zero());
        Some(self.day.observe_with(ts, equity, flat))
    }

    /// Records a fill that moved the position in `market` from `before` to `after`: a position from
    /// flat, or across zero, opens at `ts`; one back to flat is forgotten; adding or reducing keeps
    /// its opening time.
    pub fn on_fill(&mut self, ts: Millis, market: MarketId, before: Fixed, after: Fixed) {
        if after.is_zero() {
            self.opened.remove(&market);
        } else if before.is_zero() || before.signum() != after.signum() {
            self.opened.insert(market, ts);
        }
    }

    /// When each open position was opened, to keep across a restart.
    pub fn opened(&self) -> &BTreeMap<MarketId, Millis> {
        &self.opened
    }

    /// Takes the opening times kept from an earlier run, for the positions the account still holds.
    pub fn restore_opened(&mut self, opened: BTreeMap<MarketId, Millis>) {
        self.opened = opened;
    }

    /// Whether the day's loss limit has stopped the account for the day.
    pub fn is_stopped(&self) -> bool {
        self.day.is_stopped()
    }

    /// The order that takes the account toward `bar.target`, or why there is none. Sizing sets the
    /// size of a new position; a position already on the target's side keeps its size, so the bot
    /// does not trade as equity moves it. An order that adds to exposure passes the cash, position
    /// count, notional and margin checks; one that only reduces it passes the minimum hold, and
    /// nothing else but the day's loss limit and the market's hours.
    pub fn decide(&mut self, account: &Account<'_>, bar: &BarTarget<'_>) -> Decision {
        let Some((equity, _)) = equity_and_margin(account.balance, account.holdings) else {
            return Decision::Refused(Refusal::Unvalued);
        };
        let flat = account.holdings.iter().all(|holding| holding.qty.is_zero());
        let stopped = matches!(self.day.observe_with(bar.ts, equity, flat), DayStatus::Stopped { .. });
        if !is_open(&bar.spec.schedule, bar.ts) {
            return Decision::Refused(Refusal::MarketClosed);
        }
        if stopped {
            let open = account.holdings.iter().any(|holding| !holding.qty.is_zero());
            return if open { Decision::Flatten } else { Decision::Refused(Refusal::DailyLossStopped) };
        }
        if bar.gap {
            return Decision::Refused(Refusal::GapBar);
        }
        let position = account.holdings.iter().find(|h| h.market == bar.market).map_or(Fixed::ZERO, |h| h.qty);
        let on_side = matches!((bar.target, position.signum()), (Target::Long, 1) | (Target::Short, -1));
        let others: Fixed = (account.holdings.iter()).filter(|h| h.market != bar.market).map(Holding::notional).sum();
        let size = if on_side { position.abs() } else { self.sizing.qty(bar.mark, equity, others) };
        // With no size to open, a target that opens something can only close what is held.
        let target = if size.is_zero() { Target::Flat } else { bar.target };
        let Some(intent) = order_for(target, size, position, bar.close_first) else {
            let nothing_to_open = bar.target != Target::Flat && position.is_zero();
            return if nothing_to_open { Decision::Refused(Refusal::NoSize) } else { Decision::AtTarget };
        };
        let after = position + if intent.side == Side::Buy { intent.qty } else { -intent.qty };
        let crosses = position.signum() * after.signum() < 0;
        let closes = !position.is_zero() && (after.abs() < position.abs() || crosses);
        if closes && let Some(until) = self.hold_until(bar.market).filter(|until| bar.ts < *until) {
            return Decision::Refused(Refusal::MinHold { until });
        }
        let adds = crosses || after.abs() > position.abs();
        // The profit a reversal's closing leg realises pays the engine's profit fee, whose rate the bot
        // does not know, so that profit is not counted toward the margin buffer.
        let fee_exposure = if crosses {
            let held = Position {
                qty: position,
                entry: account.holdings.iter().find(|h| h.market == bar.market).map_or(bar.mark, |h| h.entry),
            };
            held.unrealised(bar.mark, ProtocolVersion::LATEST).max(Fixed::ZERO)
        } else {
            Fixed::ZERO
        };
        if adds && let Some(refusal) = self.refuse_adding(account, bar, equity - fee_exposure, after, intent) {
            // A reversal whose opening leg is refused still closes the side the strategy has left.
            if crosses && let Some(close) = order_for(Target::Flat, Fixed::ZERO, position, false) {
                return Decision::Order(close);
            }
            return Decision::Refused(refusal);
        }
        Decision::Order(intent)
    }

    /// When the position in `market` may be closed, if its opening is known.
    fn hold_until(&self, market: MarketId) -> Option<Millis> {
        let hold = Millis::try_from(self.risk.min_hold.as_millis()).unwrap_or(Millis::MAX);
        self.opened.get(&market).map(|opened| opened.saturating_add(hold))
    }

    /// The first limit an order that takes the market's position to `after` breaks, if any.
    fn refuse_adding(
        &self,
        account: &Account<'_>,
        bar: &BarTarget<'_>,
        equity: Fixed,
        after: Fixed,
        intent: OrderIntent,
    ) -> Option<Refusal> {
        if !account.balance.is_positive() {
            return Some(Refusal::NoCash);
        }
        if intent.qty.mul_rounded(bar.mark, Rounding::Down) < bar.spec.min_order_notional {
            return Some(Refusal::BelowMinimum);
        }
        let others = || account.holdings.iter().filter(|h| h.market != bar.market && !h.qty.is_zero());
        let opens_another = account.holdings.iter().all(|h| h.market != bar.market || h.qty.is_zero());
        if opens_another && others().count() >= self.risk.max_open_positions {
            return Some(Refusal::TooManyPositions);
        }
        // Only the requirement is read from the holdings after the order, so the new one's entry is the mark.
        let this = Holding { market: bar.market, qty: after, entry: bar.mark, mark: bar.mark, spec: bar.spec };
        let after_holdings: Vec<_> = others().copied().chain([this]).collect();
        if after_holdings.iter().map(Holding::notional).sum::<Fixed>() > self.risk.max_total_notional {
            return Some(Refusal::TotalNotional);
        }
        // A fill at the mark leaves equity as it is; one at the worst price the order allows costs the
        // slippage on its notional. The order is judged on that equity against the requirement after it.
        let Some((_, required)) = equity_and_margin(account.balance, &after_holdings) else {
            return Some(Refusal::Unvalued);
        };
        let slippage =
            intent.qty.mul_rounded(bar.mark, Rounding::Up).mul_bps_rounded(bar.max_slippage_bps, Rounding::Up);
        let equity = equity - slippage;
        let needed = required.mul_bps_rounded(10_000 + self.risk.margin_buffer_bps, Rounding::Up);
        (equity < needed).then_some(Refusal::Margin { equity, needed })
    }
}

#[cfg(test)]
mod tests;
