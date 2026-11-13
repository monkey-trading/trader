//! MONKEY claims: the account's entitlement and its proof from the node, claimed with the vault's
//! `claimEmissions`, which mints to the account whoever sends it. On demand, or while the bot runs,
//! once the unclaimed amount reaches a threshold.

use std::sync::Arc;
use std::time::Duration;

use alloy_primitives::{Address, B256, U256};
use alloy_sol_types::{SolCall, SolEvent, SolInterface};
use anyhow::{bail, ensure};
use tokio::time::{Instant, MissedTickBehavior, interval, sleep};
use tracing::{Instrument, debug, info, info_span, warn};

use crate::accounts::Lifecycle;
use crate::chain::contracts::Vault;
use crate::chain::{Call, TxError};
use crate::keys::AccountKey;
use crate::node::CallError;

/// What a claim came to.
#[derive(Debug, PartialEq, Eq)]
pub enum Claimed {
    /// No checkpoint holds an entitlement for the account yet.
    Nothing,
    /// The newest entitlement waits for the vault to record its checkpoint; `unclaimed` MONKEY base
    /// units will be claimable then.
    Waiting { unclaimed: U256 },
    /// Less than the threshold is unclaimed, so nothing was sent.
    Below { unclaimed: U256 },
    /// The vault minted `amount` base units to the account.
    Minted { amount: U256, tx: B256 },
}

impl Lifecycle {
    /// Claims `key`'s account's MONKEY, paid for by the funding key when there is one, once at least
    /// `threshold` base units (and always at least one) are unclaimed.
    /// Error cases:
    /// - a node read does not verify or is refused, or names another account
    /// - a chain read fails, or the claim would revert, reverts, or gets no receipt
    /// - the receipt holds no mint to the account
    pub async fn claim(&self, key: &AccountKey, threshold: U256) -> anyhow::Result<Claimed> {
        let account = key.address();
        let span = info_span!("claim", id = %account);
        async {
            let proof = match self.node.emissions(account).await {
                Ok(answer) => answer.body,
                Err(CallError::Refused { status: 404, .. }) => return Ok(Claimed::Nothing),
                Err(e) => return Err(e.into()),
            };
            ensure!(proof.account == account, "the node's proof is for {}, not {account}", proof.account);
            self.tokens().await?;
            let wallet = Address::from(account.0);
            let claimed = self.chain()?.view(self.vault, Vault::emissionsClaimedCall { account: wallet }).await?;
            if !proof.claimable {
                let unclaimed = U256::from(proof.newest_entitled.0).saturating_sub(claimed);
                debug!(%unclaimed, seq = proof.seq, "the entitlement waits for the vault to record its checkpoint");
                return Ok(Claimed::Waiting { unclaimed });
            }
            let entitled = U256::from(proof.entitled.0);
            let unclaimed = entitled.saturating_sub(claimed);
            if unclaimed.is_zero() || unclaimed < threshold {
                debug!(%unclaimed, %threshold, "below the claim threshold");
                return Ok(Claimed::Below { unclaimed });
            }
            let call = Vault::claimEmissionsCall {
                account: wallet,
                entitled,
                checkpointSeq: proof.seq,
                siblings: proof.siblings.clone(),
            };
            let what = format!("claiming {unclaimed} MONKEY base units");
            let call = Call { to: self.vault, data: call.abi_encode().into(), what };
            let receipt = match self.chain()?.send(self.payer(key), &call).await {
                Ok(receipt) => receipt,
                // Claimed by another sender since the bot read it unclaimed, perhaps on an endpoint behind.
                Err(TxError::WouldRevert { revert: Some(data), .. } | TxError::Reverted { revert: Some(data), .. })
                    if claimed_already(&data) =>
                {
                    info!("the entitlement is claimed already");
                    return Ok(Claimed::Below { unclaimed: U256::ZERO });
                }
                Err(e) => return Err(e.into()),
            };
            let amount = receipt
                .logs()
                .iter()
                .filter(|log| log.address() == self.vault)
                .filter_map(|log| Vault::EmissionsClaimed::decode_log(&log.inner).ok())
                .find(|event| event.account == wallet)
                .map(|event| event.amount);
            let Some(amount) = amount else {
                bail!("{} holds no MONKEY claim for {account}", receipt.transaction_hash);
            };
            info!(%amount, tx = %receipt.transaction_hash, "MONKEY claimed");
            Ok(Claimed::Minted { amount, tx: receipt.transaction_hash })
        }
        .instrument(span)
        .await
    }
}

/// Whether revert data is the vault's refusal of a claim with nothing left to claim.
fn claimed_already(data: &[u8]) -> bool {
    matches!(Vault::VaultErrors::abi_decode(data), Ok(Vault::VaultErrors::NothingToClaim(_)))
}

/// How a claims pass walks its accounts. The node keeps the proofs of the 128 accounts asked about
/// most recently, so a pass asks `size` accounts at a time, well under that, and holds a batch while
/// any of its accounts waits for the vault to record a checkpoint, asking those again every `poll`
/// until `bound` passes: an account is not left to fall out of the node's watch set before it can
/// be claimed.
#[derive(Clone, Copy, Debug)]
pub struct Batching {
    pub size: usize,
    pub poll: Duration,
    pub bound: Duration,
}

impl Default for Batching {
    fn default() -> Self {
        Self { size: 64, poll: Duration::from_secs(10), bound: Duration::from_secs(300) }
    }
}

/// Every `every`, claims each of `accounts`' MONKEY once `threshold` base units are unclaimed, in
/// batches (see `Batching`). A failed claim is a warning, tried again at the next check, and so is
/// an account still waiting for the vault to record its checkpoint when its batch's bound passes. It
/// returns only when the node answers with another key's signature, which nothing the bot does may
/// follow; that error.
pub async fn watch(
    lifecycle: Arc<Lifecycle>,
    accounts: Vec<Arc<AccountKey>>,
    threshold: U256,
    every: Duration,
) -> anyhow::Error {
    watch_batched(lifecycle, accounts, threshold, every, Batching::default()).await
}

/// `watch` with the batches `batching` sets.
pub async fn watch_batched(
    lifecycle: Arc<Lifecycle>,
    accounts: Vec<Arc<AccountKey>>,
    threshold: U256,
    every: Duration,
    batching: Batching,
) -> anyhow::Error {
    let mut tick = interval(every);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut failing = vec![false; accounts.len()];
    let mut waiting = vec![false; accounts.len()];
    let size = batching.size.max(1);
    loop {
        tick.tick().await;
        for first in (0..accounts.len()).step_by(size) {
            let started = Instant::now();
            let mut pending: Vec<usize> = (first..accounts.len().min(first + size)).collect();
            while !pending.is_empty() {
                let mut again = Vec::new();
                for at in pending {
                    let account = accounts[at].address();
                    match lifecycle.claim(&accounts[at], threshold).await {
                        Ok(claimed) => {
                            if std::mem::take(&mut failing[at]) {
                                info!(alert_resolved = "monkey_claim_failed", account = %account, "the claim check succeeds again");
                            }
                            if matches!(claimed, Claimed::Waiting { .. }) {
                                again.push(at);
                            } else if std::mem::take(&mut waiting[at]) {
                                info!(alert_resolved = "monkey_claim_waiting", account = %account, "the vault records its checkpoint");
                            }
                        }
                        Err(e) if is_wrong_signer(&e) => return e,
                        Err(e) => {
                            if !std::mem::replace(&mut failing[at], true) {
                                warn!(alert = "monkey_claim_failed", account = %account, "MONKEY was not claimed; trying again in {every:?}: {e:#}");
                            } else {
                                debug!(account = %account, "MONKEY still not claimed: {e:#}");
                            }
                        }
                    }
                }
                pending = again;
                let left = batching.bound.saturating_sub(started.elapsed());
                if pending.is_empty() || left.is_zero() {
                    break;
                }
                sleep(batching.poll.min(left)).await;
            }
            for at in pending {
                if !std::mem::replace(&mut waiting[at], true) {
                    let account = accounts[at].address();
                    warn!(
                        alert = "monkey_claim_waiting", account = %account,
                        "its MONKEY waits for the vault to record a checkpoint; asking again in {every:?}"
                    );
                }
            }
        }
    }
}

/// Whether `error` is a node answer signed by another key than the signer.
pub(crate) fn is_wrong_signer(error: &anyhow::Error) -> bool {
    matches!(error.downcast_ref::<CallError>(), Some(CallError::WrongSigner { .. }))
}
