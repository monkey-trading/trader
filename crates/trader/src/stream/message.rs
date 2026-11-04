//! The public log's stream messages, read as borrowed views of the frame's text: the envelope first,
//! and a command's parts only when the bot needs them, so a message it skips costs no allocation.

use std::fmt;
use std::str::FromStr;

use anyhow::Context;
use engine_core::command::Command;
use engine_core::event::Event;
use engine_core::price::SourceQuote;
use engine_core::types::{MarketId, Millis};
use serde::Deserialize;
use serde_json::value::RawValue;

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

/// A `Prices` command's quotes.
#[derive(Debug, Deserialize)]
pub struct Quotes {
    pub quotes: Vec<SourceQuote>,
}

impl Offset {
    /// Whether `next` is the next message a gap-free stream may carry after this one: a later entry
    /// of the same seq, or the next seq's command.
    pub fn is_followed_by(self, next: Offset) -> bool {
        (next.seq == self.seq && next.index > self.index)
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

    /// After 5:0, the stream may carry 5:1 or 5:2 (withdrawals, then a checkpoint) or 6:0, and
    /// nothing else: not 5:0 again, 6:1, or 7:0.
    #[test]
    fn a_gap_free_stream_moves_to_a_later_entry_or_the_next_command() {
        let at = |seq, index| Offset { seq, index };
        assert!(at(5, 0).is_followed_by(at(5, 1)));
        assert!(at(5, 0).is_followed_by(at(5, 2)));
        assert!(at(5, 2).is_followed_by(at(6, 0)));
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
        let quotes: Quotes = serde_json::from_str(message.cmd.get()).unwrap();
        assert_eq!(quotes.quotes[0].mid, "100000.5".parse().unwrap());
        assert!(message.events().unwrap().is_empty());
    }
}
