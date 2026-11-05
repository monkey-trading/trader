//! A market's index mark, derived from the public log's `Prices` commands as the engine derives it,
//! for a market priced from spot quotes: the venue quotes each command stores, and the median of the
//! fresh ones (`engine_core::price::compute_index`) at each command's time. A market with trading
//! hours keeps only the quotes from one open session, and has no mark while it is shut.

use std::collections::BTreeMap;

use engine_core::Fixed;
use engine_core::market::PriceRules;
use engine_core::price::{MAX_QUOTE_LEAD_MS, SourceQuote, compute_index};
use engine_core::session::Schedule;
use engine_core::types::Millis;

/// One market's stored venue quotes, as the engine holds them.
#[derive(Clone, Debug)]
pub struct IndexMarks {
    rules: PriceRules,
    schedule: Schedule,
    /// The time of the last command seen, for the closures between it and the next.
    last_now: Option<Millis>,
    /// The latest quote per venue.
    quotes: BTreeMap<String, SourceQuote>,
    /// The time of the first command seen. A quote that counts at a later time `t` was sequenced
    /// no earlier than `t - max_age_ms`, so after `first + max_age_ms` the book holds every one.
    first: Option<Millis>,
    /// The last mark worked out, and the latest time it holds through while no quote is stored:
    /// until a fresh quote ages out, or the book becomes complete.
    known: Option<(Option<Fixed>, Millis)>,
}

impl IndexMarks {
    /// An empty book for a market priced under `rules` that never closes.
    pub fn new(rules: PriceRules) -> Self {
        Self::scheduled(rules, Schedule::AlwaysOpen)
    }

    /// An empty book for a market priced under `rules` that trades in `schedule`'s hours.
    pub fn scheduled(rules: PriceRules, schedule: Schedule) -> Self {
        Self { rules, schedule, last_now: None, quotes: BTreeMap::new(), first: None, known: None }
    }

    /// Whether the market stays open from a quote's venue stamp to the sequenced time `now`, as the
    /// engine checks it (`MarketState::in_session`); a span longer than the age limit holds only a
    /// quote too old to count, so only its two ends are checked.
    fn in_session(&self, ts: Millis, now: Millis) -> bool {
        let (first, last) = (ts.min(now), ts.max(now));
        if last.saturating_sub(first) > self.rules.max_age_ms {
            return self.schedule.is_open(first) && self.schedule.is_open(last);
        }
        self.schedule.open_throughout(first, last)
    }

    /// Takes the hours a new listing of the market sets, from the command at `now` that applies it
    /// on, and drops the quotes the new hours would not have let in.
    pub fn set_schedule(&mut self, now: Millis, schedule: Schedule) {
        self.schedule = schedule;
        let max_age = self.rules.max_age_ms;
        let stale: Vec<String> = (self.quotes.iter())
            .filter(|(_, quote)| !(now - quote.ts <= max_age && self.in_session(quote.ts, now)))
            .map(|(source, _)| source.clone())
            .collect();
        stale.iter().for_each(|source| _ = self.quotes.remove(source));
        self.known = None;
    }

    /// Brings the book to the command at `now`, which every command of the log does before it acts:
    /// where the market was not open throughout since the last command, a closure the log may not
    /// have shown, no quote from before it counts, so those are dropped. A gap longer than the age
    /// limit checks only the window after the last command and the instant `now`: later, every quote
    /// held is stale anyway.
    pub fn advance(&mut self, now: Millis) {
        let Some(last_now) = self.last_now.replace(now) else { return };
        if matches!(self.schedule, Schedule::AlwaysOpen) || !self.quotes.values().any(|quote| quote.ts <= last_now) {
            return;
        }
        let window = self.rules.max_age_ms;
        let closed = if now - last_now > window {
            !(self.schedule.open_throughout(last_now, last_now + window) && self.schedule.is_open(now))
        } else {
            !self.schedule.open_throughout(last_now, now)
        };
        if closed {
            self.quotes.retain(|_, quote| quote.ts > last_now);
            self.known = None;
        }
    }

    /// Takes the rules a new listing of the market sets, from the command at `now` that applies it
    /// on. A raised age limit first drops the quotes already stale under the old one, as the engine
    /// does from protocol `V33`, so none of them counts again.
    pub fn set_rules(&mut self, now: Millis, rules: PriceRules) {
        let old_max_age = self.rules.max_age_ms;
        if rules.max_age_ms > old_max_age {
            self.quotes.retain(|_, quote| quote.ts > now || now - quote.ts <= old_max_age);
        }
        self.rules = rules;
        self.known = None;
    }

    /// Stores the quotes of a `Prices` command for this market sequenced at `now`, refusing those
    /// the engine refuses: malformed ones, those for a futures contract, those stamped more than
    /// `MAX_QUOTE_LEAD_MS` ahead, and those from outside one open session. A quote stamped ahead counts from `now`, and replaces its venue's
    /// only when it is no older.
    pub fn store(&mut self, now: Millis, quotes: &[SourceQuote]) {
        self.first.get_or_insert(now);
        self.known = None;
        for quote in quotes {
            let refused = !quote.is_well_formed() || quote.contract.is_some();
            if refused || quote.ts.saturating_sub(now) > MAX_QUOTE_LEAD_MS || !self.in_session(quote.ts, now) {
                continue;
            }
            let ts = quote.ts.min(now);
            match self.quotes.get_mut(&quote.source) {
                Some(held) if ts < held.ts && held.ts <= now => {}
                Some(held) => {
                    held.mid = quote.mid;
                    held.ts = ts;
                }
                None => {
                    self.quotes.insert(quote.source.clone(), SourceQuote { ts, ..quote.clone() });
                }
            }
        }
    }

    /// Whether the book holds, at `now`, every quote that can count: more than `max_age_ms` of the
    /// log has passed since the first command it saw.
    pub fn is_complete(&self, now: Millis) -> bool {
        self.first.is_some_and(|first| now.saturating_sub(first) > self.rules.max_age_ms)
    }

    /// The index mark at `now`, any command's sequenced time: `None` until the book has seen
    /// `max_age_ms` of the log, or while the fresh quotes give no index. Worked out again only once
    /// a quote is stored or a fresh one ages out, so most commands cost a comparison.
    pub fn mark(&mut self, now: Millis) -> Option<Fixed> {
        if !matches!(self.schedule, Schedule::AlwaysOpen) {
            // Hours turn the index on and off between quotes, so it is worked out at every command.
            self.known = None;
            let first = *self.first.get_or_insert(now);
            let complete = now.saturating_sub(first) > self.rules.max_age_ms;
            let open = self.schedule.is_open(now);
            return (complete && open)
                .then(|| compute_index(&self.quotes, &self.rules, now).ok())
                .flatten()
                .map(|i| i.mid);
        }
        if let Some((mark, through)) = self.known
            && now <= through
        {
            return mark;
        }
        let max_age = self.rules.max_age_ms;
        let first = *self.first.get_or_insert(now);
        let known = if !self.is_complete(now) {
            (None, first.saturating_add(max_age))
        } else {
            let mark = compute_index(&self.quotes, &self.rules, now).ok().map(|index| index.mid);
            // Stored quotes are stamped at or before `now`, so only an ageing one changes the index.
            let ages_out = self.quotes.values().map(|q| q.ts.saturating_add(max_age)).filter(|at| *at >= now).min();
            (mark, ages_out.unwrap_or(Millis::MAX))
        };
        self.known = Some(known);
        known.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULES: PriceRules = PriceRules { max_age_ms: 1_000, max_divergence_bps: 25, min_sources: 2 };

    fn quote(source: &str, mid: &str, ts: Millis) -> SourceQuote {
        SourceQuote { source: source.into(), mid: mid.parse().unwrap(), ts, contract: None }
    }

    fn fx(text: &str) -> Fixed {
        text.parse().unwrap()
    }

    /// No mark until more than `max_age_ms` of the log has passed, then the median of fresh quotes.
    #[test]
    fn marks_only_once_the_book_is_complete() {
        let mut marks = IndexMarks::new(RULES);
        marks.store(10_000, &[quote("a", "100", 10_000), quote("b", "100.02", 10_000), quote("c", "100.04", 10_000)]);
        assert_eq!(marks.mark(10_000), None);
        assert_eq!(marks.mark(11_000), None);
        assert!(!marks.is_complete(11_000));
        assert!(marks.is_complete(11_001));
        marks.store(11_001, &[quote("a", "100.01", 11_001)]);
        // b and c are 1001ms old and drop out, leaving one source: no index.
        assert_eq!(marks.mark(11_001), None);
        marks.store(11_002, &[quote("b", "100.03", 11_002), quote("c", "100.05", 11_002)]);
        assert_eq!(marks.mark(11_002), Some(fx("100.03")));
    }

    /// Two fresh venues give their mean; a quote ages out at `max_age_ms` past its stamp.
    #[test]
    fn quotes_age_out() {
        let mut marks = IndexMarks::new(RULES);
        marks.store(0, &[]);
        marks.store(5_000, &[quote("a", "100", 5_000), quote("b", "101", 5_000)]);
        // 101 and 100 diverge by 99 bps: no index.
        assert_eq!(marks.mark(5_000), None);
        marks.store(5_100, &[quote("b", "100.1", 5_100)]);
        assert_eq!(marks.mark(5_100), Some(fx("100.05")));
        assert_eq!(marks.mark(6_000), Some(fx("100.05")));
        assert_eq!(marks.mark(6_001), None);
    }

    /// A quote stamped ahead counts from the command's time; one too far ahead, malformed, or for a
    /// futures contract is refused; an older quote does not replace a newer one.
    #[test]
    fn stores_as_the_engine_does() {
        let mut marks = IndexMarks::new(RULES);
        marks.store(0, &[]);
        let contract = SourceQuote { contract: Some("H7".into()), ..quote("e", "100", 5_000) };
        marks.store(
            5_000,
            &[
                quote("a", "100", 5_900),
                quote("b", "100", 6_001),
                quote("c", "0", 5_000),
                quote("d", "100.02", 4_990),
                contract,
            ],
        );
        assert_eq!(marks.quotes.keys().collect::<Vec<_>>(), ["a", "d"]);
        assert_eq!(marks.quotes["a"].ts, 5_000);
        marks.store(5_010, &[quote("d", "99", 4_980)]);
        assert_eq!(marks.quotes["d"].mid, fx("100.02"));
        assert_eq!(marks.mark(5_010), Some(fx("100.01")));
    }

    /// New rules apply from the command that sets them: three sources needed leaves two with no index.
    #[test]
    fn new_rules_apply_at_once() {
        let mut marks = IndexMarks::new(RULES);
        marks.store(0, &[]);
        marks.store(5_000, &[quote("a", "100", 5_000), quote("b", "100", 5_000)]);
        assert_eq!(marks.mark(5_000), Some(fx("100")));
        marks.set_rules(5_001, PriceRules { min_sources: 3, ..RULES });
        assert_eq!(marks.mark(5_001), None);
    }

    /// A raised age limit does not bring back a quote stale under the old one; a quote still fresh
    /// under the old limit counts under the new one as it ages.
    #[test]
    fn a_raised_age_limit_keeps_stale_quotes_out() {
        let mut marks = IndexMarks::new(RULES);
        marks.store(0, &[]);
        marks.store(5_000, &[quote("a", "100", 5_000)]);
        marks.store(6_500, &[quote("b", "100.02", 6_500), quote("c", "100.04", 6_500)]);
        // At 6,500, a is 1,500 ms old: stale under 1,000, fresh under 5,000, and dropped.
        marks.set_rules(6_500, PriceRules { max_age_ms: 5_000, ..RULES });
        assert_eq!(marks.quotes.keys().collect::<Vec<_>>(), ["b", "c"]);
        assert_eq!(marks.mark(9_000), Some(fx("100.03")));
        // Lowering the limit drops nothing: age alone keeps a quote out.
        marks.set_rules(9_000, RULES);
        assert_eq!(marks.quotes.len(), 2);
        assert_eq!(marks.mark(9_000), None);
    }

    // Mon 2026-11-02 14:30:00 UTC, in milliseconds, and the exchange's hours: Monday to Friday 14:30 to
    // 21:00 UTC, with a halt on Monday from 15:00 to 15:05.
    const OPEN: Millis = 1_793_577_600_000 + 14 * 3_600_000 + 30 * 60_000;
    const HALT: Millis = OPEN + 30 * 60_000;
    const MINUTE: Millis = 60_000;

    fn exchange() -> Schedule {
        let windows: Vec<_> = ["Mon", "Tue", "Wed", "Thu", "Fri"]
            .iter()
            .map(|day| serde_json::json!({ "open": format!("{day} 14:30"), "close": format!("{day} 21:00") }))
            .collect();
        let closures = [serde_json::json!({ "from": "2026-11-02T15:00:00", "to": "2026-11-02T15:05:00" })];
        let schedule = serde_json::json!({ "kind": "weekly", "tz": "UTC", "windows": windows, "closures": closures });
        serde_json::from_value(schedule).unwrap()
    }

    const HOURS: PriceRules = PriceRules { max_age_ms: 10 * MINUTE, max_divergence_bps: 25, min_sources: 2 };

    fn scheduled() -> IndexMarks {
        let mut marks = IndexMarks::scheduled(HOURS, exchange());
        marks.advance(OPEN - 20 * MINUTE);
        marks
    }

    /// A quote must come from one open session: stamped before the open and sequenced after it is
    /// refused, stamped inside is kept, and one stamped in the halt and sequenced after it is refused.
    #[test]
    fn quotes_from_outside_a_session_are_refused() {
        let mut marks = scheduled();
        marks.advance(OPEN + 1_000);
        marks.store(
            OPEN + 1_000,
            &[quote("a", "100", OPEN - 1), quote("b", "100", OPEN), quote("c", "100", OPEN + 500)],
        );
        assert_eq!(marks.quotes.keys().collect::<Vec<_>>(), ["b", "c"]);
        marks.advance(HOUR_AFTER_HALT);
        marks.store(HOUR_AFTER_HALT, &[quote("d", "100", HALT + MINUTE), quote("e", "100", HOUR_AFTER_HALT)]);
        assert_eq!(marks.quotes.keys().collect::<Vec<_>>(), ["b", "c", "e"]);
    }

    const HOUR_AFTER_HALT: Millis = HALT + 5 * MINUTE + 10_000;

    /// No mark while the market is shut, though the quotes held are fresh, and none before the book
    /// is complete; after a halt the mark comes from quotes stamped after it.
    #[test]
    fn no_mark_while_shut() {
        let mut marks = scheduled();
        let quotes = |ts| [quote("a", "100", ts), quote("b", "100.02", ts)];
        marks.advance(OPEN);
        marks.store(OPEN, &quotes(OPEN));
        assert_eq!(marks.mark(OPEN), None);
        let later = OPEN + 10 * MINUTE + 1;
        marks.advance(later);
        marks.store(later, &quotes(later));
        assert_eq!(marks.mark(later), Some(fx("100.01")));
        // In the halt from 15:00 to 15:05 the quotes are fresh and the market has no mark.
        marks.advance(HALT + MINUTE);
        assert_eq!(marks.mark(HALT + MINUTE), None);
        marks.advance(HALT + 5 * MINUTE);
        marks.store(HALT + 5 * MINUTE, &quotes(HALT + 5 * MINUTE));
        assert_eq!(marks.mark(HALT + 5 * MINUTE), Some(fx("100.01")));
    }

    /// A halt between two commands drops the quotes from before it, though they are fresh under the age
    /// limit: the quote of 14:59 does not price 15:06, but the one of 15:06 does.
    #[test]
    fn a_closure_between_commands_drops_older_quotes() {
        let mut marks = scheduled();
        marks.advance(OPEN);
        marks.store(OPEN, &[]);
        let before = HALT - MINUTE;
        marks.advance(before);
        marks.store(before, &[quote("a", "100", before), quote("b", "100.02", before)]);
        let after = HALT + 6 * MINUTE;
        marks.advance(after);
        assert_eq!(marks.mark(after), None, "the quotes of 14:59 are 7 minutes old, fresh, but from before the halt");
        marks.store(after, &[quote("a", "101", after), quote("b", "101.02", after)]);
        assert_eq!(marks.mark(after), Some(fx("101.01")));
    }

    /// A quiet stretch inside one session drops nothing, and a gap longer than the age limit across the
    /// close leaves nothing from before it.
    #[test]
    fn open_sessions_and_long_gaps() {
        let mut marks = scheduled();
        marks.advance(OPEN);
        marks.store(OPEN, &[]);
        marks.advance(OPEN + 2 * MINUTE);
        marks.store(OPEN + 2 * MINUTE, &[quote("a", "100", OPEN + 2 * MINUTE)]);
        marks.advance(OPEN + 9 * MINUTE);
        assert_eq!(marks.quotes.len(), 1);
        // A gap past the age limit that ends in a shut market drops what is held; one that ends in an
        // open session leaves it, stale, to be replaced.
        let evening = OPEN + 8 * 3_600_000;
        marks.advance(evening);
        assert!(marks.quotes.is_empty());
        let mut other = scheduled();
        other.advance(OPEN);
        other.store(OPEN + 2 * MINUTE, &[quote("a", "100", OPEN + 2 * MINUTE)]);
        other.advance(OPEN + 24 * 3_600_000);
        assert_eq!(other.quotes.len(), 1);
        assert_eq!(other.mark(OPEN + 24 * 3_600_000), None);
    }

    /// New hours from a relisting drop the quotes they would not have let in.
    #[test]
    fn new_hours_drop_quotes_outside_them() {
        let mut marks = IndexMarks::new(HOURS);
        marks.advance(OPEN);
        marks.store(OPEN, &[quote("a", "100", OPEN)]);
        marks.advance(OPEN + MINUTE);
        marks.set_schedule(OPEN + MINUTE, exchange());
        assert_eq!(marks.quotes.len(), 1);
        marks.advance(HALT);
        marks.set_schedule(HALT + MINUTE, exchange());
        assert!(marks.quotes.is_empty(), "stale under the age limit");
    }
}
