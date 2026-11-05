//! A transaction: estimated, priced within the cap, signed with the payer's key, sent to every
//! endpoint, and followed to its receipt. A transaction is signed again on a fresh nonce only once
//! its nonce is mined by another: an endpoint that took it without answering, or answered that its
//! nonce is spent because it was itself mined, leaves it to be followed, never sent twice.

use std::sync::atomic::Ordering;

use alloy_consensus::{SignableTransaction, TxEip1559, TxEnvelope};
use alloy_eips::eip2718::Encodable2718;
use alloy_primitives::{Address, B256, Signature, TxKind, U256};
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

impl Chain {
    /// Sends `call` from `payer`, which signs it and pays its gas, and waits for its receipt.
    /// Error cases:
    /// - see `TxError`
    pub async fn send(&self, payer: &AccountKey, call: &Call) -> Result<TransactionReceipt, TxError> {
        let from = Address::from(payer.address().0);
        let what = &call.what;
        let not_sent = |reason: String| TxError::NotSent { what: what.clone(), reason };
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
            Err(e) => return Err(not_sent(format!("estimating its gas: {e}"))),
        };
        let gas_limit = gas.saturating_add(gas.saturating_mul(GAS_HEADROOM_PERCENT) / 100);
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
                    return self.landed(call, hash, gas_limit).await;
                }
                Broadcast::Unanswered(reason) => {
                    warn!(tx = %hash, nonce, "{what}: no endpoint answered ({reason}); following it, as one may have taken it");
                    return self.landed(call, hash, gas_limit).await;
                }
                Broadcast::NonceSpent(message) => {
                    info!(tx = %hash, nonce, "{what}: the nonce is spent or taken ({message}); waiting for it to be mined");
                    if let Some(receipt) = self.spent_by(call, from, nonce, hash).await? {
                        return self.checked(call, hash, receipt, gas_limit).await;
                    }
                    if attempt == NONCE_ATTEMPTS {
                        return Err(not_sent(message));
                    }
                    warn!(nonce, "{what}: another transaction spent the nonce; signing again");
                }
                Broadcast::Refused(message) => return Err(not_sent(message)),
            }
        }
        unreachable!("the last attempt returns")
    }

    /// Sends a signed transaction to every endpoint: it is sent once one takes it or already has it.
    async fn broadcast(&self, raw: &[u8]) -> Broadcast {
        let (mut sent, mut spent, mut refused) = (false, None, None);
        let mut unanswered = String::from("no endpoint");
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
                    unanswered = describe(&e);
                    debug!(endpoint = index, "the transaction was not answered: {unanswered}");
                }
            }
        }
        match (sent, spent, refused) {
            (true, ..) => Broadcast::Sent,
            (false, Some(message), _) => Broadcast::NonceSpent(message),
            (false, None, Some(message)) => Broadcast::Refused(message),
            (false, None, None) => Broadcast::Unanswered(unanswered),
        }
    }

    /// Waits for `hash`'s receipt, from whichever endpoint has it first, and checks it succeeded.
    /// Error cases:
    /// - the transaction reverted, or no receipt came within the wait
    async fn landed(&self, call: &Call, hash: B256, gas_limit: u64) -> Result<TransactionReceipt, TxError> {
        let deadline = Instant::now() + self.receipt_timeout;
        loop {
            if let Some(receipt) = self.receipt(hash).await {
                return self.checked(call, hash, receipt, gas_limit).await;
            }
            if Instant::now() >= deadline {
                return Err(TxError::NoReceipt { what: call.what.clone(), hash, wait: self.receipt_timeout });
            }
            sleep(self.poll_interval).await;
        }
    }

    /// Once `from`'s nonce `nonce` is mined, the receipt of `hash` if it is the transaction that
    /// spent it, or `None` when another did.
    /// Error cases:
    /// - the nonce is still not mined after `receipt_timeout`: `hash` may yet be
    async fn spent_by(
        &self,
        call: &Call,
        from: Address,
        nonce: u64,
        hash: B256,
    ) -> Result<Option<TransactionReceipt>, TxError> {
        let deadline = Instant::now() + self.receipt_timeout;
        loop {
            if let Some(receipt) = self.receipt(hash).await {
                return Ok(Some(receipt));
            }
            let mined = self.read(|endpoint| async move { endpoint.get_transaction_count(from).latest().await }).await;
            if mined.is_ok_and(|mined| mined > nonce) {
                // Its nonce is mined: by this transaction if a receipt shows it now.
                return Ok(self.receipt(hash).await);
            }
            if Instant::now() >= deadline {
                return Err(TxError::NoReceipt { what: call.what.clone(), hash, wait: self.receipt_timeout });
            }
            sleep(self.poll_interval).await;
        }
    }

    /// `hash`'s receipt from the first endpoint that has it.
    async fn receipt(&self, hash: B256) -> Option<TransactionReceipt> {
        for endpoint in &self.endpoints {
            if let Ok(Some(receipt)) = endpoint.get_transaction_receipt(hash).await {
                return Some(receipt);
            }
        }
        None
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
        let reason = if receipt.gas_used >= gas_limit {
            format!("it ran out of its {gas_limit} gas")
        } else {
            self.replayed_reason(call, &receipt).await
        };
        Err(TxError::Reverted { what: call.what.clone(), hash, reason })
    }
    /// Why a reverted transaction reverted: its call replayed at its block.
    async fn replayed_reason(&self, call: &Call, receipt: &TransactionReceipt) -> String {
        let replay = request(Some(receipt.from), call.to, call.data.clone());
        let block = BlockId::number(receipt.block_number.unwrap_or_default());
        match self
            .read(|endpoint| {
                let replay = replay.clone();
                async move { endpoint.call(replay).block(block).await }
            })
            .await
        {
            Err(ReadError::Answer { revert: Some(data), .. }) => revert_reason(&data),
            Err(ReadError::Answer { message, .. }) => message,
            Ok(_) => String::from("its call no longer reverts, so the reason is unknown"),
            Err(e) => format!("the reason was not read: {e}"),
        }
    }
}
