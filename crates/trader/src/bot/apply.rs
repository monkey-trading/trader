//! Applying one message of the log to the fleet: a `Prices` command for a market the fleet trades
//! stores its quotes, a new listing of such a market changes its rules and hours, the accounts'
//! fills move their positions, and the command's time may complete bars.

use engine_core::command::{AdminCommand, Command};
use engine_core::event::Event;
use engine_core::types::Millis;
use serde_json::value::RawValue;
use tracing::info;

use crate::bot::fleet::{Fleet, Warm};
use crate::bot::{Halt, bar_fits, supported};
use crate::stream::message::{CommandMessage, Envelope, Offset, for_each_event, store_prices};

/// How much sequenced time passes between checks of which accounts are due a read.
const SCAN_EVERY_MS: Millis = 1_000;

/// Why a message's events could not be applied.
enum EventsError {
    Halt(Halt),
    Unreadable(anyhow::Error),
}

impl Fleet {
    /// Applies one message of the log, sent by the node at `node_time` as `at`.
    /// Error cases:
    /// - a command or its events the pinned engine cannot read
    /// - a new listing makes a market one the bot does not trade,
    ///   (an account it cannot size for, or a bar length it no longer fits, stops only that account)
    /// - a corporate action that changes a traded market's units or fails
    pub fn apply(&mut self, msg: &str, at: Offset, node_time: u64) -> Result<(), Halt> {
        let unreadable = |e: anyhow::Error| Halt::Unfollowable(e.context(format!("the log message at {at}")));
        let envelope: Envelope<'_> = serde_json::from_str(msg).map_err(|e| unreadable(e.into()))?;
        if envelope.kind != "command" {
            return Ok(());
        }
        let message = envelope.command().map_err(unreadable)?;
        self.log_ts = message.ts;
        let command = message.head().map_err(unreadable)?;
        // Every command brings each book to its time before it acts: a closure between two commands
        // drops the quotes from before it.
        for feed in &mut self.markets {
            feed.marks.advance(message.ts);
        }
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

    /// Takes the rules and hours of a listing that an applied admin command at `ts` sets, for a
    /// market the fleet follows.
    /// Error cases:
    /// - the new listing is one the bot cannot follow; a bar length that no longer fits its hours or a
    ///   sizing an account's controls refuse stops only the accounts it concerns
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
        // What an account cannot follow stops that account; the log stays followable for the others.
        let mut refused = Vec::new();
        let feed = &self.markets[slot as usize];
        for &series in &feed.series {
            if let Err(e) = bar_fits(spec, self.series[series as usize].length, ts) {
                let state = &self.series[series as usize];
                refused
                    .extend(state.assignments.iter().map(|&a| (self.assignments[a as usize].account, e.to_string())));
            }
        }
        for &a in &feed.assignments {
            let account = self.assignments[a as usize].account;
            if let Err(e) = self.accounts[account as usize].controls.check_market(spec) {
                refused.push((account, format!("sizing for {}: {e}", spec.symbol)));
            }
        }
        for (account, reason) in refused {
            self.fail(account, format!("the market's new listing: {reason}"));
        }
        info!(market = spec.id, rules = ?spec.price_rules, "the market's listing changed");
        let feed = &mut self.markets[slot as usize];
        feed.marks.set_rules(ts, spec.price_rules);
        feed.marks.set_schedule(ts, spec.schedule.clone());
        feed.spec = spec.clone();
        Ok(())
    }

    /// Moves every market's index to the command at `ts`, and every series' bars: only a move of the
    /// exact index goes into the bars, as the node's marks history records only moves, so bars built
    /// live and bars warmed up from the history are built alike. The first exact value is where
    /// moves start from; the history holds what came before it. `node_time` is the node's time when
    /// it sent the command.
    fn advance(&mut self, ts: Millis, node_time: Millis) {
        let lag = node_time.saturating_sub(ts);
        for slot in 0..self.markets.len() {
            let feed = &mut self.markets[slot];
            let mark = feed.marks.mark(ts);
            // Once warmed up without ever having seen an exact value, the first one is a move: nothing
            // came before it, and the node's history holds it.
            let blind = feed.unwarmed == 0 && feed.exact_mark.is_none();
            let moved = mark.filter(|mark| blind || feed.exact_mark.is_some_and(|last| last != *mark));
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
                let bar = state.live.as_mut().and_then(|live| live.on_command(ts, moved));
                if let Some(bar) = bar {
                    self.bar_closed(series, bar, ts, lag);
                }
            }
            if moved.is_some() {
                self.observe_market(slot, ts);
            }
        }
        if ts >= self.next_scan {
            self.next_scan = ts.saturating_add(SCAN_EVERY_MS);
            self.scan(ts);
        }
        self.retry_flattens(lag);
    }

    /// Applies each event of the message: the accounts' fills and cash, and the traded markets' corporate
    /// actions.
    /// Error cases:
    /// - an event the bot cannot read, or one that halts it
    fn apply_events(&mut self, message: &CommandMessage<'_>, events: &RawValue) -> Result<(), EventsError> {
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
