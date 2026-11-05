//! What the console's handlers share: each account's link and the refresher's view of it, the
//! sessions, the login's hash and rate limit, and the cap on open live feeds. The sessions and the
//! rate limit sit behind locks held only for a lookup, never across an await.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use engine_core::types::AccountId;
use serde_json::{Value, json};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, SemaphorePermit, mpsc, watch};
use tokio::time::Instant;
use zeroize::Zeroizing;

use crate::bot::link::{AccountLink, AccountRequest, AccountStatus};
use crate::console::board::{AccountView, FillView, NodeView, Totals, totals};
use crate::console::login::Limiter;
use crate::console::session::Sessions;

/// The most live feeds open at once.
const FEEDS: usize = 8;

/// The console's shared state.
pub struct Server {
    accounts: BTreeMap<AccountId, Account>,
    sessions: Mutex<Sessions>,
    password_hash: String,
    limiter: Mutex<Limiter>,
    /// One password check at a time: each takes a deliberate share of a core.
    checking: Semaphore,
    feeds: Arc<Semaphore>,
    next_id: AtomicU64,
}

/// One account: its link to its task, and the refresher's view of it.
struct Account {
    link: AccountLink,
    node: watch::Receiver<NodeView>,
}

/// One account as a live feed last sent it.
pub struct Watched {
    address: AccountId,
    status: watch::Receiver<AccountStatus>,
    node: watch::Receiver<NodeView>,
    fills: Arc<Vec<FillView>>,
}

impl Server {
    /// The state for `accounts`, each with the refresher's view of it, in the same order.
    pub fn new(accounts: Vec<AccountLink>, views: Vec<watch::Receiver<NodeView>>, password_hash: String) -> Self {
        assert_eq!(accounts.len(), views.len(), "the refresher reads every account");
        let accounts =
            accounts.into_iter().zip(views).map(|(link, node)| (link.address, Account { link, node })).collect();
        Self {
            accounts,
            sessions: Mutex::default(),
            password_hash,
            limiter: Mutex::default(),
            checking: Semaphore::new(1),
            feeds: Arc::new(Semaphore::new(FEEDS)),
            next_id: AtomicU64::new(1),
        }
    }

    /// A new id for a request or a feed.
    pub fn next_request_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Whether `token` is a live session's.
    pub fn session_live(&self, token: &str, now: Instant) -> bool {
        lock(&self.sessions).is_live(token, now)
    }

    /// A new session's token.
    pub fn new_session(&self, now: Instant) -> Zeroizing<String> {
        lock(&self.sessions).create(now)
    }

    /// Ends `token`'s session.
    pub fn end_session(&self, token: &str) {
        lock(&self.sessions).end(token);
    }

    /// Takes a login attempt for `client`.
    /// Error cases:
    /// - none is left: the time until one is
    pub fn take_login_attempt(&self, client: IpAddr, now: Instant) -> Result<(), Duration> {
        lock(&self.limiter).take(client, now)
    }

    /// The turn to check a password, once the one before is done.
    pub async fn password_check(&self) -> SemaphorePermit<'_> {
        self.checking.acquire().await.expect("the password check's semaphore is never closed")
    }

    /// The login's Argon2id hash.
    pub fn password_hash(&self) -> &str {
        &self.password_hash
    }

    /// A slot for a live feed, while one is free.
    pub fn open_feed(&self) -> Option<OwnedSemaphorePermit> {
        self.feeds.clone().try_acquire_owned().ok()
    }

    /// Where `address`'s requests go, if it is one of the bot's accounts.
    pub fn requests(&self, address: &str) -> Option<mpsc::Sender<AccountRequest>> {
        self.find(address).map(|account| account.link.requests.clone())
    }

    /// Every account's view, and the totals.
    pub fn overview(&self) -> (Vec<Value>, Totals) {
        self.overview_of(&mut self.watch(), false)
    }

    /// `address`'s view, with its whole equity curve, if it is one of the bot's accounts.
    pub fn account_view(&self, address: &str) -> Option<Value> {
        let account = self.find(address)?;
        Some(view(account.link.address, &account.link.status.borrow(), &account.node.borrow(), Curve::Whole))
    }

    /// `address`'s latest fills, newest first, if it is one of the bot's accounts.
    pub fn fills(&self, address: &str) -> Option<Arc<Vec<FillView>>> {
        Some(self.find(address)?.node.borrow().fills.clone())
    }

    /// Every account, to follow from a live feed.
    pub fn watch(&self) -> Vec<Watched> {
        self.accounts
            .values()
            .map(|account| Watched {
                address: account.link.address,
                status: account.link.status.clone(),
                node: account.node.clone(),
                fills: Arc::default(),
            })
            .collect()
    }

    /// The views of `watched`, each with only its curve's newest point, and their totals; with
    /// `seen`, each is marked as sent.
    pub fn overview_of(&self, watched: &mut [Watched], seen: bool) -> (Vec<Value>, Totals) {
        let mut views = Vec::with_capacity(watched.len());
        let mut nodes = Vec::with_capacity(watched.len());
        for account in watched.iter_mut() {
            let (status, node) = if seen {
                (account.status.borrow_and_update().clone(), account.node.borrow_and_update().clone())
            } else {
                (account.status.borrow().clone(), account.node.borrow().clone())
            };
            views.push(view(account.address, &status, &node, Curve::Newest));
            if seen {
                account.fills = node.fills.clone();
            }
            nodes.push(node);
        }
        (views, totals(nodes.iter().map(|node| node.figures.as_deref())))
    }

    /// The messages that bring a feed up to date with `watched`: each changed account, with only its
    /// curve's newest point, its fills when they changed, and then the totals; none when nothing
    /// changed.
    pub fn changes(&self, watched: &mut [Watched]) -> Vec<Value> {
        let mut messages = Vec::new();
        for account in watched.iter_mut() {
            let changed = account.status.has_changed().unwrap_or(false) | account.node.has_changed().unwrap_or(false);
            if !changed {
                continue;
            }
            let status = account.status.borrow_and_update().clone();
            let node = account.node.borrow_and_update().clone();
            let view = view(account.address, &status, &node, Curve::Newest);
            messages.push(json!({ "type": "account", "account": view }));
            if !Arc::ptr_eq(&account.fills, &node.fills) && *account.fills != *node.fills {
                messages.push(json!({ "type": "fills", "address": account.address, "fills": *node.fills }));
            }
            account.fills = node.fills;
        }
        if !messages.is_empty() {
            messages.push(json!({ "type": "totals", "totals": self.totals() }));
        }
        messages
    }

    /// The totals of every account's figures as they stand.
    fn totals(&self) -> Totals {
        let figures: Vec<_> = self.accounts.values().map(|account| account.node.borrow().figures.clone()).collect();
        totals(figures.iter().map(Option::as_deref))
    }

    /// The account `address` names, if it is one of the bot's; addresses compare in either case.
    fn find(&self, address: &str) -> Option<&Account> {
        self.accounts.get(&address.parse::<AccountId>().ok()?)
    }
}

/// How much of an account's equity curve its view carries. A day of points per account is too much
/// to send for every account at once, so the overview and the live feed send only the newest point,
/// which a client appends to the curve it read for the account it shows.
#[derive(Clone, Copy)]
enum Curve {
    Whole,
    Newest,
}

/// An account's view as JSON, with `curve` of its equity curve.
fn view(address: AccountId, status: &AccountStatus, node: &NodeView, curve: Curve) -> Value {
    let curve = match curve {
        Curve::Whole => &node.curve[..],
        Curve::Newest => &node.curve[node.curve.len().saturating_sub(1)..],
    };
    let view = AccountView { address, status, figures: node.figures.as_deref(), error: node.error.as_deref(), curve };
    serde_json::to_value(view).expect("an account's view serializes")
}

/// Locks `mutex`; a handler that panicked while holding it left nothing half-changed, since every
/// change under these locks is a single map operation.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
