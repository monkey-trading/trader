//! A transaction: estimated, priced within the cap, signed with the payer's key, sent to every
//! endpoint, and followed to its receipt. It is estimated and signed again on a fresh nonce only
//! once another transaction from the same key took its nonce, read at one block: for a guarded
//! call, once the chain at that block holds the effect of none of the transactions signed for it,
//! whatever the receipts say; otherwise once every endpoint, poll after poll, answers that it holds
//! none of their receipts, and a second effect would fail harmlessly. An endpoint that took it
//! without answering, one that answered that its nonce is spent because it was itself mined, or one
//! that fails to say whether it holds a receipt, leaves it to be followed.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use alloy_consensus::{SignableTransaction, TxEip1559, TxEnvelope};
use alloy_eips::BlockNumberOrTag;
use alloy_eips::eip2718::Encodable2718;
use alloy_primitives::{Address, B256, Bytes, Signature, TxKind, U256};
use alloy_rpc_types_eth::{BlockId, TransactionReceipt};
use alloy_transport::RpcError;
use futures_util::future::BoxFuture;
use tokio::time::{Instant, sleep};
use tracing::{debug, info, warn};

use crate::chain::contracts::revert_reason;
use crate::chain::progress::{self, Progress};
use crate::chain::{Call, Chain, ReadError, TxError, describe, request};
use crate::keys::AccountKey;

/// How many times a transaction is signed again, on a fresh nonce, when another took its nonce.
const NONCE_ATTEMPTS: usize = 3;
/// The gas limit's margin over the estimate, in percent: state can move between the two.
const GAS_HEADROOM_PERCENT: u64 = 20;
/// How many polls in a row, and for how long at least, every endpoint must answer that it holds no
/// receipt for a transaction whose nonce is mined before it counts as replaced: an endpoint's
/// receipts can lag its state. The time is at most half the receipt wait, so the wait can end in it.
const ABSENT_POLLS: u32 = 5;
const ABSENT_FOR: Duration = Duration::from_secs(10);

/// What sending a signed transaction to every endpoint came to.
enum Broadcast {
    /// An endpoint took it, or already had it.
    Sent,
    /// No endpoint answered: one may have taken it.
    Unanswered(String),
    /// An endpoint holds its nonce spent or taken, by it or by another transaction.
    NonceSpent(String),
    /// The endpoints refused it, for this reason.
    Refused(String),
}

/// What the endpoints say of the receipts of the transactions signed for one call.
enum Lookup {
    /// The receipt of the one at this place among them.
    Found(usize, Box<TransactionReceipt>),
    /// Every endpoint answered that it holds none.
    Absent,
    /// No endpoint holds one, and at least one did not answer.
    Unknown,
}

/// What following the transactions signed for one call came to.
enum Followed {
    /// The receipt of the one at this place among them.
    Mined(usize, Box<TransactionReceipt>),
    /// The guard found the one with this hash landed, though no endpoint holds its receipt yet.
    Landed(B256),
    /// Another transaction took the last one's nonce, and none of them landed.
    Replaced,
}

/// What a guarded transaction came to: mined, with its receipt, or found by its guard to have
/// landed, by the hash of the transaction that did, before any endpoint held its receipt.
#[derive(Debug)]
pub enum Sent {
    Mined(Box<TransactionReceipt>),
    Landed(B256),
}

/// A block by which a nonce was seen spent: its number, and its hash, which names it on the chain
/// that endpoint saw it on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spent {
    pub number: u64,
    pub hash: B256,
}

/// A call's guard: given the block by which another transaction may have taken the nonce of the
/// call's last transaction, and the hashes of every transaction signed for the call, the one among
/// them whose effect the chain holds at that block, or `None` when it holds none of theirs. A read
/// it cannot make is an error, and the transaction is followed on.
pub type Guard<'a> = dyn Fn(Spent, Vec<B256>) -> BoxFuture<'a, anyhow::Result<Option<B256>>> + Sync + 'a;

impl Chain {
    /// Sends `call` from `payer`, which signs it and pays its gas, and waits for its receipt. Once
    /// another transaction from the same key took its nonce, it is signed again on a fresh one: a
    /// call that must not take effect twice is sent with `send_guarded`.
    /// Error cases:
    /// - see `TxError`
    pub async fn send(&self, payer: &AccountKey, call: &Call) -> Result<TransactionReceipt, TxError> {
        match self.send_with(payer, call, None).await? {
            Sent::Mined(receipt) => Ok(*receipt),
            Sent::Landed(_) => unreachable!("only a guard finds a transaction landed"),
        }
    }

    /// Sends `call` as `send` does, but signs it again on a fresh nonce only once `guard` says the
    /// chain holds the effect of none of the transactions signed for it, at a block by which
    /// another transaction took the last one's nonce: whatever receipts the endpoints hold, the
    /// call never takes effect twice. While the guard cannot tell, the transaction is followed on.
    /// Error cases:
    /// - see `TxError`
    pub async fn send_guarded(&self, payer: &AccountKey, call: &Call, guard: &Guard<'_>) -> Result<Sent, TxError> {
        self.send_with(payer, call, Some(guard)).await
    }

    /// `send` and `send_guarded`, one transaction of the payer's at a time: a call that arrives while
    /// another of the payer's is being followed waits for it, and reports its progress to the task's
    /// sink (`progress`).
    async fn send_with(&self, payer: &AccountKey, call: &Call, guard: Option<&Guard<'_>>) -> Result<Sent, TxError> {
        let from = Address::from(payer.address().0);
        let turn = {
            let mut senders = self.senders.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            Arc::clone(senders.entry(from).or_default())
        };
        let _turn = turn.lock().await;
        self.send_unqueued(payer, call, guard).await
    }

    /// `send_with`, without waiting for the payer's turn.
    async fn send_unqueued(&self, payer: &AccountKey, call: &Call, guard: Option<&Guard<'_>>) -> Result<Sent, TxError> {
        let from = Address::from(payer.address().0);
        let what = &call.what;
        let not_sent = |reason: String| TxError::NotSent { what: what.clone(), reason };
        let fee = |e: ReadError| not_sent(format!("its fee: {e}"));
        let price = self.read(|endpoint| async move { endpoint.get_gas_price().await }).await.map_err(fee)?;
        let tip =
            self.read(|endpoint| async move { endpoint.get_max_priority_fee_per_gas().await }).await.map_err(fee)?;
        // Room for the base fee to double before the transaction is mined, within the configured cap:
        // an endpoint's price is the chain's word only as far as the cap.
        let max_fee = price.saturating_mul(2).saturating_add(tip);
        if max_fee > self.max_fee {
            let reason = format!("its fee of {max_fee} wei per gas is over chain.max_fee_gwei ({} wei)", self.max_fee);
            return Err(not_sent(reason));
        }
        // A transaction of the payer's still pending, such as a copy of this call that outlived its
        // receipt wait, holds every later nonce: nothing is sent beside it.
        let (mined, pending) = self
            .read(|endpoint| async move {
                let mined = endpoint.get_transaction_count(from, BlockId::latest()).await?;
                Ok((mined, endpoint.get_transaction_count(from, BlockId::pending()).await?))
            })
            .await
            .map_err(|e| not_sent(format!("its nonce: {e}")))?;
        if pending > mined {
            let reason = format!(
                "{from} has a transaction pending at nonce {mined}: nothing is sent until it is mined or dropped"
            );
            return Err(not_sent(reason));
        }
        // Each transaction signed for the call, and its gas limit.
        let mut signed: Vec<(B256, u64)> = Vec::new();
        for attempt in 1..=NONCE_ATTEMPTS {
            // Estimated on each attempt: one signed again runs on the state that spent the last nonce.
            let gas_limit = self.gas_limit(from, call).await?;
            let nonce = self
                .read(|endpoint| async move { endpoint.get_transaction_count(from, BlockId::pending()).await })
                .await
                .map_err(|e| not_sent(format!("its nonce: {e}")))?;
            let tx = TxEip1559 {
                chain_id: self.chain_id,
                nonce,
                gas_limit,
                max_fee_per_gas: max_fee,
                max_priority_fee_per_gas: tip,
                to: TxKind::Call(call.to),
                value: U256::ZERO,
                access_list: Default::default(),
                input: call.data.clone(),
            };
            let signature = payer.sign(&tx.signature_hash());
            let signature =
                Signature::from_raw(signature.as_slice()).expect("eip712::sign gives r, s and v as 27 or 28");
            let envelope = TxEnvelope::from(tx.into_signed(signature));
            let hash = *envelope.tx_hash();
            match self.broadcast(&envelope.encoded_2718()).await {
                Broadcast::Sent => {
                    info!(tx = %hash, nonce, gas_limit, max_fee, to = %call.to, from = %from, "{what}: sent");
                }
                Broadcast::Unanswered(reason) => {
                    warn!(tx = %hash, nonce, "{what}: no endpoint answered ({reason}); following it, as one may have taken it");
                }
                Broadcast::NonceSpent(message) => {
                    info!(tx = %hash, nonce, "{what}: the nonce is spent or taken ({message}); following it");
                }
                Broadcast::Refused(message) => return Err(not_sent(message)),
            }
            progress::report(Progress::Sent { tx: hash, what: what.clone() });
            signed.push((hash, gas_limit));
            match self.follow(call, from, nonce, &signed, guard).await? {
                Followed::Mined(at, receipt) => {
                    let (hash, gas_limit) = signed[at];
                    return self
                        .checked(call, hash, *receipt, gas_limit)
                        .await
                        .map(|receipt| Sent::Mined(Box::new(receipt)));
                }
                Followed::Landed(hash) => {
                    info!(tx = %hash, "{what}: landed, though no endpoint holds its receipt yet");
                    progress::report(Progress::Mined { tx: hash });
                    return Ok(Sent::Landed(hash));
                }
                Followed::Replaced => progress::report(Progress::Replaced { tx: hash }),
            }
            if attempt == NONCE_ATTEMPTS {
                return Err(not_sent(format!("another transaction from {from} took each nonce it was signed on")));
            }
            warn!(nonce, "{what}: another transaction from the same key took the nonce; signing again");
        }
        unreachable!("the last attempt returns")
    }

    /// Follows the transaction `tx`, sent earlier, for up to `wait`, sending nothing: how it came out
    /// once an endpoint holds its receipt, or `None` when none does in time. For a transaction whose
    /// call failed with `NoReceipt`, which may still be mined.
    pub async fn settle(&self, tx: B256, wait: Duration) -> Option<Progress> {
        let deadline = Instant::now() + wait;
        loop {
            if let Lookup::Found(_, receipt) = self.lookup(&[tx]).await {
                if let Some(block) = receipt.block_number {
                    self.floor.fetch_max(block, Ordering::Relaxed);
                }
                info!(tx = %tx, block = receipt.block_number, "a transaction past its receipt wait is mined");
                return Some(if receipt.status() { Progress::Mined { tx } } else { Progress::Reverted { tx } });
            }
            if Instant::now() >= deadline {
                return None;
            }
            sleep(self.poll_interval).await;
        }
    }

    /// The gas limit for `call` from `from`: its estimate, with headroom.
    /// Error cases:
    /// - the call would revert (`WouldRevert`), or no estimate is read (`NotSent`)
    async fn gas_limit(&self, from: Address, call: &Call) -> Result<u64, TxError> {
        let what = &call.what;
        let estimate = request(Some(from), call.to, call.data.clone());
        let gas = match self
            .read(|endpoint| {
                let estimate = estimate.clone();
                async move { endpoint.estimate_gas(estimate).await }
            })
            .await
        {
            Ok(gas) => gas,
            Err(ReadError::Answer { revert: Some(data), .. }) => {
                let reason = revert_reason(&data);
                return Err(TxError::WouldRevert { what: what.clone(), reason, revert: Some(data) });
            }
            Err(ReadError::Answer { message, .. }) if message.contains("revert") => {
                return Err(TxError::WouldRevert { what: what.clone(), reason: message, revert: None });
            }
            Err(e) => return Err(TxError::NotSent { what: what.clone(), reason: format!("estimating its gas: {e}") }),
        };
        Ok(gas.saturating_add(gas.saturating_mul(GAS_HEADROOM_PERCENT) / 100))
    }

    /// Sends a signed transaction to every endpoint: it is sent once one takes it or already has it,
    /// and may be once one does not answer.
    async fn broadcast(&self, raw: &[u8]) -> Broadcast {
        let (mut sent, mut spent, mut refused, mut unanswered) = (false, None, None, None);
        for (index, endpoint) in self.endpoints.iter().enumerate() {
            match endpoint.send_raw_transaction(raw).await {
                Ok(_) => sent = true,
                Err(RpcError::ErrorResp(payload)) => {
                    let message = payload.message.to_lowercase();
                    debug!(endpoint = index, "the transaction was refused: {message}");
                    if message.contains("already known") || message.contains("known transaction") {
                        sent = true;
                    } else if message.contains("nonce too low")
                        || message.contains("replacement transaction underpriced")
                    {
                        spent = Some(message);
                    } else {
                        refused = Some(message);
                    }
                }
                Err(e) => {
                    let reason = describe(&e);
                    debug!(endpoint = index, "the transaction was not answered: {reason}");
                    unanswered = Some(reason);
                }
            }
        }
        // An endpoint that did not answer may have taken it, whatever another refused it for.
        match (sent, spent, unanswered, refused) {
            (true, ..) => Broadcast::Sent,
            (false, Some(message), ..) => Broadcast::NonceSpent(message),
            (false, None, Some(reason), Some(refused)) => {
                Broadcast::Unanswered(format!("{reason}; another: {refused}"))
            }
            (false, None, Some(reason), None) => Broadcast::Unanswered(reason),
            (false, None, None, Some(message)) => Broadcast::Refused(message),
            (false, None, None, None) => Broadcast::Unanswered(String::from("no endpoint")),
        }
    }

    /// Follows the transactions `signed` for `call`, the last on `from`'s nonce `nonce`, until an
    /// endpoint holds one's receipt, or until another transaction took that nonce: by a block at
    /// which `guard`, when given, finds none of them landed, or, without one, once, on
    /// `ABSENT_POLLS` polls in a row over `ABSENT_FOR`, the nonce is mined and every endpoint answers
    /// that it holds none of their receipts. The nonce is read at one block on each poll, so a block
    /// that is undone takes back what it showed.
    /// Error cases:
    /// - neither is shown within `receipt_timeout`: one of them may yet be mined
    async fn follow(
        &self,
        call: &Call,
        from: Address,
        nonce: u64,
        signed: &[(B256, u64)],
        guard: Option<&Guard<'_>>,
    ) -> Result<Followed, TxError> {
        let deadline = Instant::now() + self.receipt_timeout;
        let margin = ABSENT_FOR.min(self.receipt_timeout / 2);
        let hashes: Vec<B256> = signed.iter().map(|(hash, _)| *hash).collect();
        let (mut absent, mut absent_since) = (0, None);
        loop {
            let lookup = self.lookup(&hashes).await;
            if let Lookup::Found(at, receipt) = lookup {
                return Ok(Followed::Mined(at, receipt));
            }
            let spent = self.spent_by(from, nonce).await;
            match (spent, guard) {
                (Some(block), Some(guard)) => match guard(block, hashes.clone()).await {
                    Ok(Some(hash)) => return Ok(Followed::Landed(hash)),
                    Ok(None) => return Ok(Followed::Replaced),
                    Err(e) => debug!(block = block.number, "whether {} landed was not read: {e:#}", call.what),
                },
                (Some(_), None) if matches!(lookup, Lookup::Absent) => {
                    absent += 1;
                    let since: Instant = *absent_since.get_or_insert_with(Instant::now);
                    if absent >= ABSENT_POLLS && since.elapsed() >= margin {
                        return Ok(Followed::Replaced);
                    }
                }
                _ => (absent, absent_since) = (0, None),
            }
            if Instant::now() >= deadline {
                let hash = hashes.last().copied().unwrap_or_default();
                return Err(TxError::NoReceipt { what: call.what.clone(), hash, wait: self.receipt_timeout });
            }
            sleep(self.poll_interval).await;
        }
    }

    /// The latest block on an endpoint where `from`'s nonce `nonce` is mined by that block, its hash
    /// and the count read at that hash, all from the one endpoint; `None` while it is not mined, or
    /// none is read.
    async fn spent_by(&self, from: Address, nonce: u64) -> Option<Spent> {
        let read = self
            .read(|endpoint| async move {
                let number = endpoint.get_block_number().await?;
                let block = endpoint.get_block_by_number(BlockNumberOrTag::Number(number)).await?;
                let hash = block
                    .map(|block| block.header.hash)
                    .ok_or_else(|| RpcError::local_usage_str("the latest block is not held"))?;
                let count = endpoint.get_transaction_count(from, BlockId::hash(hash)).await?;
                Ok((Spent { number, hash }, count))
            })
            .await;
        read.ok().and_then(|(spent, count)| (count > nonce).then_some(spent))
    }

    /// What the endpoints say of the receipts of `hashes`: the first an endpoint holds, or whether
    /// every endpoint answered.
    async fn lookup(&self, hashes: &[B256]) -> Lookup {
        let mut unknown = false;
        for endpoint in &self.endpoints {
            for (at, hash) in hashes.iter().enumerate() {
                match endpoint.get_transaction_receipt(*hash).await {
                    Ok(Some(receipt)) => return Lookup::Found(at, Box::new(receipt)),
                    Ok(None) => {}
                    Err(_) => unknown = true,
                }
            }
        }
        if unknown { Lookup::Unknown } else { Lookup::Absent }
    }

    /// `receipt` when its transaction succeeded, and raises the floor reads must reach to its
    /// block either way.
    /// Error cases:
    /// - the transaction reverted
    async fn checked(
        &self,
        call: &Call,
        hash: B256,
        receipt: TransactionReceipt,
        gas_limit: u64,
    ) -> Result<TransactionReceipt, TxError> {
        if let Some(block) = receipt.block_number {
            self.floor.fetch_max(block, Ordering::Relaxed);
        }
        if receipt.status() {
            info!(tx = %hash, block = receipt.block_number, gas_used = receipt.gas_used, "{}: mined", call.what);
            progress::report(Progress::Mined { tx: hash });
            return Ok(receipt);
        }
        progress::report(Progress::Reverted { tx: hash });
        let (reason, revert) = if receipt.gas_used >= gas_limit {
            (format!("it ran out of its {gas_limit} gas"), None)
        } else {
            self.replayed_reason(call, &receipt).await
        };
        Err(TxError::Reverted { what: call.what.clone(), hash, reason, revert })
    }

    /// Why a reverted transaction reverted, and its revert data: its call replayed at its block.
    async fn replayed_reason(&self, call: &Call, receipt: &TransactionReceipt) -> (String, Option<Bytes>) {
        let replay = request(Some(receipt.from), call.to, call.data.clone());
        let block = BlockId::number(receipt.block_number.unwrap_or_default());
        match self
            .read(|endpoint| {
                let replay = replay.clone();
                async move { endpoint.call_at(replay, block).await }
            })
            .await
        {
            Err(ReadError::Answer { revert: Some(data), .. }) => (revert_reason(&data), Some(data)),
            Err(ReadError::Answer { message, .. }) => (message, None),
            Ok(_) => (String::from("its call no longer reverts, so the reason is unknown"), None),
            Err(e) => (format!("the reason was not read: {e}"), None),
        }
    }
}
