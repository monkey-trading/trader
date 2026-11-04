//! A market's index mark, derived from the public log's `Prices` commands as the engine derives it,
//! for a market priced from spot quotes that never closes: the venue quotes each command stores,
//! and the median of the fresh ones (`engine_core::price::compute_index`) at each command's time.

use std::collections::BTreeMap;

use engine_core::Fixed;
use engine_core::market::PriceRules;
use engine_core::price::{MAX_QUOTE_LEAD_MS, SourceQuote, compute_index};
use engine_core::types::Millis;

/// One market's stored venue quotes, as the engine holds them.
#[derive(Clone, Debug)]
pub struct IndexMarks {
    rules: PriceRules,
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
    /// An empty book for a market priced under `rules`.
    pub fn new(rules: PriceRules) -> Self {
        Self { rules, quotes: BTreeMap::new(), first: None, known: None }
    }

    /// Takes the rules a new listing of the market sets, from the command that applies it on.
    pub fn set_rules(&mut self, rules: PriceRules) {
        self.rules = rules;
        self.known = None;
    }

    /// Stores the quotes of a `Prices` command for this market sequenced at `now`, refusing those
    /// the engine refuses: malformed ones, those for a futures contract, and those stamped more than
    /// `MAX_QUOTE_LEAD_MS` ahead. A quote stamped ahead counts from `now`, and replaces its venue's
    /// only when it is no older.
    pub fn store(&mut self, now: Millis, quotes: &[SourceQuote]) {
        self.first.get_or_insert(now);
        self.known = None;
        for quote in quotes {
            let refused = !quote.is_well_formed() || quote.contract.is_some();
            if refused || quote.ts.saturating_sub(now) > MAX_QUOTE_LEAD_MS {
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

    /// The index mark at `now`, any command's sequenced time: `None` until the book has seen
    /// `max_age_ms` of the log, or while the fresh quotes give no index. Worked out again only once
    /// a quote is stored or a fresh one ages out, so most commands cost a comparison.
    pub fn mark(&mut self, now: Millis) -> Option<Fixed> {
        if let Some((mark, through)) = self.known
            && now <= through
        {
            return mark;
        }
        let max_age = self.rules.max_age_ms;
        let first = *self.first.get_or_insert(now);
        let known = if now.saturating_sub(first) <= max_age {
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
        marks.set_rules(PriceRules { min_sources: 3, ..RULES });
        assert_eq!(marks.mark(5_001), None);
    }
}
