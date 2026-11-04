//! The daily loss limit: the loss since the day's first equity reading, per UTC day.

use engine_core::Fixed;
use engine_core::fixed::Rounding;
use engine_core::types::Millis;
use serde::{Deserialize, Serialize};

const MS_PER_DAY: Millis = 86_400_000;

/// How much an account may lose in a UTC day.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum LossLimit {
    /// This many USD.
    Usd(Fixed),
    /// This many bps of the equity at the day's first reading, from 1 to 10,000.
    EquityBps(i64),
}

impl LossLimit {
    /// Whether the limit is a positive amount, and a share is at most all of the equity.
    pub fn is_valid(&self) -> bool {
        match *self {
            Self::Usd(usd) => usd.is_positive() && usd <= Fixed::MAX_INPUT,
            Self::EquityBps(bps) => (1..=10_000).contains(&bps),
        }
    }

    /// The loss, in USD, that trips the limit for a day that opened at `start_equity`.
    fn amount(&self, start_equity: Fixed) -> Fixed {
        match *self {
            Self::Usd(usd) => usd,
            Self::EquityBps(bps) => start_equity.mul_bps_rounded(bps, Rounding::Down),
        }
    }
}

/// Where a day stands, kept so a restart does not hand a stopped account a new day.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayState {
    /// Days since the Unix epoch, UTC.
    pub day: i64,
    /// The equity at the day's first reading.
    pub start_equity: Fixed,
    pub tripped: bool,
}

/// What the day's loss says about trading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DayStatus {
    Trading,
    /// The limit is reached and the account stays stopped until the next day. `newly` on the
    /// reading that reached it.
    Stopped {
        newly: bool,
    },
}

/// Tracks one account's loss within a UTC day against its limit.
#[derive(Clone, Debug)]
pub struct DayLoss {
    limit: LossLimit,
    state: Option<DayState>,
}

impl DayLoss {
    /// A tracker for `limit`, resuming `restored` when it is from a day that is not over.
    pub fn new(limit: LossLimit, restored: Option<DayState>) -> Self {
        Self { limit, state: restored }
    }

    /// Reads the account's equity at sequenced time `ts`. The first reading of a day sets its
    /// opening equity; a loss from it of at least the limit, and more than nothing, stops the
    /// account until the next day. A reading from before the current day, which commands in order
    /// never give, changes nothing.
    pub fn observe(&mut self, ts: Millis, equity: Fixed) -> DayStatus {
        let day = ts.div_euclid(MS_PER_DAY);
        let state = match self.state {
            Some(state) if state.day >= day => state,
            _ => DayState { day, start_equity: equity, tripped: false },
        };
        let loss = state.start_equity - equity;
        let reached = loss.is_positive() && loss >= self.limit.amount(state.start_equity);
        let newly = reached && !state.tripped;
        let state = DayState { tripped: state.tripped || reached, ..state };
        self.state = Some(state);
        if state.tripped { DayStatus::Stopped { newly } } else { DayStatus::Trading }
    }

    /// Whether the account is stopped for the current day, without a new reading.
    pub fn is_stopped(&self) -> bool {
        self.state.is_some_and(|state| state.tripped)
    }

    /// The day's state, to keep across a restart.
    pub fn state(&self) -> Option<DayState> {
        self.state
    }
}
