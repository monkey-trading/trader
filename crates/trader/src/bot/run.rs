//! The runner: the fleet's decisions turned into requests. It reads the log in one task, keeps every
//! account's state there, and starts each request as a future whose answer comes back to the same
//! task, so a fill read from the log can never overtake the record of the order it comes from.
//! Orders leave in the order their bars completed, one request out per account at a time, and the
//! node paces them (`node::pace`) under its limits instead of failing them.

use std::collections::VecDeque;
use std::sync::Arc;

use engine_core::eip712::Eip712Domain;
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use futures_util::stream::FuturesUnordered;
use tokio::time::{Instant, sleep_until};
use tracing::{info, warn};

use crate::bot::Halt;
use crate::bot::day::Saved;
use crate::bot::fleet::Fleet;
use crate::node::api::{AccountView, MarkBucket, NonceView};
use crate::node::{CallError, Node, Verified};
use crate::stream::{Stream, StreamError};

/// How long after a failed read of the node's time it is tried again.
const CLOCK_RETRY: std::time::Duration = std::time::Duration::from_secs(1);

/// What a started request answered.
pub(super) enum Done {
    Order {
        account: u32,
        assignment: u32,
        nonce: u64,
        outcome: Result<Verified<Vec<engine_core::event::Event>>, CallError>,
    },
    Nonce {
        account: u32,
        outcome: Result<Verified<NonceView>, CallError>,
    },
    View {
        account: u32,
        outcome: Result<Verified<AccountView>, CallError>,
    },
    Saved {
        account: u32,
        saved: Saved,
        outcome: anyhow::Result<()>,
    },
    History {
        series: u32,
        generation: u32,
        outcome: Result<Vec<MarkBucket>, CallError>,
    },
    Clock(Result<Verified<NonceView>, CallError>),
}

/// The fleet and the requests started for it.
pub struct Runner {
    pub(super) fleet: Fleet,
    pub(super) node: Arc<Node>,
    pub(super) domain: Eip712Domain,
    pub(super) in_flight: FuturesUnordered<BoxFuture<'static, Done>>,
    /// The most order and nonce requests out at once.
    pub(super) cap: usize,
    pub(super) out: usize,
    /// Reads of an account out, and the order they were started in with the answers that are in.
    pub(super) view_reads: usize,
    pub(super) view_order: VecDeque<(u32, Option<ViewOutcome>)>,
    pub(super) clock_reading: bool,
    /// The earliest a failed read of the node's time is tried again.
    pub(super) clock_retry_at: Option<Instant>,
    /// Whether the node's limits stopped an order this round; the orders behind it wait too, so they
    /// leave in bar order.
    pub(super) admit_blocked: bool,
    /// Decisions dropped for age since the last warning.
    pub(super) stale_dropped: u32,
    /// When the log last gave a decision to trade; the node's time an order is signed on must come
    /// from a request sent after it.
    pub(super) decided_at: Instant,
    pub(super) seen_decisions: u64,
    /// The earliest the runner has something to do again without a message or answer.
    pub(super) wake_at: Option<Instant>,
}

/// A read of an account, once answered.
pub(super) type ViewOutcome = Result<Verified<AccountView>, CallError>;

impl Runner {
    /// A runner for `fleet` on `node`, with at most `cap` order requests out at once.
    pub fn new(fleet: Fleet, node: Arc<Node>, domain: Eip712Domain, cap: usize) -> Self {
        Self {
            fleet,
            node,
            domain,
            in_flight: FuturesUnordered::new(),
            cap,
            out: 0,
            view_reads: 0,
            view_order: VecDeque::new(),
            clock_reading: false,
            clock_retry_at: None,
            admit_blocked: false,
            stale_dropped: 0,
            decided_at: Instant::now(),
            seen_decisions: 0,
            wake_at: None,
        }
    }

    /// Applies the log's messages and the answers to the requests they start, until `stop`
    /// completes or the bot halts.
    /// Error cases:
    /// - see `Halt`
    pub async fn follow(&mut self, stream: &mut Stream, stop: impl Future<Output = ()>) -> Result<(), Halt> {
        tokio::pin!(stop);
        loop {
            if self.fleet.failed == self.fleet.accounts.len() {
                let reasons: Vec<_> = self.fleet.accounts.iter().filter_map(|a| a.failed.as_deref()).collect();
                return Err(Halt::AllStopped(reasons.join("; ")));
            }
            self.pump();
            let wake = self.wake_at;
            tokio::select! {
                biased;
                () = &mut stop => {
                    info!("stop signal");
                    return Ok(());
                }
                Some(done) = self.in_flight.next(), if !self.in_flight.is_empty() => self.handle(done)?,
                frame = stream.next() => {
                    let frame = frame.map_err(|e| match e {
                        StreamError::WrongSigner => Halt::WrongSigner,
                        StreamError::Unreadable(e) => Halt::Unfollowable(e),
                    })?;
                    self.fleet.apply(&frame.msg, frame.offset, frame.time)?;
                }
                () = async {
                    match wake {
                        Some(at) => sleep_until(at).await,
                        None => std::future::pending().await,
                    }
                } => self.wake_at = None,
            }
        }
    }

    /// Starts every request the fleet's state calls for that the limits on requests out allow.
    fn pump(&mut self) {
        if self.fleet.decisions != self.seen_decisions {
            (self.seen_decisions, self.decided_at) = (self.fleet.decisions, Instant::now());
        }
        self.start_warm_ups();
        self.start_orders();
        self.start_reads();
        self.start_saves();
        self.start_flattens();
    }

    /// Applies one answer.
    /// Error cases:
    /// - an answer signed by another key than the signer
    pub(super) fn handle(&mut self, done: Done) -> Result<(), Halt> {
        match done {
            Done::Order { account, assignment, nonce, outcome } => self.order_done(account, assignment, nonce, outcome),
            Done::Nonce { account, outcome } => self.nonce_done(account, outcome),
            Done::View { account, outcome } => self.view_done(account, outcome),
            Done::Saved { account, saved, outcome } => self.saved_done(account, saved, outcome),
            Done::History { series, generation, outcome } => self.history_done(series, generation, outcome),
            Done::Clock(outcome) => {
                self.clock_reading = false;
                match outcome {
                    Err(CallError::WrongSigner { .. }) => Err(Halt::WrongSigner),
                    Err(e) => {
                        warn!("the node's time was not read: {e}");
                        self.clock_retry_at = Some(Instant::now() + CLOCK_RETRY);
                        Ok(())
                    }
                    Ok(_) => Ok(()),
                }
            }
        }
    }
}

/// The wrong-signer check every answer passes: such an answer ends the run.
/// Error cases:
/// - the call's signature is not the signer's
pub(super) fn signer_ok<T>(outcome: &Result<T, CallError>) -> Result<(), Halt> {
    match outcome {
        Err(CallError::WrongSigner { .. }) => Err(Halt::WrongSigner),
        _ => Ok(()),
    }
}
