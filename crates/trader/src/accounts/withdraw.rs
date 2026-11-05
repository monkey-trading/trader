//! Withdrawals: a signed request to the node (`POST /v1/withdrawals`), which reserves the amount in
//! the public FIFO queue and approves it in parts as cash allows; each approved part becomes a
//! payout the enclave authorizes once a checkpoint holds it, and the vault's `withdraw` pays it to
//! the account, whoever sends it. An authorization that expires unpaid is replaced on the account's
//! signed request.

use std::collections::{BTreeMap, BTreeSet};

use alloy_primitives::{Address, B256};
use alloy_sol_types::{Eip712Domain, SolCall, SolInterface, SolStruct, sol};
use anyhow::{Context, bail, ensure};
use engine_core::Fixed;
use engine_core::auth::RequestAuth;
use engine_core::command::WithdrawalRequest;
use engine_core::eip712::withdrawal_hash;
use engine_core::event::Event;
use engine_core::types::AccountId;
use tokio::time::{Instant, sleep};
use tracing::{Instrument, info, info_span, warn};

use crate::accounts::{Lifecycle, REQUEST_EXPIRY_MS, usdc_units};
use crate::chain::contracts::Vault;
use crate::chain::{Call, TxError};
use crate::keys::AccountKey;
use crate::node::CallError;
use crate::node::accounts::{Payout, ReissueBody, SignedWithdrawal};

/// How close to its expiry, in seconds, an authorization is not sent but replaced once expired: it
/// might expire before it is mined.
const EXPIRY_MARGIN_S: u64 = 120;
/// How many polls an expired authorization may stay unreplaced before the bot asks again: the node
/// holds a queued reissue in memory and asks to be asked again after it restarts.
const REISSUE_AGAIN_POLLS: u32 = 30;

sol! {
    /// The account's request to replace the authorization of its payout `payoutNonce` that expires
    /// at `replacesExpiresAt`, as the node checks it (`api/reissue.rs` at the pinned engine commit).
    struct ReissueRequest {
        address account;
        uint64 payoutNonce;
        uint64 replacesExpiresAt;
    }
}

/// One payout the vault paid to the account.
#[derive(Debug, PartialEq, Eq)]
pub struct Paid {
    pub nonce: u64,
    pub amount: Fixed,
    pub tx: B256,
}

impl Lifecycle {
    /// Withdraws `amount` USDC from `key`'s account when given, then pays every unpaid payout of
    /// the account to the account, each sent by the funding key when there is one: it waits for the
    /// queue to approve the request, for each payout's authorization, and for a replacement of any
    /// that expired. Without an amount it only pays what is already approved.
    /// Error cases:
    /// - the amount is not whole USDC base units above zero
    /// - the node refuses the request, or gives no answer to it
    /// - a payout's authorization is not for this account, vault and chain
    /// - a transaction would revert, reverts, or gets no receipt (see `TxError`)
    /// - the request or a payout is still waiting after `settle_timeout`
    pub async fn withdraw(&self, key: &AccountKey, amount: Option<Fixed>) -> anyhow::Result<Vec<Paid>> {
        let account = key.address();
        let span = info_span!("withdraw", id = %account);
        async {
            // The chain that pays it is checked before anything is asked of the node.
            self.tokens().await?;
            let deadline = Instant::now() + self.settle_timeout;
            if let Some(amount) = amount {
                let requested_seq = self.request(key, amount, deadline).await?;
                self.approved(account, requested_seq, deadline).await?;
            }
            self.pay(key, deadline).await
        }
        .instrument(span)
        .await
    }

    /// Requests a withdrawal of `amount`, asking again on a retry-later answer or a stale nonce
    /// until `deadline`; the seq the queue knows the request by.
    /// Error cases:
    /// - the amount is not whole USDC base units above zero
    /// - the node refuses it, in the log or outside it, or gives no answer to it
    /// - it is still not taken at `deadline`
    async fn request(&self, key: &AccountKey, amount: Fixed, deadline: Instant) -> anyhow::Result<u64> {
        usdc_units(amount)?;
        let account = key.address();
        loop {
            if Instant::now() >= deadline {
                bail!("the node did not take the withdrawal request within {:?}", self.settle_timeout);
            }
            let (nonce, now) = self.next_nonce(account).await?;
            let expires_at = now.saturating_add(REQUEST_EXPIRY_MS);
            let hash = withdrawal_hash(&self.domain, account, amount, nonce, expires_at);
            let auth = RequestAuth { nonce, expires_at, signature: key.sign(&hash) };
            let request = WithdrawalRequest { account, amount, auth: Some(auth) };
            info!(%amount, nonce, expires_at, "requesting a withdrawal");
            let error = match self.node.withdraw(&request).await {
                Ok(answer) => return queued(&answer.body),
                Err(error) => error,
            };
            match (self.retry_later(&error), &error) {
                (Some(wait), _) => {
                    info!(?wait, "the node asks to request later: {error}");
                    sleep(wait).await;
                }
                (None, CallError::Refused { status: 409, .. }) => info!("the nonce moved on; signing again"),
                (None, CallError::Failed { .. }) => {
                    return Err(anyhow::Error::from(error)).with_context(|| {
                        format!("the request may still enter the log until {expires_at}: see `trader accounts status` before asking again")
                    });
                }
                (None, _) => return Err(error.into()),
            }
        }
    }

    /// Waits until the queue holds nothing more of request `requested_seq`: every part approved.
    /// Error cases:
    /// - a read fails, or the request is still queued after `deadline`
    async fn approved(&self, account: AccountId, requested_seq: u64, deadline: Instant) -> anyhow::Result<()> {
        let mut position = None;
        loop {
            let queued = self.node.withdrawal_queue(account).await?;
            let Some(mine) = queued.iter().find(|queued| queued.requested_seq == requested_seq) else {
                info!(requested_seq, "the withdrawal is approved");
                return Ok(());
            };
            if position != Some(mine.position) {
                info!(position = mine.position, remaining = %mine.remaining, "the withdrawal waits in the queue");
                position = Some(mine.position);
            }
            if Instant::now() >= deadline {
                bail!("the withdrawal is still queued at position {} after {:?}", mine.position, self.settle_timeout);
            }
            sleep(self.poll_interval).await;
        }
    }

    /// Pays each of the account's unpaid payouts once its authorization is signed and usable,
    /// asking for a replacement of one that expired, until none is left unpaid.
    /// Error cases:
    /// - a read fails, or an authorization does not match the account, vault and chain
    /// - a payment would revert, reverts, or gets no receipt
    /// - a payout is still unpaid after `deadline`
    async fn pay(&self, key: &AccountKey, deadline: Instant) -> anyhow::Result<Vec<Paid>> {
        let account = key.address();
        let wallet = Address::from(account.0);
        let mut paid = Vec::new();
        let mut done = BTreeSet::new();
        // Each expired authorization the bot asked to replace, and the polls since it asked.
        let mut reissued: BTreeMap<(u64, u64), u32> = BTreeMap::new();
        loop {
            let answer = self.node.payouts(account).await?;
            let now_s = answer.time_ms / 1000;
            let unpaid: Vec<&Payout> = answer.payouts.iter().filter(|payout| !done.contains(&payout.nonce)).collect();
            if unpaid.is_empty() && answer.loaded {
                return Ok(paid);
            }
            for payout in unpaid {
                let nonce = payout.nonce;
                if self.chain.view(self.vault, Vault::usedNoncesCall { account: wallet, nonce }).await? {
                    info!(nonce, "the payout is paid already");
                    done.insert(nonce);
                    continue;
                }
                let Some(signed) = &payout.withdrawal else { continue };
                let call = self.withdraw_call(account, payout, signed)?;
                let expires_at = signed.authorization.as_ref().expect("checked by withdraw_call").expires_at;
                if expires_at > now_s.saturating_add(EXPIRY_MARGIN_S) {
                    let what = format!("paying withdrawal {nonce} of {} USDC", payout.amount);
                    let call = Call { to: self.vault, data: call.abi_encode().into(), what };
                    match self.chain.send(self.payer(key), &call).await {
                        Ok(receipt) => {
                            info!(nonce, amount = %payout.amount, tx = %receipt.transaction_hash, "withdrawal paid");
                            paid.push(Paid { nonce, amount: payout.amount, tx: receipt.transaction_hash });
                        }
                        Err(TxError::WouldRevert { revert: Some(data), .. }) if already_paid(&data) => {
                            info!(nonce, "the payout is paid already");
                        }
                        Err(e) => return Err(e.into()),
                    }
                    done.insert(nonce);
                } else if expires_at <= now_s {
                    let polls = reissued.entry((nonce, expires_at)).or_insert(REISSUE_AGAIN_POLLS);
                    if *polls >= REISSUE_AGAIN_POLLS {
                        self.reissue(key, nonce, expires_at).await?;
                        *polls = 0;
                    }
                    *polls += 1;
                }
            }
            if Instant::now() >= deadline {
                bail!("withdrawals are still unpaid after {:?}: see `trader accounts status`", self.settle_timeout);
            }
            sleep(self.poll_interval).await;
        }
    }

    /// The vault call that pays `payout` by its authorization `signed`.
    /// Error cases:
    /// - the authorization is for another account, payout, vault or chain, or lacks its terms
    fn withdraw_call(
        &self,
        account: AccountId,
        payout: &Payout,
        signed: &SignedWithdrawal,
    ) -> anyhow::Result<Vault::withdrawCall> {
        let nonce = payout.nonce;
        ensure!(signed.account == account, "payout {nonce}'s authorization pays {}, not {account}", signed.account);
        ensure!(
            signed.nonce == nonce && signed.amount == payout.amount,
            "payout {nonce}'s authorization is for another payout"
        );
        ensure!(
            Address::from(signed.vault.0) == self.vault && signed.chain_id == self.chain_id,
            "payout {nonce}'s authorization is for another vault or chain"
        );
        let auth =
            signed.authorization.as_ref().with_context(|| format!("payout {nonce}'s authorization has no terms"))?;
        let approved_seq = signed.approved_seq.with_context(|| format!("payout {nonce} has no approval seq"))?;
        let signature = hex::decode(signed.signature.trim_start_matches("0x"))
            .with_context(|| format!("payout {nonce}'s signature is not hex"))?;
        Ok(Vault::withdrawCall {
            withdrawal: Vault::Withdrawal {
                account: Address::from(account.0),
                amount: usdc_units(payout.amount)?,
                nonce,
                approvedSeq: approved_seq,
                signerEpoch: auth.signer_epoch,
                issuedAt: auth.issued_at,
                expiresAt: auth.expires_at,
            },
            checkpointSeq: auth.checkpoint_seq,
            siblings: auth.siblings.clone(),
            signature: signature.into(),
        })
    }

    /// Asks the node to replace payout `nonce`'s authorization, which expired at `expires_at`.
    /// Error cases:
    /// - the node refuses it for a reason a wait does not cure, such as a signature it does not take
    async fn reissue(&self, key: &AccountKey, nonce: u64, expires_at: u64) -> anyhow::Result<()> {
        let account = key.address();
        let hash = reissue_hash(&self.domain, account, nonce, expires_at);
        let request = ReissueBody { replaces_expires_at: expires_at, signature: key.sign(&hash) };
        info!(nonce, expires_at, "the authorization expired unpaid; asking for another");
        match self.node.reissue(account, nonce, &request).await {
            Ok(_) => Ok(()),
            // Already replaced, or not yet expired by the node's clock: the payouts show which, and the
            // bot asks again later if they still show it expired.
            Err(e @ CallError::Refused { status: 400, .. }) => {
                warn!("the reissue was not taken: {e}");
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }
}

/// The seq the queue knows a withdrawal by, from the node's events for its request.
/// Error cases:
/// - the events refuse it, or neither queue nor refuse it
fn queued(events: &[Event]) -> anyhow::Result<u64> {
    for event in events {
        match event {
            Event::WithdrawalRejected { reason, .. } => bail!("the node refused the withdrawal: {reason:?}"),
            Event::WithdrawalQueued { requested_seq, .. } => {
                info!(requested_seq, "the withdrawal is queued");
                return Ok(*requested_seq);
            }
            _ => {}
        }
    }
    bail!("the node's answer neither queues nor refuses the withdrawal: {events:?}")
}

/// Whether revert data is the vault's refusal of a payout it paid already.
fn already_paid(data: &[u8]) -> bool {
    matches!(Vault::VaultErrors::abi_decode(data), Ok(Vault::VaultErrors::WithdrawalAlreadyPaid(_)))
}

/// The digest `account` signs to replace payout `nonce`'s authorization that expires at
/// `replaces_expires_at`, under the requests' EIP-712 domain.
pub fn reissue_hash(domain: &Eip712Domain, account: AccountId, nonce: u64, replaces_expires_at: u64) -> B256 {
    ReissueRequest { account: Address::from(account.0), payoutNonce: nonce, replacesExpiresAt: replaces_expires_at }
        .eip712_signing_hash(domain)
}

#[cfg(test)]
mod tests {
    use engine_core::eip712;

    use super::*;

    /// The digest is the one the node checks, computed independently with Foundry's `cast` from the
    /// node's definition: keccak256(0x1901 ‖ domain separator ‖ keccak256(abi.encode(typehash,
    /// account, payoutNonce, replacesExpiresAt))).
    #[test]
    fn the_reissue_digest_is_the_nodes() {
        let account: AccountId = "0x19E7E376E7C213B7E7e7e46cc70A5dD086DAff2A".parse().unwrap();
        let domain = eip712::domain(31337, AccountId([0; 20]));
        let expected: B256 = "0x549d84ebb05332df707290ab910af68ca8af16b9d32463d52f7f68875179d0b8".parse().unwrap();
        assert_eq!(reissue_hash(&domain, account, 7, 1_800_003_600), expected);
        assert_ne!(reissue_hash(&domain, account, 8, 1_800_003_600), expected);
        assert_ne!(reissue_hash(&domain, account, 7, 1_800_003_601), expected);
    }
}
