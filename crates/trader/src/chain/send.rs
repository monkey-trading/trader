//! A transaction: estimated, priced within the cap, signed with the payer's key, sent to every
//! endpoint, and followed to its receipt. It is estimated and signed again on a fresh nonce only
//! once another transaction from the same key took its nonce: the nonce is mined and every
//! endpoint, poll after poll, answers that it holds no receipt for it. An endpoint that took it
//! without answering, one that answered that its nonce is spent because it was itself mined, or one
//! that fails to say whether it holds the receipt, leaves it to be followed, never sent twice.

use std::sync::atomic::Ordering;

use alloy_consensus::{SignableTransaction, TxEip1559, TxEnvelope};
use alloy_eips::eip2718::Encodable2718;
use alloy_primitives::{Address, B256, Bytes, Signature, TxKind, U256};
use alloy_provider::Provider;
use alloy_provider::transport::RpcError;
use alloy_rpc_types_eth::{BlockId, TransactionReceipt};
use tokio::time::{Instant, sleep};
use tracing::{debug, info, warn};

use crate::chain::contracts::revert_reason;
use crate::chain::{Call, Chain, ReadError, TxError, describe, request};
use crate::keys::AccountKey;

/// How many times a transaction is signed again, on a fresh nonce, when another took its nonce.
const NONCE_ATTEMPTS: usize = 3;
/// The gas limit's margin over the estimate, in percent: state can move between the two.
const GAS_HEADROOM_PERCENT: u64 = 20;
/// How many polls in a row every endpoint must answer that it holds no receipt for a transaction
/// whose nonce is mined before it counts as replaced: an endpoint can lag the block that mined it.
const ABSENT_POLLS: u32 = 5;

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

/// What the endpoints say of a transaction's receipt.
enum Lookup {
    Found(Box<TransactionReceipt>),
    /// Every endpoint answered that it holds none.
    Absent,
    /// No endpoint holds one, and at least one did not answer.
    Unknown,
}

impl Chain {
    /// Sends `call` from `payer`, which signs it and pays its gas, and waits for its receipt.
    /// Error cases:
    /// - see `TxError`
    pub async fn send(&self, payer: &AccountKey, call: &Call) -> Result<TransactionReceipt, TxError> {
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
        for attempt in 1..=NONCE_ATTEMPTS {
            // Estimated on each attempt: one signed again runs on the state that spent the last nonce.
            let gas_limit = self.gas_limit(from, call).await?;
            let nonce = self
                .read(|endpoint| async move { endpoint.get_transaction_count(from).pending().await })
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
            if let Some(receipt) = self.follow(call, from, nonce, hash).await? {
                return self.checked(call, hash, receipt, gas_limit).await;
            }
            if attempt == NONCE_ATTEMPTS {
                return Err(not_sent(format!("another transaction from {from} took each nonce it was signed on")));
            }
            warn!(nonce, "{what}: another transaction from the same key took the nonce; signing again");
        }
        unreachable!("the last attempt returns")
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

    /// Follows `hash`, signed on `from`'s nonce `nonce`: its receipt once an endpoint holds it, or
    /// `None` once another transaction took the nonce, which is when the nonce is mined and every
    /// endpoint has answered that it holds no receipt for `hash` on `ABSENT_POLLS` polls in a row
    /// since.
    /// Error cases:
    /// - neither is shown within `receipt_timeout`: `hash` may yet be mined
    async fn follow(
        &self,
        call: &Call,
        from: Address,
        nonce: u64,
        hash: B256,
    ) -> Result<Option<TransactionReceipt>, TxError> {
        let deadline = Instant::now() + self.receipt_timeout;
        let (mut mined, mut absent) = (false, 0);
        loop {
            let lookup = self.lookup(hash).await;
            if let Lookup::Found(receipt) = lookup {
                return Ok(Some(*receipt));
            }
            if !mined {
                let count =
                    self.read(|endpoint| async move { endpoint.get_transaction_count(from).latest().await }).await;
                // Its nonce is mined: by this transaction only if a receipt shows it.
                mined = count.is_ok_and(|count| count > nonce);
            } else if matches!(lookup, Lookup::Absent) {
                absent += 1;
                if absent >= ABSENT_POLLS {
                    return Ok(None);
                }
            } else {
                absent = 0;
            }
            if Instant::now() >= deadline {
                return Err(TxError::NoReceipt { what: call.what.clone(), hash, wait: self.receipt_timeout });
            }
            sleep(self.poll_interval).await;
        }
    }

    /// What the endpoints say of `hash`'s receipt: the first that holds it, or whether every
    /// endpoint answered.
    async fn lookup(&self, hash: B256) -> Lookup {
        let mut unknown = false;
        for endpoint in &self.endpoints {
            match endpoint.get_transaction_receipt(hash).await {
                Ok(Some(receipt)) => return Lookup::Found(Box::new(receipt)),
                Ok(None) => {}
                Err(_) => unknown = true,
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
            return Ok(receipt);
        }
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
                async move { endpoint.call(replay).block(block).await }
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
