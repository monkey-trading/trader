//! The strategies the bot runs: each turns completed bar closes into a target position, updating
//! its indicators in constant time per close. `StrategyParams` is a strategy's config section, and
//! `Strategy` runs whichever it names.

mod bollinger_reversion;
mod donchian;
mod ema_crossover;
mod macd;
mod rsi_reversion;

use engine_core::Fixed;
use serde::Deserialize;

use crate::bands::MAX_CLOSE;
use crate::sma::MAX_PERIOD;
use crate::strategy::{ParamsError, SmaCrossover, Target};

pub use bollinger_reversion::BollingerReversion;
pub use donchian::DonchianBreakout;
pub use ema_crossover::EmaCrossover;
pub use macd::Macd;
pub use rsi_reversion::RsiReversion;

/// What a close says about moving into or out of a position, for the strategies that hold their
/// target between signals.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Signals {
    pub enter_long: bool,
    pub enter_short: bool,
    pub exit_long: bool,
    pub exit_short: bool,
}

/// The target after a close: flat enters on one signal, a long or short goes the other way on an
/// entry in that direction, goes flat on its exit, and otherwise stays. Two entries at once are no
/// signal.
pub(crate) fn next_target(previous: Target, signals: Signals) -> Target {
    match previous {
        Target::Flat => match (signals.enter_long, signals.enter_short) {
            (true, false) => Target::Long,
            (false, true) => Target::Short,
            _ => Target::Flat,
        },
        Target::Long if signals.enter_short => Target::Short,
        Target::Long if signals.exit_long => Target::Flat,
        Target::Short if signals.enter_long => Target::Long,
        Target::Short if signals.exit_short => Target::Flat,
        held => held,
    }
}

/// A strategy and its parameters, as a `[strategy]` section names it by `kind`. Fields a strategy
/// has a conventional value for may be left out.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StrategyParams {
    SmaCrossover {
        fast: usize,
        slow: usize,
    },
    EmaCrossover {
        fast: usize,
        slow: usize,
    },
    Macd {
        #[serde(default = "default_macd_fast")]
        fast: usize,
        #[serde(default = "default_macd_slow")]
        slow: usize,
        #[serde(default = "default_macd_signal")]
        signal: usize,
    },
    RsiReversion {
        #[serde(default = "default_rsi_period")]
        period: usize,
        #[serde(default = "default_oversold")]
        oversold: Fixed,
        #[serde(default = "default_rsi_exit")]
        exit: Fixed,
        #[serde(default = "default_overbought")]
        overbought: Fixed,
    },
    BollingerReversion {
        #[serde(default = "default_band_period")]
        period: usize,
        #[serde(default = "default_band_width")]
        width: Fixed,
    },
    DonchianBreakout {
        #[serde(default = "default_entry")]
        entry: usize,
        #[serde(default = "default_exit")]
        exit: usize,
    },
}

/// One of the strategies, running.
#[derive(Clone, Debug)]
pub enum Strategy {
    SmaCrossover(SmaCrossover),
    EmaCrossover(EmaCrossover),
    Macd(Macd),
    RsiReversion(RsiReversion),
    BollingerReversion(BollingerReversion),
    DonchianBreakout(DonchianBreakout),
}

impl StrategyParams {
    /// How many bars of history to feed a new strategy before it trades, so its indicators have the
    /// window they need and the exponential ones, which never forget, have mostly lost their seed:
    /// the bars to a first target for the windowed strategies, a few times that for the others. At
    /// most `MAX_PERIOD` bars.
    pub fn warm_up_bars(&self) -> usize {
        let bars = match *self {
            Self::SmaCrossover { slow, .. } => slow,
            Self::EmaCrossover { slow, .. } => 3 * slow,
            Self::Macd { slow, signal, .. } => 3 * (slow + signal),
            Self::RsiReversion { period, .. } => 5 * period + 1,
            Self::BollingerReversion { period, .. } => period,
            Self::DonchianBreakout { entry, .. } => entry + 1,
        };
        bars.min(MAX_PERIOD)
    }

    /// The strategy these parameters name, with its indicators empty.
    /// Error cases:
    /// - a parameter outside what its strategy takes (see `ParamsError`)
    pub fn build(&self) -> Result<Strategy, ParamsError> {
        Ok(match *self {
            Self::SmaCrossover { fast, slow } => Strategy::SmaCrossover(SmaCrossover::new(fast, slow)?),
            Self::EmaCrossover { fast, slow } => Strategy::EmaCrossover(EmaCrossover::new(fast, slow)?),
            Self::Macd { fast, slow, signal } => Strategy::Macd(Macd::new(fast, slow, signal)?),
            Self::RsiReversion { period, oversold, exit, overbought } => {
                Strategy::RsiReversion(RsiReversion::new(period, oversold, exit, overbought)?)
            }
            Self::BollingerReversion { period, width } => {
                Strategy::BollingerReversion(BollingerReversion::new(period, width)?)
            }
            Self::DonchianBreakout { entry, exit } => Strategy::DonchianBreakout(DonchianBreakout::new(entry, exit)?),
        })
    }
}

impl Strategy {
    /// Takes a completed bar's close and returns the target once the strategy has enough closes. A
    /// close that is not a valid mark (see `is_valid_close`) is no data: it changes nothing and
    /// gives no target.
    pub fn on_close(&mut self, close: Fixed) -> Option<Target> {
        if !is_valid_close(close) {
            return None;
        }
        match self {
            Self::SmaCrossover(strategy) => strategy.on_close(close),
            Self::EmaCrossover(strategy) => strategy.on_close(close),
            Self::Macd(strategy) => strategy.on_close(close),
            Self::RsiReversion(strategy) => strategy.on_close(close),
            Self::BollingerReversion(strategy) => strategy.on_close(close),
            Self::DonchianBreakout(strategy) => strategy.on_close(close),
        }
    }
}

/// Where the history that counts starts in `closes`, oldest first: after the last step of more than
/// 40% down or 67% up between neighbours, which no market moves in a bar but a split does (2-for-1 or
/// more; a smaller ratio cannot be told from a move). History before it is in other units.
pub fn after_unit_break(closes: &[Fixed]) -> usize {
    let broken = |pair: &[Fixed]| {
        let (before, after) = (pair[0], pair[1]);
        after.mul_int(100) < before.mul_int(60) || after.mul_int(100) > before.mul_int(167)
    };
    closes.windows(2).rposition(broken).map_or(0, |at| at + 1)
}

/// Whether `close` is a mark the strategies take: positive, and at most `MAX_CLOSE`, which keeps
/// every indicator's arithmetic inside its integers.
pub fn is_valid_close(close: Fixed) -> bool {
    close.is_positive() && close <= MAX_CLOSE
}

/// The conventional fast period of a MACD.
fn default_macd_fast() -> usize {
    12
}

/// The conventional slow period of a MACD.
fn default_macd_slow() -> usize {
    26
}

/// The conventional signal period of a MACD.
fn default_macd_signal() -> usize {
    9
}

/// The conventional period of an RSI.
fn default_rsi_period() -> usize {
    14
}

/// The conventional oversold level of an RSI.
fn default_oversold() -> Fixed {
    Fixed::from_int(30)
}

/// The level an RSI reversion leaves its position at: the index's midpoint.
fn default_rsi_exit() -> Fixed {
    Fixed::from_int(50)
}

/// The conventional overbought level of an RSI.
fn default_overbought() -> Fixed {
    Fixed::from_int(70)
}

/// The conventional period of Bollinger bands.
fn default_band_period() -> usize {
    20
}

/// The conventional width of Bollinger bands, in standard deviations.
fn default_band_width() -> Fixed {
    Fixed::from_int(2)
}

/// The conventional entry channel of a Donchian breakout.
fn default_entry() -> usize {
    20
}

/// The conventional exit channel of a Donchian breakout.
fn default_exit() -> usize {
    10
}

#[cfg(test)]
mod tests;
