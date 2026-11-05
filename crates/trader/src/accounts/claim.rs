//! MONKEY claims: the account's entitlement and its proof from the node, claimed with the vault's
//! `claimEmissions`, which mints to the account whoever sends it. On demand, or while the bot runs,
//! once the unclaimed amount reaches a threshold.

use std::sync::Arc;
use std::time::Duration;

use alloy_primitives::{Address, B256, U256};
use alloy_sol_types::{SolCall, SolEvent};
use anyhow::{bail, ensure};
use tokio::time::{MissedTickBehavior, interval};
use tracing::{Instrument, debug, info, info_span, warn};

use crate::accounts::Lifecycle;
use crate::chain::Call;
use crate::chain::contracts::Vault;
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
            let claimed = self.chain.view(self.vault, Vault::emissionsClaimedCall { account: wallet }).await?;
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
            let receipt = self.chain.send(self.payer(key), &call).await?;
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

/// Every `every`, claims each of `accounts`' MONKEY once `threshold` base units are unclaimed. A
/// failed claim is a warning, tried again at the next check. It returns only when the node answers
/// with another key's signature, which nothing the bot does may follow; that error.
pub async fn watch(
    lifecycle: Arc<Lifecycle>,
    accounts: Vec<Arc<AccountKey>>,
    threshold: U256,
    every: Duration,
) -> anyhow::Error {
    let mut tick = interval(every);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut failing = vec![false; accounts.len()];
    loop {
        tick.tick().await;
        for (key, failing) in accounts.iter().zip(failing.iter_mut()) {
            let account = key.address();
            match lifecycle.claim(key, threshold).await {
                Ok(_) => {
                    if std::mem::take(failing) {
                        info!(alert_resolved = "monkey_claim_failed", account = %account, "the claim check succeeds again");
                    }
                }
                Err(e) if is_wrong_signer(&e) => return e,
                Err(e) => {
                    if !std::mem::replace(failing, true) {
                        warn!(alert = "monkey_claim_failed", account = %account, "MONKEY was not claimed; trying again in {every:?}: {e:#}");
                    } else {
                        debug!(account = %account, "MONKEY still not claimed: {e:#}");
                    }
                }
            }
        }
    }
}

/// Whether `error` is a node answer signed by another key than the signer.
pub(crate) fn is_wrong_signer(error: &anyhow::Error) -> bool {
    matches!(error.downcast_ref::<CallError>(), Some(CallError::WrongSigner { .. }))
}
