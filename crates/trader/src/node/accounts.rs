//! The node's routes for an account's lifecycle, with the shapes the pinned engine serves: its
//! declaration, its deposits' custody records, its MONKEY entitlement, and its withdrawals, their
//! payouts and their reissue. Reads take what the bot needs and ignore the rest.

use engine_core::Fixed;
use engine_core::access::{AccessReject, EligibilityClass, EligibilityRequest};
use engine_core::command::WithdrawalRequest;
use engine_core::deposits::DepositReceipt;
use engine_core::eip712::{B256, Signature};
use engine_core::emissions::TokenUnits;
use engine_core::event::Event;
use engine_core::types::AccountId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::node::{CallError, Node, Verified};

/// The most entries a page of the withdrawal queue or of payouts carries.
const PAGE: usize = 200;
/// The most pages read of one list: far more than one account's unpaid payouts or the queue.
const MAX_PAGES: usize = 64;

/// `GET /v1/accounts/{address}/eligibility`: the class the account declared, if it has.
#[derive(Debug, Deserialize)]
pub struct EligibilityView {
    pub class: Option<EligibilityClass>,
}

/// `GET /v1/accounts/{address}/emissions`: the proof of the account's cumulative MONKEY entitlement
/// at checkpoint `seq`, as the vault's `claimEmissions` takes it, and whether the vault can check
/// it now (`claimable`); otherwise the proof waits for the vault to record that checkpoint.
#[derive(Debug, Deserialize)]
pub struct EmissionsView {
    pub seq: u64,
    pub account: AccountId,
    pub entitled: TokenUnits,
    pub siblings: Vec<B256>,
    pub newest_entitled: TokenUnits,
    pub claimable: bool,
}

/// One custody record of a vault deposit to the account: the principal it moved into the balance
/// (out of it, in a reorganisation), or the access refusal it is held uncredited for.
#[derive(Debug, Deserialize)]
pub struct DepositRecord {
    pub receipt: DepositReceipt,
    pub principal_delta: Fixed,
    pub uncredited: Option<AccessReject>,
}

/// One of the account's requests in the public withdrawal queue: its place in the whole queue, and
/// what is still to be approved.
#[derive(Debug, Deserialize)]
pub struct QueuedWithdrawal {
    pub position: u64,
    pub remaining: Fixed,
    pub requested_seq: u64,
}

/// An approved, unpaid withdrawal, and its latest authorization once the enclave has signed one.
#[derive(Debug, Deserialize)]
pub struct Payout {
    pub nonce: u64,
    pub amount: Fixed,
    pub withdrawal: Option<SignedWithdrawal>,
}

/// The enclave's signed authorization of a payout, which the vault's `withdraw` takes.
#[derive(Debug, Deserialize)]
pub struct SignedWithdrawal {
    pub account: AccountId,
    pub amount: Fixed,
    pub nonce: u64,
    pub chain_id: u64,
    pub vault: AccountId,
    /// The enclave's signature, as hex.
    pub signature: String,
    pub approved_seq: Option<u64>,
    pub authorization: Option<Authorization>,
}

/// The authorization's terms and its approval's proof against an accepted checkpoint. Times are
/// Unix seconds.
#[derive(Debug, Deserialize)]
pub struct Authorization {
    pub signer_epoch: u64,
    pub issued_at: u64,
    pub expires_at: u64,
    pub checkpoint_seq: u64,
    pub siblings: Vec<B256>,
}

/// The account's unpaid payouts, oldest first, and the node's time at the last page. While the node
/// is still loading them (`loaded` is false) a payout's absence says nothing.
#[derive(Debug)]
pub struct Payouts {
    pub loaded: bool,
    pub payouts: Vec<Payout>,
    pub time_ms: u64,
}

/// The account's signed request for a new authorization of payout `nonce`, replacing the one that
/// expires at `replaces_expires_at`.
#[derive(Debug, Serialize)]
pub struct ReissueBody {
    pub replaces_expires_at: u64,
    pub signature: Signature,
}

/// One page of an account's history, newest first.
#[derive(Deserialize)]
struct HistoryPage<T> {
    events: Vec<T>,
}

/// One page of the withdrawal queue, by the request seq after which the next starts.
#[derive(Deserialize)]
struct QueuePage {
    queue: Vec<QueuedWithdrawal>,
    next: Option<u64>,
}

/// One page of payouts, by the nonce after which the next starts.
#[derive(Deserialize)]
struct PayoutsPage {
    loaded: bool,
    payouts: Vec<Payout>,
    next: Option<u64>,
}

impl Node {
    /// The class `account` declared, if it has; the node answers for an account it never credited.
    /// Error cases:
    /// - see `CallError`
    pub async fn eligibility(&self, account: AccountId) -> Result<Verified<EligibilityView>, CallError> {
        self.get(&format!("/v1/accounts/{account}/eligibility")).await
    }

    /// Records a signed declaration; the events it produced.
    /// Error cases:
    /// - see `CallError`
    pub async fn declare(&self, request: &EligibilityRequest) -> Result<Verified<Vec<Event>>, CallError> {
        self.post("/v1/eligibility", request, Some(request.account)).await
    }

    /// `account`'s MONKEY entitlement proof; a refusal with 404 while no checkpoint holds one.
    /// Error cases:
    /// - see `CallError`
    pub async fn emissions(&self, account: AccountId) -> Result<Verified<EmissionsView>, CallError> {
        self.get(&format!("/v1/accounts/{account}/emissions")).await
    }

    /// The custody records of `account`'s latest vault deposits, newest first.
    /// Error cases:
    /// - see `CallError`
    pub async fn deposit_records(&self, account: AccountId) -> Result<Vec<DepositRecord>, CallError> {
        let path = format!("/v1/accounts/{account}/history?kind=custody_deposit_changed&limit={PAGE}");
        Ok(self.get::<HistoryPage<DepositRecord>>(&path).await?.body.events)
    }

    /// Requests a signed withdrawal; the events it produced.
    /// Error cases:
    /// - see `CallError`
    pub async fn withdraw(&self, request: &WithdrawalRequest) -> Result<Verified<Vec<Event>>, CallError> {
        self.post("/v1/withdrawals", request, Some(request.account)).await
    }

    /// `account`'s requests in the withdrawal queue, in queue order.
    /// Error cases:
    /// - see `CallError`
    /// - a page whose cursor does not move on, or more pages than `MAX_PAGES`
    pub async fn withdrawal_queue(&self, account: AccountId) -> Result<Vec<QueuedWithdrawal>, CallError> {
        let mut queued = Vec::new();
        let mut after: Option<u64> = None;
        for _ in 0..MAX_PAGES {
            let cursor = after.map(|seq| format!("&after={seq}")).unwrap_or_default();
            let path = format!("/v1/accounts/{account}/withdrawals?limit={PAGE}{cursor}");
            let page: QueuePage = self.get(&path).await?.body;
            queued.extend(page.queue);
            match page.next {
                None => return Ok(queued),
                Some(next) if after.is_none_or(|after| next > after) => after = Some(next),
                Some(next) => return Err(stuck(&path, next)),
            }
        }
        Err(too_long(account, "withdrawal queue"))
    }

    /// `account`'s unpaid payouts, every page.
    /// Error cases:
    /// - see `CallError`
    /// - a page whose cursor does not move on, or more pages than `MAX_PAGES`
    pub async fn payouts(&self, account: AccountId) -> Result<Payouts, CallError> {
        let mut payouts = Payouts { loaded: true, payouts: Vec::new(), time_ms: 0 };
        let mut after: Option<u64> = None;
        for _ in 0..MAX_PAGES {
            let cursor = after.map(|nonce| format!("&after={nonce}")).unwrap_or_default();
            let path = format!("/v1/accounts/{account}/payouts?limit={PAGE}{cursor}");
            let answer = self.get::<PayoutsPage>(&path).await?;
            let page = answer.body;
            payouts.time_ms = answer.time_ms;
            payouts.loaded &= page.loaded;
            payouts.payouts.extend(page.payouts);
            match page.next {
                None => return Ok(payouts),
                Some(next) if after.is_none_or(|after| next > after) => after = Some(next),
                Some(next) => return Err(stuck(&path, next)),
            }
        }
        Err(too_long(account, "payouts"))
    }

    /// Asks for a new authorization of `account`'s payout `nonce`, which arrives later.
    /// Error cases:
    /// - see `CallError`
    pub async fn reissue(
        &self,
        account: AccountId,
        nonce: u64,
        request: &ReissueBody,
    ) -> Result<Verified<Value>, CallError> {
        self.post(&format!("/v1/accounts/{account}/withdrawals/{nonce}/reissue"), request, Some(account)).await
    }
}

/// A page whose cursor, `next`, does not move past the page it continues.
fn stuck(path: &str, next: u64) -> CallError {
    CallError::Failed { target: format!("GET {path}"), source: anyhow::anyhow!("the next page starts at {next} again") }
}

/// A list longer than `MAX_PAGES` pages.
fn too_long(account: AccountId, list: &str) -> CallError {
    let source = anyhow::anyhow!("more than {MAX_PAGES} pages");
    CallError::Failed { target: format!("{account}'s {list}"), source }
}
