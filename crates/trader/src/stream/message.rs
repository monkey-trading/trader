//! The public log's stream messages, read as borrowed views of the frame's text: the envelope and a
//! command's type first, and the rest of a command only when the bot needs it.

use std::fmt;
use std::str::FromStr;

use anyhow::Context;
use engine_core::Fixed;
use engine_core::command::Command;
use engine_core::event::Event;
use engine_core::types::{MarketId, Millis};
use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;
use trader_core::marks::IndexMarks;

/// Where a message sits in the log: its seq, and its index among that seq's entries, the command
/// being 0. Written `<seq>:<index>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Offset {
    pub seq: u64,
    pub index: u32,
}

/// What every stream message starts with: its type and, but for a heartbeat, its offset.
#[derive(Debug, Deserialize)]
pub struct Head<'a> {
    #[serde(rename = "type")]
    pub kind: &'a str,
    pub offset: Option<&'a str>,
}

/// A stream message read in one pass: its type and, for a command, the rest of what `CommandMessage`
/// holds. A message of another type leaves the command's fields out.
#[derive(Debug, Deserialize)]
pub struct Envelope<'a> {
    #[serde(rename = "type")]
    pub kind: &'a str,
    pub seq: Option<u64>,
    pub ts: Option<Millis>,
    #[serde(borrow)]
    pub cmd: Option<&'a RawValue>,
    #[serde(borrow)]
    pub events: Option<&'a RawValue>,
}

impl<'a> Envelope<'a> {
    /// The command message this envelope holds.
    /// Error cases:
    /// - a command with no seq, time or command
    pub fn command(&self) -> anyhow::Result<CommandMessage<'a>> {
        match (self.seq, self.ts, self.cmd) {
            (Some(seq), Some(ts), Some(cmd)) => Ok(CommandMessage { seq, ts, cmd, events: self.events }),
            _ => anyhow::bail!("a command message with no seq, time or command"),
        }
    }
}

/// A command message: its seq, sequenced time, command, and the events applying it produced.
#[derive(Debug, Deserialize)]
pub struct CommandMessage<'a> {
    pub seq: u64,
    pub ts: Millis,
    #[serde(borrow)]
    pub cmd: &'a RawValue,
    #[serde(borrow)]
    pub events: Option<&'a RawValue>,
}

/// The type of a command, and its market for a command that names one.
#[derive(Debug, Deserialize)]
pub struct CommandHead<'a> {
    #[serde(rename = "type")]
    pub kind: &'a str,
    pub market: Option<MarketId>,
}

/// What every event starts with that the bot looks at: its type and, for most, its account.
#[derive(Debug, Deserialize)]
pub struct EventHead<'a> {
    #[serde(rename = "type")]
    pub kind: &'a str,
    pub account: Option<&'a str>,
}

/// A fill event, borrowed: the fields the bot follows.
#[derive(Debug, Deserialize)]
pub struct FillRef<'a> {
    pub account: &'a str,
    pub order_id: u64,
    pub market: MarketId,
    pub side: &'a str,
    pub qty: &'a str,
    pub price: &'a str,
    pub realised_pnl: &'a str,
    pub profit_fee: &'a str,
    pub position: &'a str,
    pub liquidation: bool,
}

/// An order command, borrowed: whose it is, and the signature it carries.
#[derive(Debug, Deserialize)]
pub struct OrderRef<'a> {
    pub account: &'a str,
    pub auth: Option<AuthRef<'a>>,
}

/// A request's signature, as text.
#[derive(Debug, Deserialize)]
pub struct AuthRef<'a> {
    pub signature: &'a str,
}

/// One quote of a `Prices` command, borrowed.
#[derive(Debug, Deserialize)]
struct QuoteRef<'a> {
    source: &'a str,
    mid: &'a str,
    ts: Millis,
    contract: Option<&'a str>,
}

/// Reads the quotes of a `Prices` command into `marks` one by one, with no allocation but for a
/// venue the book has not seen.
/// Error cases:
/// - a command that is not an object with a quotes array of venue, mid and time
pub fn store_prices(cmd: &str, now: Millis, marks: &mut IndexMarks) -> anyhow::Result<()> {
    struct PricesCmd<'m>(&'m mut IndexMarks, Millis);
    struct Quotes<'m>(&'m mut IndexMarks, Millis);
    impl<'de> Visitor<'de> for PricesCmd<'_> {
        type Value = ();
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a prices command")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
            while let Some(key) = map.next_key::<&str>()? {
                if key == "quotes" {
                    map.next_value_seed(Quotes(self.0, self.1))?;
                } else {
                    map.next_value::<IgnoredAny>()?;
                }
            }
            Ok(())
        }
    }
    impl<'de> DeserializeSeed<'de> for Quotes<'_> {
        type Value = ();
        fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
            d.deserialize_seq(self)
        }
    }
    impl<'de> Visitor<'de> for Quotes<'_> {
        type Value = ();
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("quotes")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
            while let Some(quote) = seq.next_element::<QuoteRef<'de>>()? {
                let mid: Fixed = quote.mid.parse().map_err(serde::de::Error::custom)?;
                self.0.store_quote(self.1, quote.source, mid, quote.ts, quote.contract.is_some());
            }
            Ok(())
        }
    }
    marks.begin(now);
    let mut de = serde_json::Deserializer::from_str(cmd);
    Deserializer::deserialize_map(&mut de, PricesCmd(marks, now)).context("a prices command")
}

/// Calls `f` with each event of `events` in turn, as borrowed text, with no allocation.
/// Error cases:
/// - `events` is not an array, or `f` fails
pub fn for_each_event<'a>(
    events: &'a RawValue,
    mut f: impl FnMut(&'a RawValue) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    struct Each<'f, F>(&'f mut F);
    impl<'de, F: FnMut(&'de RawValue) -> anyhow::Result<()>> Visitor<'de> for Each<'_, F> {
        type Value = ();
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("an array of events")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
            while let Some(event) = seq.next_element::<&'de RawValue>()? {
                (self.0)(event).map_err(|e| serde::de::Error::custom(format!("{e:#}")))?;
            }
            Ok(())
        }
    }
    let mut de = serde_json::Deserializer::from_str(events.get());
    Deserializer::deserialize_seq(&mut de, Each(&mut f)).context("events")
}

impl Offset {
    /// Whether `next` is the next message a gap-free stream may carry after this one: the same seq's
    /// next entry, or the next seq's command. The node numbers a seq's entries one after another.
    pub fn is_followed_by(self, next: Offset) -> bool {
        (next.seq == self.seq && Some(next.index) == self.index.checked_add(1))
            || (Some(next.seq) == self.seq.checked_add(1) && next.index == 0)
    }
}

impl fmt::Display for Offset {
    /// `<seq>:<index>`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.seq, self.index)
    }
}

impl FromStr for Offset {
    type Err = anyhow::Error;

    /// Parses `<seq>:<index>`.
    /// Error cases:
    /// - no `:`, or either side is not an unsigned integer in range
    fn from_str(text: &str) -> anyhow::Result<Self> {
        let (seq, index) = text.split_once(':').with_context(|| format!("offset {text:?} is not <seq>:<index>"))?;
        Ok(Self {
            seq: seq.parse().with_context(|| format!("offset {text:?}: seq"))?,
            index: index.parse().with_context(|| format!("offset {text:?}: index"))?,
        })
    }
}

impl<'a> CommandMessage<'a> {
    /// The command's type and market.
    /// Error cases:
    /// - the command is not an object with a string `type`
    pub fn head(&self) -> anyhow::Result<CommandHead<'a>> {
        serde_json::from_str(self.cmd.get()).context("command head")
    }

    /// The whole command, as the engine reads it.
    /// Error cases:
    /// - the command is not one the pinned engine knows
    pub fn command(&self) -> anyhow::Result<Command> {
        serde_json::from_str(self.cmd.get()).context("command")
    }

    /// The command's events, or none when it produced none.
    /// Error cases:
    /// - an event the pinned engine does not know
    pub fn events(&self) -> anyhow::Result<Vec<Event>> {
        match self.events {
            None => Ok(Vec::new()),
            Some(raw) if raw.get() == "[]" => Ok(Vec::new()),
            Some(raw) => serde_json::from_str(raw.get()).context("events"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_parse_and_print_as_seq_and_index() {
        let offset: Offset = "12:3".parse().unwrap();
        assert_eq!(offset, Offset { seq: 12, index: 3 });
        assert_eq!(offset.to_string(), "12:3");
        for bad in ["12", "12:", ":3", "a:1", "1:-1", "1:4294967296"] {
            assert!(bad.parse::<Offset>().is_err(), "{bad}");
        }
    }

    /// After 5:0, the stream may carry 5:1 (a withdrawal or a checkpoint) or 6:0, and nothing else:
    /// not 5:0 again, 5:2, 6:1, or 7:0.
    #[test]
    fn a_gap_free_stream_moves_to_the_next_entry_or_the_next_command() {
        let at = |seq, index| Offset { seq, index };
        assert!(at(5, 0).is_followed_by(at(5, 1)));
        assert!(at(5, 1).is_followed_by(at(5, 2)));
        assert!(at(5, 2).is_followed_by(at(6, 0)));
        assert!(at(5, 0).is_followed_by(at(6, 0)));
        assert!(!at(5, 0).is_followed_by(at(5, 2)));
        assert!(!at(5, 0).is_followed_by(at(5, 0)));
        assert!(!at(5, 1).is_followed_by(at(6, 1)));
        assert!(!at(5, 0).is_followed_by(at(7, 0)));
        assert!(!at(5, 1).is_followed_by(at(4, 9)));
        assert!(!at(u64::MAX, 0).is_followed_by(at(0, 0)));
    }

    /// A command message as the node streams it: the envelope borrows its parts, and a command with
    /// no events gives none.
    #[test]
    fn reads_a_prices_command() {
        let text = r#"{"offset":"7:0","type":"command","seq":7,"ts":1000,"cmd":{"type":"prices","market":1,
            "quotes":[{"source":"mock-a","mid":"100000.5","ts":999}]},"events":[]}"#;
        let head: Head<'_> = serde_json::from_str(text).unwrap();
        assert_eq!((head.kind, head.offset), ("command", Some("7:0")));
        let message: CommandMessage<'_> = serde_json::from_str(text).unwrap();
        assert_eq!((message.seq, message.ts), (7, 1000));
        let cmd = message.head().unwrap();
        assert_eq!((cmd.kind, cmd.market), ("prices", Some(1)));
        let mut marks = IndexMarks::new(engine_core::market::PriceRules {
            max_age_ms: 1_000,
            max_divergence_bps: 25,
            min_sources: 1,
        });
        store_prices(message.cmd.get(), 1000, &mut marks).unwrap();
        assert_eq!(marks.mark(2_001), None, "the book has not seen max_age_ms of the log yet");
        assert_eq!(marks.mark(2_002), None, "the quote is 1002 ms old");
        let mut marks2 = marks.clone();
        marks2.begin(2_000);
        assert!(marks.is_complete(2_001));
        assert!(message.events().unwrap().is_empty());
    }
}
