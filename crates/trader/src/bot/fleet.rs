//! Many accounts on one log. The fleet follows the public log once: each `Prices` command moves its
//! market's index mark, which goes into the bars of every bar length some account trades, and every
//! strategy of one set of parameters on one market and bar length runs once for all the accounts that
//! use it. A completed bar gives each account a target; the account's controls (sizing, risk limits,
//! margin, hours) turn it into an order or into none (`decide`), and an order waits in a queue in bar
//! order until the runner sends it (`run`). The fleet itself does no IO, and once warm allocates only
//! what reading a message's nesting costs.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::{Context, ensure};
use engine_core::Fixed;
use engine_core::eip712::Signature;
use engine_core::market::MarketSpec;
use engine_core::types::{AccountId, MarketId, Millis};
use tokio::time::Instant;
use trader_core::controls::Controls;
use trader_core::hours::GapTracker;
use trader_core::marks::IndexMarks;
use trader_core::strategies::{Strategy, StrategyParams};
use trader_core::strategy::{OrderIntent, Target};

use crate::bot::day::{Saved, StateFile};
use crate::bot::warm::WarmRequest;
use crate::config::ParamBounds;
use crate::keys::AccountKey;
use crate::metrics::FleetMetrics;

/// How far behind the node's time a bar may close and still be acted on; one further behind is
/// history the stream is replaying, quoted at marks long gone.
pub const REPLAY_LAG_MS: Millis = 30_000;
/// How long after the node's time a signed request may still be sequenced: long enough to reach a
/// node over a slow link, and shorter than the shortest bar, so a live bot's next bar is past it.
pub const EXPIRES_IN_MS: Millis = 30_000;
/// How long, in sequenced time, between reads of an account whose last read was not at the log.
pub const READ_RETRY_MS: Millis = 5_000;
/// How long, in sequenced time, between reads of an account whose last read was at the log, whether or
/// not it holds a position, so a balance moved by a withdrawal or financing is found.
pub const READ_EVERY_MS: Millis = 60_000;
/// How long, in sequenced time, after a try at closing the positions of a stopped account the daily
/// loss limit tries again.
pub const FLATTEN_RETRY_MS: Millis = 5_000;

/// A market the fleet follows, as the node lists it.
#[derive(Clone, Debug)]
pub struct MarketIn {
    pub spec: MarketSpec,
    /// The node's mark at startup, which orders are quoted at until the log gives one.
    pub mark: Option<Fixed>,
}

/// One account to trade, with the markets it trades.
pub struct AccountIn {
    pub key: Arc<AccountKey>,
    pub close_on_stop: bool,
    /// Whether the console lets the account's strategies place orders.
    pub running: bool,
    pub balance: Fixed,
    /// The account's next nonce, when the startup read gave it.
    pub nonce: Option<u64>,
    pub controls: Controls,
    pub state_file: StateFile,
    pub saved: Saved,
    pub strategies: Vec<StrategyIn>,
}

/// One market and strategy of an account, and the account's position in the market.
#[derive(Clone, Debug)]
pub struct StrategyIn {
    pub market: MarketId,
    pub bar_ms: Millis,
    pub params: StrategyParams,
    pub max_slippage_bps: i64,
    pub position: Fixed,
    pub entry: Fixed,
    /// Whether the console lets the strategy place orders.
    pub running: bool,
    /// The parameters the console may set, and their limits.
    pub bounds: Option<ParamBounds>,
}

/// Where an account's nonce stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nonce {
    /// The next request must carry this nonce.
    Synced(u64),
    /// The last request's outcome leaves the node's count unknown: read it before signing.
    Unknown,
}

/// One account the fleet trades.
pub struct Account {
    pub key: Arc<AccountKey>,
    pub address: AccountId,
    pub close_on_stop: bool,
    /// Whether the console lets the account's strategies place orders.
    pub running: bool,
    /// Whether the account is in `Fleet::changed`.
    pub changed: bool,
    /// Its strategies in `Fleet::assignments`.
    pub assignments: std::ops::Range<u32>,
    /// The signatures of the last orders this process signed for the account, to tell its fills
    /// from others': a signature is over the whole request, so no other request carries one of them.
    pub signed: VecDeque<Signature>,
    /// The closes a console flatten signed, with the expiry until which each may still enter the log,
    /// kept until the log has passed it, since a flatten sends one close after another without waiting
    /// for each to reach the log.
    pub closes: Vec<(Signature, Millis)>,
    /// Set when the account stops trading, for a flatten under way to see before it signs anything more.
    pub halt: Arc<AtomicBool>,
    pub nonce: Nonce,
    /// Whether a request for the account is out, so no other goes before it answers.
    pub busy: bool,
    /// The account's balance, as the last read of it at the log gave it.
    pub balance: Fixed,
    pub controls: Controls,
    pub state_file: StateFile,
    pub saved: Saved,
    /// Whether the saved state differs from what the account's controls and positions say, so it is
    /// to be written, and whether a write is out. No order is signed for the account meanwhile.
    pub state_dirty: bool,
    pub state_saving: bool,
    /// Whether the last write failed: nothing is ordered until one succeeds.
    pub state_unsaved: bool,
    /// The earliest a failed write of the state is tried again.
    pub save_retry_at: Option<Instant>,
    /// Whether a read of the account is wanted: a decision, a fill, or the time to read again.
    pub read_wanted: bool,
    /// Whether a read is out, until its answer is taken, and whether a decision, a fill or a cash event
    /// came after it was started: its answer is then not used.
    pub reading: bool,
    pub read_stale: bool,
    /// Moves on at each fill, cash event and relisting of the account's market: an order decided before one is
    /// not sent after it.
    pub epoch: u64,
    /// Whether the account is waiting in `Fleet::flattens`.
    pub flatten_queued: bool,
    /// Whether the last read was not of the log (or failed): nothing is valued or ordered from the
    /// account until a read agrees.
    pub behind: bool,
    /// The earliest sequenced time the account is read again.
    pub read_after: Millis,
    /// The earliest sequenced time the daily loss limit tries to close the positions again.
    pub flatten_after: Millis,
    /// The slippage bound of the group of markets the last close of the account named.
    pub flatten_bound: Option<i64>,
    pub loss_alert: bool,
    pub cash_alert: bool,
    /// The earliest a failed nonce read is tried again.
    pub retry_at: Option<Instant>,
    /// Why the account stopped trading, when it has.
    pub failed: Option<String>,
}

/// One market and strategy of one account.
pub struct Assignment {
    pub account: u32,
    pub series: u32,
    /// The shared strategy of the series this one reads.
    pub strategy: u32,
    pub market: MarketId,
    pub market_slot: u32,
    pub max_slippage_bps: i64,
    /// Whether the console lets the strategy place orders.
    pub running: bool,
    /// The parameters the console may set, and their limits.
    pub bounds: Option<ParamBounds>,
    pub position: Fixed,
    /// The position's entry price as the last read of the account at the log gave it.
    pub entry: Fixed,
    /// Whether the next reversal is only a close: the engine refused the last one whole.
    pub close_first: bool,
    /// The expiry of the last order signed here. Until the log passes it, that order may still enter
    /// the log, so its fill may still be to come, and no other order is placed.
    pub expires_at: Option<Millis>,
    /// A target the account's controls have not yet seen, waiting for a read of the account at the log.
    pub wanted: Option<Wanted>,
    /// An order the controls allowed, waiting for the runner to send it.
    pub pending: Option<Pending>,
    pub queued: bool,
    /// The bar whose order the node refused for its nonce and was decided again: once per bar.
    pub nonce_retried: Option<Millis>,
}

/// A strategy's target after the bar starting at `bar_start`, completed by the command at `ts`, which
/// the node sent `lag` ms after it, and whether that bar is the first after a closure.
#[derive(Clone, Copy, Debug)]
pub struct Wanted {
    pub target: Target,
    pub bar_start: Millis,
    pub ts: Millis,
    pub lag: Millis,
    pub gap: bool,
}

/// An order to send, decided for a bar (`wanted` names it and the command that completed it).
#[derive(Clone, Copy, Debug)]
pub struct Pending {
    pub intent: OrderIntent,
    /// The account's `epoch` when the order was decided.
    pub epoch: u64,
    /// The target it was decided from, to decide again if the account changes before it is sent.
    pub wanted: Wanted,
}

/// Where a series stands in its warm-up from the node's marks history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Warm {
    Waiting,
    Requested,
    /// A failed read is tried again at this sequenced time.
    RetryAt(Millis),
    Done,
}

/// A strategy of one set of parameters, run once for every account that uses it.
pub struct Shared {
    pub params: StrategyParams,
    pub strategy: Strategy,
    /// The target after the last bar.
    pub target: Option<Target>,
    /// Whether the last bar changed the target.
    pub changed: bool,
}

/// The bars of a market at a bar length, the strategies that read them, and the assignments that
/// trade on those.
pub struct SeriesState {
    pub market_slot: u32,
    pub length: Millis,
    /// The bars from the log, once the series has warmed up.
    pub live: Option<trader_core::bars::BarBuilder>,
    pub gaps: GapTracker,
    pub strategies: Vec<Shared>,
    pub assignments: Vec<u32>,
    pub warm: Warm,
    /// The number of the latest warm-up read asked for; an answer to an earlier one is no use.
    pub generation: u32,
    /// How many bars of history the longest-warming strategy wants.
    pub history_bars: usize,
}

/// A market's index and the series built on it.
pub struct MarketFeed {
    pub id: MarketId,
    pub spec: MarketSpec,
    pub marks: IndexMarks,
    /// The last index mark: the node's at startup, then the log's, which orders are quoted at.
    pub mark: Option<Fixed>,
    /// The last exact index mark the fleet derived, which a move is a change of.
    pub exact_mark: Option<Fixed>,
    /// The time of the first command at which the index was exact.
    pub exact_from: Option<Millis>,
    /// The moves of the index from `exact_from` until every series has warmed up.
    pub pending_moves: Vec<(Millis, Fixed)>,
    /// The first exact value, until every series has warmed up: where moves start from, unless the
    /// history says the index was already there.
    pub first_exact: Option<(Millis, Fixed)>,
    pub series: Vec<u32>,
    /// The assignments in this market.
    pub assignments: Vec<u32>,
    pub unwarmed: usize,
    /// When the market split, if it did while the bot ran.
    pub split_at: Option<Millis>,
}

/// The accounts, markets and series of one process, and the decisions waiting to be sent.
pub struct Fleet {
    pub markets: Vec<MarketFeed>,
    pub series: Vec<SeriesState>,
    pub accounts: Vec<Account>,
    pub assignments: Vec<Assignment>,
    pub(super) by_address: HashMap<AccountId, u32>,
    pub(super) market_slot: BTreeMap<MarketId, u32>,
    /// Assignments with an order to send, in the order their bars completed.
    pub queue: VecDeque<u32>,
    /// The latest expiry an order signed before this start can carry. Until the log passes it, an
    /// earlier process's order may still enter the log, and no order is placed.
    pub trading_from: Millis,
    /// The latest expiry of a request this process signed whose answer carried the engine's rejection.
    /// The rejection is in the log, which a stop that began first may not have read: it reads the log
    /// past this expiry too, so the rejection counts once.
    pub counts_until: Millis,
    /// The time of the last command read from the log.
    pub log_ts: Millis,
    /// When the accounts were last checked for a read they are due.
    pub next_scan: Millis,
    /// The warm-up reads wanted, for the runner to make and answer with `warmed`.
    pub warm_requests: Vec<WarmRequest>,
    /// Accounts whose read is wanted, and whose saved state is to be written, and that the daily loss
    /// limit has stopped with positions open (to be flattened).
    pub reads: VecDeque<u32>,
    pub saves: Vec<u32>,
    pub flattens: VecDeque<u32>,
    /// Accounts the daily loss limit has stopped, whose positions are closed again on any command.
    pub stopped: Vec<u32>,
    /// How many decisions to trade the fleet has taken.
    pub decisions: u64,
    /// How many accounts have stopped trading.
    pub failed: usize,
    /// Accounts whose status for the console changed since it last heard: a target, a warm-up, a
    /// failure, a day's stop, or a control. Each is listed once, so it never outgrows the capacity
    /// it starts with.
    pub changed: Vec<u32>,
    /// The series the fleet records, once `register_metrics` gives them.
    pub metrics: FleetMetrics,
}

impl Fleet {
    /// The fleet for `accounts` trading `markets`, whose log reading starts at `trading_from`.
    /// Error cases:
    /// - an account trades a market not in `markets`, or two accounts share an address
    pub fn new(markets: &[MarketIn], accounts: Vec<AccountIn>, trading_from: Millis) -> anyhow::Result<Self> {
        let mut feeds: Vec<MarketFeed> = markets
            .iter()
            .map(|m| MarketFeed {
                id: m.spec.id,
                spec: m.spec.clone(),
                marks: IndexMarks::scheduled(m.spec.price_rules, m.spec.schedule.clone()),
                mark: m.mark,
                exact_mark: None,
                exact_from: None,
                pending_moves: Vec::new(),
                first_exact: None,
                series: Vec::new(),
                assignments: Vec::new(),
                unwarmed: 0,
                split_at: None,
            })
            .collect();
        let market_slot: BTreeMap<MarketId, u32> = (0u32..).zip(markets).map(|(slot, m)| (m.spec.id, slot)).collect();
        let mut series: Vec<SeriesState> = Vec::new();
        let mut series_of: BTreeMap<(u32, Millis), u32> = BTreeMap::new();
        let mut fleet_accounts = Vec::with_capacity(accounts.len());
        let mut assignments = Vec::new();
        let mut by_address = HashMap::with_capacity(accounts.len());
        for (index, account) in (0u32..).zip(accounts) {
            for (market, at) in &account.saved.splits {
                if let Some(&slot) = market_slot.get(market) {
                    let split = &mut feeds[slot as usize].split_at;
                    *split = (*split).max(Some(*at));
                }
            }
            let address = account.key.address();
            ensure!(by_address.insert(address, index).is_none(), "{address} is listed twice");
            let first = u32::try_from(assignments.len())?;
            for s in &account.strategies {
                let slot = *market_slot.get(&s.market).with_context(|| format!("market {} is not listed", s.market))?;
                let series_index = *series_of.entry((slot, s.bar_ms)).or_insert_with(|| {
                    let index = u32::try_from(series.len()).unwrap_or(u32::MAX);
                    feeds[slot as usize].series.push(index);
                    series.push(SeriesState {
                        market_slot: slot,
                        length: s.bar_ms,
                        live: None,
                        gaps: GapTracker::new(),
                        strategies: Vec::new(),
                        assignments: Vec::new(),
                        warm: Warm::Waiting,
                        generation: 0,
                        history_bars: 0,
                    });
                    index
                });
                let state = &mut series[series_index as usize];
                let shared = match state.strategies.iter().position(|shared| shared.params == s.params) {
                    Some(shared) => shared,
                    None => {
                        let strategy = s.params.build()?;
                        state.history_bars = state.history_bars.max(s.params.warm_up_bars());
                        state.strategies.push(Shared {
                            params: s.params.clone(),
                            strategy,
                            target: None,
                            changed: false,
                        });
                        state.strategies.len() - 1
                    }
                };
                let assignment = u32::try_from(assignments.len())?;
                state.assignments.push(assignment);
                feeds[slot as usize].assignments.push(assignment);
                assignments.push(Assignment {
                    account: index,
                    series: series_index,
                    strategy: u32::try_from(shared)?,
                    market: s.market,
                    market_slot: slot,
                    max_slippage_bps: s.max_slippage_bps,
                    running: s.running,
                    bounds: s.bounds.clone(),
                    position: s.position,
                    entry: s.entry,
                    close_first: false,
                    expires_at: None,
                    wanted: None,
                    pending: None,
                    queued: false,
                    nonce_retried: None,
                });
            }
            fleet_accounts.push(Account {
                key: account.key,
                address,
                close_on_stop: account.close_on_stop,
                running: account.running,
                changed: false,
                assignments: first..u32::try_from(assignments.len())?,
                signed: VecDeque::with_capacity(2 * account.strategies.len().max(2)),
                closes: Vec::new(),
                halt: Arc::new(AtomicBool::new(false)),
                nonce: account.nonce.map_or(Nonce::Unknown, Nonce::Synced),
                busy: false,
                balance: account.balance,
                controls: account.controls,
                state_file: account.state_file,
                saved: account.saved,
                state_dirty: false,
                state_saving: false,
                state_unsaved: false,
                save_retry_at: None,
                read_wanted: false,
                reading: false,
                read_stale: false,
                epoch: 0,
                flatten_queued: false,
                behind: false,
                read_after: Millis::MIN,
                flatten_after: Millis::MIN,
                flatten_bound: None,
                loss_alert: false,
                cash_alert: false,
                retry_at: None,
                failed: None,
            });
        }
        for state in &series {
            feeds[state.market_slot as usize].unwarmed += 1;
        }
        let (queue, accounts_len) = (VecDeque::with_capacity(assignments.len()), fleet_accounts.len());
        Ok(Self {
            markets: feeds,
            series,
            accounts: fleet_accounts,
            assignments,
            by_address,
            market_slot,
            queue,
            trading_from,
            counts_until: Millis::MIN,
            log_ts: Millis::MIN,
            next_scan: Millis::MIN,
            warm_requests: Vec::with_capacity(8),
            reads: VecDeque::with_capacity(accounts_len),
            saves: Vec::with_capacity(accounts_len),
            flattens: VecDeque::with_capacity(accounts_len),
            stopped: Vec::new(),
            decisions: 0,
            failed: 0,
            changed: Vec::with_capacity(accounts_len),
            metrics: FleetMetrics::default(),
        })
    }

    /// The slot of the account `address` names, if the fleet trades it.
    pub fn index_of(&self, address: AccountId) -> Option<u32> {
        self.by_address.get(&address).copied()
    }

    /// Notes that account `account`'s status for the console changed.
    pub fn mark_changed(&mut self, account: u32) {
        let entry = &mut self.accounts[account as usize];
        if !entry.changed {
            entry.changed = true;
            self.changed.push(account);
        }
    }

    /// The slot of `address`'s account, if the fleet trades it.
    pub fn account_of(&self, address: &str) -> Option<u32> {
        self.by_address.get(&address.parse::<AccountId>().ok()?).copied()
    }

    /// The assignment of account `account` in `market`.
    pub fn assignment_of(&self, account: u32, market: MarketId) -> Option<u32> {
        self.accounts[account as usize].assignments.clone().find(|&a| self.assignments[a as usize].market == market)
    }
}
