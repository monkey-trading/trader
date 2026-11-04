//! Reads each account from the node for the console, every `refresh_interval`: its figures, its
//! MONKEY entitlement, and its fills since the last read, each a verified answer. The first read of
//! an account goes through its whole fill history, which realised profit and loss is summed over;
//! later reads take only the fills after the newest one seen. A read that fails keeps the last
//! figures and says why; an answer signed by another key clears them.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use engine_core::Fixed;
use engine_core::types::{AccountId, MarketId};
use tokio::sync::watch;
use tracing::{Instrument, error, info, info_span, warn};

use crate::console::board::{Figures, FillView, NodeView};
use crate::node::{CallError, Node};

/// The most fills the console keeps for an account.
pub const FILLS_KEPT: usize = 50;

/// The node, and the accounts it reads.
pub struct Refresher {
    node: Node,
    interval: Duration,
    accounts: Vec<Tracked>,
    symbols: BTreeMap<MarketId, String>,
}

/// An account the refresher reads, where it publishes what it read, and its fills so far.
struct Tracked {
    address: AccountId,
    view: watch::Sender<NodeView>,
    ledger: Ledger,
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
    /// A refresher reading `accounts` from `node` every `interval`, and the views it publishes.
    pub fn new(node: Node, interval: Duration, accounts: &[AccountId]) -> (Self, Vec<watch::Receiver<NodeView>>) {
        let (accounts, views) = accounts
            .iter()
            .map(|&address| {
                let (view, receiver) = watch::channel(NodeView::default());
                (Tracked { address, view, ledger: Ledger::default(), failing: false }, receiver)
            })
            .unzip();
        (Self { node, interval, accounts, symbols: BTreeMap::new() }, views)
    }

    /// Reads every account, then waits `interval`, for as long as the console runs.
    pub async fn run(mut self) {
        loop {
            self.refresh().await;
            tokio::time::sleep(self.interval).await;
        }
    }

    /// Reads the markets' symbols, then each account once.
    pub async fn refresh(&mut self) {
        match self.node.markets().await {
            Ok(markets) => self.symbols = markets.body.into_iter().map(|m| (m.spec.id, m.spec.symbol)).collect(),
            Err(e) => warn!("the markets were not read; their symbols are as last read: {e}"),
        }
        for account in &mut self.accounts {
            let span = info_span!("refresh", account = %account.address);
            account.refresh(&self.node, &self.symbols).instrument(span).await;
        }
    }
}

impl Tracked {
    /// Reads the account and publishes what it read, or why it could not.
    async fn refresh(&mut self, node: &Node, symbols: &BTreeMap<MarketId, String>) {
        let error = match self.read(node, symbols).await {
            Ok(figures) => {
                if self.failing {
                    info!(alert_resolved = "console_read_failed", "the account reads again");
                }
                self.failing = false;
                let fills = Arc::new(self.ledger.fills.clone());
                self.view.send_if_modified(|view| {
                    let next = NodeView { figures: Some(Arc::new(figures)), fills, error: None };
                    let changed = *view != next;
                    *view = next;
                    changed
                });
                return;
            }
            Err(e @ CallError::WrongSigner { .. }) => {
                if !self.failing {
                    error!(alert = "node_signer_mismatch", "{e}; the console shows nothing of the account");
                }
                self.ledger = Ledger::default();
                self.view.send_replace(NodeView::default());
                e.to_string()
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
    }

    /// The account's figures, entitlement and new fills, each verified.
    /// Error cases:
    /// - see `CallError`, for any of the three reads
    async fn read(&mut self, node: &Node, symbols: &BTreeMap<MarketId, String>) -> Result<Figures, CallError> {
        let answer = node.account_figures(self.address).await?;
        let entitlement = node.entitlement(self.address).await?;
        self.ledger.catch_up(node, self.address, symbols).await?;
        let signed = (answer.time_ms, answer.head_seq);
        Ok(Figures::new(answer.body, signed, entitlement, self.ledger.realised, symbols))
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
        symbols: &BTreeMap<MarketId, String>,
    ) -> Result<(), CallError> {
        let (mut realised, mut newest, mut fresh) = (Fixed::ZERO, None, Vec::new());
        let mut before: Option<String> = None;
        loop {
            let page = node.fills(account, self.newest.as_deref(), before.as_deref()).await?.body;
            for fill in page.events {
                realised = realised + fill.fill.realised_pnl - fill.fill.profit_fee;
                newest.get_or_insert_with(|| fill.cursor.clone());
                if fresh.len() < FILLS_KEPT {
                    let symbol = symbols.get(&fill.fill.market).cloned();
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

/// A history cursor, `<seq>:<index>`, as the pair it orders by.
fn cursor(text: &str) -> Option<(u64, u32)> {
    let (seq, index) = text.split_once(':')?;
    Some((seq.parse().ok()?, index.parse().ok()?))
}

#[cfg(test)]
mod tests;
