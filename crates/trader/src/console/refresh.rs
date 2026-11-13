//! Reads each account from the node for the console, every `refresh_interval`: its figures, its
//! MONKEY entitlement, and its fills since the last read, each a verified answer, and adds its
//! equity to its curve. The first read of
//! an account goes through its whole fill history, which realised profit and loss is summed over;
//! later reads take only the fills after the newest one seen. A read that fails keeps the last
//! figures and says why; an answer signed by another key ends the refresher, and with it the bot,
//! as one the bot reads for itself does. The reads take at most half of the node's per-address
//! allowance, so the accounts' orders always have the rest: with many accounts, a round of reads
//! takes longer than `refresh_interval`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use engine_core::Fixed;
use engine_core::types::{AccountId, MarketId};
use tokio::sync::watch;
use tokio::time::{Instant, sleep_until};
use tracing::{Instrument, info, info_span, warn};

use crate::config::RateSection;
use crate::console::board::{CurvePoint, Figures, FillView, Listing, NodeView, record};
use crate::node::{CallError, Node};

/// The most fills the console keeps for an account.
pub const FILLS_KEPT: usize = 50;

/// The node, and the accounts it reads.
pub struct Refresher {
    node: Arc<Node>,
    interval: Duration,
    accounts: Vec<Tracked>,
    markets: BTreeMap<MarketId, Listing>,
    pace: Pace,
}

/// Spaces the refresher's reads `gap` apart.
struct Pace {
    gap: Duration,
    next: Instant,
}

/// An account the refresher reads, where it publishes what it read, its fills so far, and its
/// equity curve.
struct Tracked {
    address: AccountId,
    view: watch::Sender<NodeView>,
    ledger: Ledger,
    curve: Vec<CurvePoint>,
    /// Whether the last read failed: an alert fires once per run of failures.
    failing: bool,
}

/// An account's fills as far as the newest the refresher has read.
#[derive(Default)]
struct Ledger {
    /// The newest fill's cursor; `None` before the first complete read.
    newest: Option<String>,
    realised: Fixed,
    fills: Vec<FillView>,
}

impl Refresher {
    /// A refresher reading `accounts` from `node` every `interval`, at most at half `per_ip`'s rate,
    /// and the views it publishes.
    pub fn new(
        node: Arc<Node>,
        interval: Duration,
        per_ip: RateSection,
        accounts: &[AccountId],
    ) -> (Self, Vec<watch::Receiver<NodeView>>) {
        let (accounts, views) = accounts
            .iter()
            .map(|&address| {
                let (view, receiver) = watch::channel(NodeView::default());
                let ledger = Ledger::default();
                (Tracked { address, view, ledger, curve: Vec::new(), failing: false }, receiver)
            })
            .unzip();
        (Self { node, interval, accounts, markets: BTreeMap::new(), pace: Pace::new(per_ip) }, views)
    }

    /// Reads every account, then waits `interval`, for as long as the console runs.
    /// Error cases:
    /// - an answer signed by another key than the signer
    pub async fn run(mut self) -> anyhow::Result<()> {
        loop {
            self.refresh().await?;
            tokio::time::sleep(self.interval).await;
        }
    }

    /// Reads the markets' symbols and marks, then each account once. Without an answer the symbols
    /// stay as last read, and no mark is shown.
    /// Error cases:
    /// - an answer signed by another key than the signer
    pub async fn refresh(&mut self) -> Result<(), CallError> {
        self.pace.wait().await;
        match self.node.markets().await {
            Ok(markets) => {
                self.markets = markets
                    .body
                    .into_iter()
                    .map(|m| {
                        let listing = Listing {
                            symbol: m.spec.symbol,
                            quote: m.spec.quote_currency,
                            mark: m.mark,
                            fx_mark: m.fx_mark,
                        };
                        (m.spec.id, listing)
                    })
                    .collect();
            }
            Err(e @ CallError::WrongSigner { .. }) => return Err(e),
            Err(e) => {
                warn!("the markets were not read; their symbols are as last read, with no marks: {e}");
                self.markets.values_mut().for_each(|listing| (listing.mark, listing.fx_mark) = (None, None));
            }
        }
        for account in &mut self.accounts {
            let span = info_span!("refresh", account = %account.address);
            account.refresh(&self.node, &self.markets, &mut self.pace).instrument(span).await?;
        }
        Ok(())
    }
}

impl Tracked {
    /// Reads the account and publishes what it read, or why it could not.
    /// Error cases:
    /// - an answer signed by another key than the signer: the account's view is cleared
    async fn refresh(
        &mut self,
        node: &Node,
        markets: &BTreeMap<MarketId, Listing>,
        pace: &mut Pace,
    ) -> Result<(), CallError> {
        let error = match self.read(node, markets, pace).await {
            Ok(figures) => {
                if self.failing {
                    info!(alert_resolved = "console_read_failed", "the account reads again");
                }
                self.failing = false;
                record(&mut self.curve, CurvePoint { time_ms: figures.time_ms, equity: figures.equity });
                let fills = Arc::new(self.ledger.fills.clone());
                let curve = Arc::new(self.curve.clone());
                self.view.send_if_modified(|view| {
                    let next = NodeView { figures: Some(Arc::new(figures)), fills, curve, error: None };
                    let changed = *view != next;
                    *view = next;
                    changed
                });
                return Ok(());
            }
            Err(e @ CallError::WrongSigner { .. }) => {
                self.view.send_replace(NodeView { error: Some(e.to_string()), ..NodeView::default() });
                return Err(e);
            }
            Err(e) => {
                if !self.failing {
                    warn!(
                        alert = "console_read_failed",
                        "the account was not read; the console shows the last figures: {e}"
                    );
                }
                e.to_string()
            }
        };
        self.failing = true;
        self.view.send_modify(|view| view.error = Some(error));
        Ok(())
    }

    /// The account's figures, entitlement and new fills, each verified.
    /// Error cases:
    /// - see `CallError`, for any of the three reads
    async fn read(
        &mut self,
        node: &Node,
        markets: &BTreeMap<MarketId, Listing>,
        pace: &mut Pace,
    ) -> Result<Figures, CallError> {
        pace.wait().await;
        let answer = node.account_figures(self.address).await?;
        pace.wait().await;
        let entitlement = node.entitlement(self.address).await?;
        self.ledger.catch_up(node, self.address, markets, pace).await?;
        let signed = (answer.time_ms, answer.head_seq);
        Ok(Figures::new(answer.body, signed, entitlement, self.ledger.realised, markets))
    }
}

impl Ledger {
    /// Reads the fills after the newest one read, page by page, newest first, and takes them once
    /// every page has been read, so a failed read takes none and the next one starts from the same
    /// place.
    /// Error cases:
    /// - see `CallError`
    /// - a page whose `next` cursor is malformed or not older than the page before
    async fn catch_up(
        &mut self,
        node: &Node,
        account: AccountId,
        markets: &BTreeMap<MarketId, Listing>,
        pace: &mut Pace,
    ) -> Result<(), CallError> {
        let (mut realised, mut newest, mut fresh) = (Fixed::ZERO, None, Vec::new());
        let mut before: Option<String> = None;
        loop {
            pace.wait().await;
            let page = node.fills(account, self.newest.as_deref(), before.as_deref()).await?.body;
            for fill in page.events {
                realised = realised + fill.fill.realised_pnl - fill.fill.profit_fee;
                newest.get_or_insert_with(|| fill.cursor.clone());
                if fresh.len() < FILLS_KEPT {
                    let symbol = markets.get(&fill.fill.market).map(|listing| listing.symbol.clone());
                    fresh.push(FillView { fill, symbol });
                }
            }
            let Some(next) = page.next else { break };
            let older = cursor(&next).is_some_and(|next| before.as_deref().and_then(cursor).is_none_or(|b| next < b));
            if !older {
                let source = anyhow::anyhow!("the history's next page {next:?} is not before {before:?}");
                return Err(CallError::Failed { target: format!("GET /v1/accounts/{account}/history"), source });
            }
            before = Some(next);
        }
        self.realised += realised;
        if newest.is_some() {
            self.newest = newest;
            fresh.append(&mut self.fills);
            fresh.truncate(FILLS_KEPT);
            self.fills = fresh;
        }
        Ok(())
    }
}

impl Pace {
    /// Reads at half `per_ip`'s rate, the first at once.
    fn new(per_ip: RateSection) -> Self {
        Self { gap: 2 * Duration::from_secs(1) / per_ip.per_second.max(1), next: Instant::now() }
    }

    /// Waits until the next read may go.
    async fn wait(&mut self) {
        sleep_until(self.next).await;
        self.next = Instant::now() + self.gap;
    }
}

/// A history cursor, `<seq>:<index>`, as the pair it orders by.
fn cursor(text: &str) -> Option<(u64, u32)> {
    let (seq, index) = text.split_once(':')?;
    Some((seq.parse().ok()?, index.parse().ok()?))
}

#[cfg(test)]
mod tests;
