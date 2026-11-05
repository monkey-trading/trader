//! Many accounts on one log. The fleet follows the public log once: each `Prices` command moves its
//! market's index mark, which goes into the bars of every bar length some account trades, whose
//! averages every strategy of that market and length shares. A completed bar is decided for each
//! account that reads it. A decision to trade waits in a queue in bar order until the dispatcher
//! sends it (`dispatch`); the fleet itself does no IO, and once warm allocates nothing per message.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;

use anyhow::{Context, ensure};
use engine_core::Fixed;
use engine_core::command::{AdminCommand, Command};
use engine_core::eip712::Signature;
use engine_core::event::Event;
use engine_core::market::PriceRules;
use engine_core::types::{AccountId, MarketId, Millis};
use tokio::time::Instant;
use tracing::{debug, info};
use trader_core::bars::Bar;
use trader_core::marks::IndexMarks;
use trader_core::series::Series;
use trader_core::strategy::{Target, order_for};

use crate::bot::warm::WarmRequest;
use crate::bot::{Halt, supported};
use crate::keys::AccountKey;
use crate::stream::message::{CommandMessage, Envelope, Offset, for_each_event, store_prices};

/// How far behind the node's time a bar may close and still be acted on; one further behind is
/// history the stream is replaying, quoted at marks long gone.
pub const REPLAY_LAG_MS: Millis = 30_000;
/// How long after the node's time a signed request may still be sequenced: long enough to reach a
/// node over a slow link, and shorter than the shortest bar, so a live bot's next bar is past it.
pub const EXPIRES_IN_MS: Millis = 30_000;

/// A market the fleet follows, as the node lists it.
#[derive(Clone, Debug)]
pub struct MarketIn {
    pub id: MarketId,
    pub rules: PriceRules,
    /// The node's mark at startup, which orders are quoted at until the log gives one.
    pub mark: Option<Fixed>,
}

/// One account to trade, with the markets it trades.
pub struct AccountIn {
    pub key: Arc<AccountKey>,
    pub close_on_stop: bool,
    pub balance: Fixed,
    pub strategies: Vec<StrategyIn>,
}

/// One market and strategy of an account, and the account's position in the market.
#[derive(Clone, Debug)]
pub struct StrategyIn {
    pub market: MarketId,
    pub bar_ms: Millis,
    pub fast: usize,
    pub slow: usize,
    pub qty: Fixed,
    pub max_slippage_bps: i64,
    pub position: Fixed,
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
    /// Its strategies in `Fleet::assignments`.
    pub assignments: std::ops::Range<u32>,
    /// The signatures of the last orders this process signed for the account, to tell its fills
    /// from others': a signature is over the whole request, so no other request carries one of them.
    pub signed: VecDeque<Signature>,
    pub nonce: Nonce,
    /// Whether a request for the account is out, so no other goes before it answers.
    pub busy: bool,
    /// The account's balance, as the last verified read gave it.
    pub balance: Fixed,
    /// Whether the account's fills since the last read of its balance are unread.
    pub balance_stale: bool,
    pub balance_reading: bool,
    /// The earliest a failed nonce read is tried again.
    pub retry_at: Option<Instant>,
    /// Why the account stopped trading, when it has.
    pub failed: Option<String>,
}

/// One market and strategy of one account.
pub struct Assignment {
    pub account: u32,
    pub series: u32,
    pub fast: usize,
    pub slow: usize,
    pub market: MarketId,
    pub market_slot: u32,
    pub qty: Fixed,
    pub max_slippage_bps: i64,
    pub position: Fixed,
    /// Whether the next reversal is only a close: the engine refused the last one whole.
    pub close_first: bool,
    /// The expiry of the last order signed here. Until the log passes it, that order may still enter
    /// the log, so its fill may still be to come, and no other order is placed.
    pub expires_at: Option<Millis>,
    pub pending: Option<Pending>,
    pub queued: bool,
}

/// A decision to trade toward `target`, taken when the bar starting at `bar_start` was completed by
/// the command at `ts`.
#[derive(Clone, Copy, Debug)]
pub struct Pending {
    pub target: Target,
    pub bar_start: Millis,
    pub ts: Millis,
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

/// The bars of a market at a bar length, and the assignments that trade on them.
pub struct SeriesState {
    pub market_slot: u32,
    pub series: Series,
    pub assignments: Vec<u32>,
    pub warm: Warm,
}

/// A market's index and the series built on it.
pub struct MarketFeed {
    pub id: MarketId,
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
    pub unwarmed: usize,
}

/// The accounts, markets and series of one process, and the decisions waiting to be sent.
pub struct Fleet {
    pub markets: Vec<MarketFeed>,
    pub series: Vec<SeriesState>,
    pub accounts: Vec<Account>,
    pub assignments: Vec<Assignment>,
    by_address: HashMap<AccountId, u32>,
    market_slot: BTreeMap<MarketId, u32>,
    /// Assignments with a decision to send, in the order their bars completed.
    pub queue: VecDeque<u32>,
    /// The latest expiry an order signed before this start can carry. Until the log passes it, an
    /// earlier process's order may still enter the log, and no order is placed.
    pub trading_from: Millis,
    /// The time of the last command read from the log.
    pub log_ts: Millis,
    /// The warm-up reads wanted, for the runner to make and answer with `warmed`.
    pub warm_requests: Vec<WarmRequest>,
    /// How many decisions to trade the fleet has taken.
    pub decisions: u64,
    /// How many accounts have stopped trading.
    pub failed: usize,
    /// Accounts that have filled since their balance was last read.
    pub stale_balances: Vec<u32>,
}

impl Fleet {
    /// The fleet for `accounts` trading `markets`, whose log reading starts at `trading_from`.
    /// Error cases:
    /// - an account trades a market not in `markets`, or two accounts share an address
    /// - a strategy's periods are refused
    pub fn new(markets: &[MarketIn], accounts: Vec<AccountIn>, trading_from: Millis) -> anyhow::Result<Self> {
        let mut feeds: Vec<MarketFeed> = markets
            .iter()
            .map(|m| MarketFeed {
                id: m.id,
                marks: IndexMarks::new(m.rules),
                mark: m.mark,
                exact_mark: None,
                exact_from: None,
                pending_moves: Vec::new(),
                first_exact: None,
                series: Vec::new(),
                unwarmed: 0,
            })
            .collect();
        let market_slot: BTreeMap<MarketId, u32> = (0u32..).zip(markets).map(|(slot, m)| (m.id, slot)).collect();
        // The periods each series needs, before any average is built.
        let mut periods: BTreeMap<(u32, Millis), Vec<usize>> = BTreeMap::new();
        for account in &accounts {
            for s in &account.strategies {
                let slot = *market_slot.get(&s.market).with_context(|| format!("market {} is not listed", s.market))?;
                periods.entry((slot, s.bar_ms)).or_default().extend([s.fast, s.slow]);
            }
        }
        let mut series: Vec<SeriesState> = Vec::with_capacity(periods.len());
        let mut series_of = BTreeMap::new();
        for ((slot, bar_ms), periods) in periods {
            let index = u32::try_from(series.len())?;
            series_of.insert((slot, bar_ms), index);
            feeds[slot as usize].series.push(index);
            series.push(SeriesState {
                market_slot: slot,
                series: Series::new(bar_ms, &periods),
                assignments: Vec::new(),
                warm: Warm::Waiting,
            });
        }
        let mut fleet_accounts = Vec::with_capacity(accounts.len());
        let mut assignments = Vec::new();
        let mut by_address = HashMap::with_capacity(accounts.len());
        for (index, account) in (0u32..).zip(accounts) {
            let address = account.key.address();
            ensure!(by_address.insert(address, index).is_none(), "{address} is listed twice");
            let first = u32::try_from(assignments.len())?;
            for s in &account.strategies {
                let slot = market_slot[&s.market];
                let series_index = series_of[&(slot, s.bar_ms)];
                let state = &mut series[series_index as usize];
                let (fast, slow) = (state.series.slot(s.fast), state.series.slot(s.slow));
                state.assignments.push(u32::try_from(assignments.len())?);
                assignments.push(Assignment {
                    account: index,
                    series: series_index,
                    fast: fast.context("fast period")?,
                    slow: slow.context("slow period")?,
                    market: s.market,
                    market_slot: slot,
                    qty: s.qty,
                    max_slippage_bps: s.max_slippage_bps,
                    position: s.position,
                    close_first: false,
                    expires_at: None,
                    pending: None,
                    queued: false,
                });
            }
            fleet_accounts.push(Account {
                key: account.key,
                address,
                close_on_stop: account.close_on_stop,
                assignments: first..u32::try_from(assignments.len())?,
                signed: VecDeque::with_capacity(2 * account.strategies.len().max(2)),
                nonce: Nonce::Unknown,
                busy: false,
                balance: account.balance,
                balance_stale: false,
                balance_reading: false,
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
            log_ts: Millis::MIN,
            warm_requests: Vec::with_capacity(8),
            stale_balances: Vec::with_capacity(accounts_len),
            decisions: 0,
            failed: 0,
        })
    }

    /// The slot of `address`'s account, if the fleet trades it.
    pub fn account_of(&self, address: &str) -> Option<u32> {
        self.by_address.get(&address.parse::<AccountId>().ok()?).copied()
    }

    /// The assignment of account `account` in `market`.
    pub fn assignment_of(&self, account: u32, market: MarketId) -> Option<u32> {
        self.accounts[account as usize].assignments.clone().find(|&a| self.assignments[a as usize].market == market)
    }

    /// Stops `account` from trading, leaving it as it is, and says why.
    pub fn fail(&mut self, account: u32, reason: String) {
        let entry = &mut self.accounts[account as usize];
        if entry.failed.is_none() {
            tracing::error!(alert = "account_failed", account = %entry.address, "{reason}; the account is left as it is");
            entry.failed = Some(reason);
            self.failed += 1;
        }
        for a in entry.assignments.clone() {
            self.assignments[a as usize].pending = None;
        }
    }

    /// Applies one message of the log, sent by the node at `node_time` as `at`: a `Prices` command
    /// for a market the fleet trades stores its quotes, a new listing of such a market changes its
    /// rules, the accounts' fills move their positions, and the command's time may complete bars,
    /// which queue the orders that move accounts toward their targets.
    /// Error cases:
    /// - a command or its events the pinned engine cannot read
    /// - a new listing makes a market one the bot does not trade
    pub fn apply(&mut self, msg: &str, at: Offset, node_time: u64) -> Result<(), Halt> {
        let unreadable = |e: anyhow::Error| Halt::Unfollowable(e.context(format!("the log message at {at}")));
        let envelope: Envelope<'_> = serde_json::from_str(msg).map_err(|e| unreadable(e.into()))?;
        if envelope.kind != "command" {
            return Ok(());
        }
        let message = envelope.command().map_err(unreadable)?;
        self.log_ts = message.ts;
        let command = message.head().map_err(unreadable)?;
        match (command.kind, command.market) {
            ("prices", Some(market)) => {
                if let Some(&slot) = self.market_slot.get(&market) {
                    store_prices(message.cmd.get(), message.ts, &mut self.markets[slot as usize].marks)
                        .map_err(unreadable)?;
                }
            }
            ("admin" | "signed_admin", _) => {
                self.relist(message.ts, &message.command().map_err(unreadable)?, &message).map_err(unreadable)?;
            }
            _ => {}
        }
        if let Some(events) = message.events.filter(|events| events.get() != "[]") {
            self.apply_events(&message, events).map_err(|e| match e {
                EventsError::Halt(halt) => halt,
                EventsError::Unreadable(e) => unreadable(e),
            })?;
        }
        let node_time = Millis::try_from(node_time).unwrap_or(Millis::MAX);
        self.advance(message.ts, node_time);
        Ok(())
    }

    /// Takes the price rules of a listing that an applied admin command at `ts` sets, for a market
    /// the fleet follows.
    /// Error cases:
    /// - the new listing is one the bot does not trade
    fn relist(&mut self, ts: Millis, command: &Command, message: &CommandMessage<'_>) -> anyhow::Result<()> {
        let admin = match command {
            Command::Admin(admin) => admin,
            Command::SignedAdmin(request) => &request.command,
            _ => return Ok(()),
        };
        let AdminCommand::UpsertMarket(spec) = admin else { return Ok(()) };
        let Some(&slot) = self.market_slot.get(&spec.id) else { return Ok(()) };
        if !message.events()?.contains(&Event::AdminApplied) {
            return Ok(());
        }
        supported(spec)?;
        info!(market = spec.id, rules = ?spec.price_rules, "the market's listing changed");
        self.markets[slot as usize].marks.set_rules(ts, spec.price_rules);
        Ok(())
    }

    /// Moves every market's index to the command at `ts`, and every series' bars: only a move of the
    /// exact index goes into the bars, as the node's marks history records only moves, so bars built
    /// live and bars warmed up from the history are built alike. The first exact value is where
    /// moves start from; the history holds what came before it. `lag` is how far behind the node's
    /// time the command is.
    fn advance(&mut self, ts: Millis, node_time: Millis) {
        let lag = node_time.saturating_sub(ts);
        for slot in 0..self.markets.len() {
            let feed = &mut self.markets[slot];
            let mark = feed.marks.mark(ts);
            let moved = mark.filter(|mark| feed.exact_mark.is_some_and(|last| last != *mark));
            // Until the market's series have warmed up, the first exact value is kept: the history
            // read after it may not reach it, and then it is a move (see `warmed`).
            if let (Some(mark), None, true) = (mark, feed.exact_mark, feed.unwarmed > 0) {
                feed.first_exact = Some((ts, mark));
            }
            if mark.is_some() {
                (feed.mark, feed.exact_mark) = (mark, mark);
            }
            if feed.unwarmed > 0 {
                self.note_for_warm_up(slot, ts, moved, node_time);
            }
            for k in 0..self.markets[slot].series.len() {
                let series = self.markets[slot].series[k];
                let state = &mut self.series[series as usize];
                if state.warm != Warm::Done {
                    continue;
                }
                let bar = state.series.live.as_mut().and_then(|live| live.on_command(ts, moved));
                if let Some(bar) = bar {
                    self.bar_closed(series, bar, ts, lag);
                }
            }
        }
    }

    /// Hands a completed bar to its series' averages and decides each account that trades on it. A
    /// bar the stream is replaying from history places no order: `lag` is past `REPLAY_LAG_MS`.
    pub(super) fn bar_closed(&mut self, series: u32, bar: Bar, ts: Millis, lag: Millis) {
        let state = &mut self.series[series as usize];
        state.series.push_close(bar.close);
        for k in 0..state.assignments.len() {
            let a = self.series[series as usize].assignments[k];
            let asg = &self.assignments[a as usize];
            let state = &self.series[series as usize];
            let target = state.series.crossover(asg.fast, asg.slow);
            if k == 0 {
                let (fast, slow) = state.series.averages(asg.fast, asg.slow);
                debug!(market = asg.market, start = bar.start, close = %bar.close, ?fast, ?slow, ?target, lag, "bar");
            }
            let Some(target) = target else { continue };
            self.decide(a, target, bar.start, ts, lag);
        }
    }

    /// Decides whether assignment `a` orders toward `target`, and if so queues the order.
    fn decide(&mut self, a: u32, target: Target, bar_start: Millis, ts: Millis, lag: Millis) {
        let asg = &mut self.assignments[a as usize];
        let account = &self.accounts[asg.account as usize];
        asg.pending = None;
        if account.failed.is_some() || order_for(target, asg.qty, asg.position, asg.close_first).is_none() {
            return;
        }
        if lag > REPLAY_LAG_MS {
            info!(account = %account.address, market = asg.market, ?target, lag, "the bar is history the stream is replaying: no order");
        } else if let Some(expires_at) = asg.expires_at.filter(|expires_at| ts <= *expires_at) {
            info!(account = %account.address, market = asg.market, ?target, expires_at, "the last order may still enter the log: no order");
        } else if ts <= self.trading_from {
            let trading_from = self.trading_from;
            info!(account = %account.address, market = asg.market, ?target, trading_from, "an order signed before this start may still enter the log: no order");
        } else {
            asg.pending = Some(Pending { target, bar_start, ts });
            self.decisions += 1;
            if !asg.queued {
                asg.queued = true;
                self.queue.push_back(a);
            }
        }
    }

    /// Applies each event of the message: the accounts' fills.
    /// Error cases:
    /// - an event the bot cannot read, or one that halts it
    fn apply_events(
        &mut self,
        message: &CommandMessage<'_>,
        events: &serde_json::value::RawValue,
    ) -> Result<(), EventsError> {
        let mut halt = None;
        let result = for_each_event(events, |event| {
            if let Err(h) = self.apply_event(message, event) {
                halt = Some(h);
                anyhow::bail!("halt");
            }
            Ok(())
        });
        match (halt, result) {
            (Some(Halt::Unfollowable(e)), _) => Err(EventsError::Unreadable(e)),
            (Some(h), _) => Err(EventsError::Halt(h)),
            (None, Err(e)) => Err(EventsError::Unreadable(e)),
            (None, Ok(())) => Ok(()),
        }
    }
}

/// Why a message's events could not be applied.
enum EventsError {
    Halt(Halt),
    Unreadable(anyhow::Error),
}
