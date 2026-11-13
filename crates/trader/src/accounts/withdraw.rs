//! Withdrawals: a signed request to the node (`POST /v1/withdrawals`), which reserves the amount in
//! the public FIFO queue and approves it in parts as cash allows; each approved part becomes a
//! payout the enclave authorizes once a checkpoint holds it, and the vault's `withdraw` pays it to
//! the account, whoever sends it. An authorization that expires unpaid is replaced on the account's
//! signed request.

use std::collections::{BTreeMap, BTreeSet};

use alloy_primitives::{Address, B256};
use alloy_sol_types::{Eip712Domain, SolCall, SolInterface, SolStruct, sol};
use anyhow::{Context, anyhow, bail, ensure};
use engine_core::Fixed;
use engine_core::auth::RequestAuth;
use engine_core::command::WithdrawalRequest;
use engine_core::eip712::withdrawal_hash;
use engine_core::event::Event;
use engine_core::types::AccountId;
use tokio::time::{Instant, sleep};
use tracing::{Instrument, debug, info, info_span, warn};

use crate::accounts::{Lifecycle, REQUEST_EXPIRY_MS, retry_later, usdc_units};
use crate::chain::contracts::{Vault, revert_reason};
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
    /// the account to the account, each sent by the funding key when there is one: it pays each part
    /// the queue approves as its authorization comes, asks for a replacement of one that expired,
    /// and waits until the queue holds nothing more of the request. Without an amount it only pays
    /// what is already approved.
    /// Error cases:
    /// - the amount is not whole USDC base units above zero
    /// - the node refuses the request, gives no answer to it, or may have taken a request it asked
    ///   to have sent again
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
            let Some(amount) = amount else { return self.pay(key, None, deadline).await };
            let requested_seq = self.request(key, amount, deadline).await?;
            // Asking again would withdraw twice: what is queued is paid by a withdrawal without an amount.
            self.pay(key, Some(requested_seq), deadline).await.with_context(|| {
                format!("the withdrawal is queued as seq {requested_seq}: `withdraw` without --amount pays it")
            })
        }
        .instrument(span)
        .await
    }

    /// Requests a withdrawal of `amount`, asking again on a retry-later answer or a stale nonce
    /// until `deadline`; the seq the queue knows the request by. A retry-later answer does not say
    /// the node did not take the request, so it is asked again on the same nonce, which the node
    /// takes at most once: a stale nonce after it may mean it was taken.
    /// Error cases:
    /// - the amount is not whole USDC base units above zero
    /// - the node refuses it, in the log or outside it, or gives no answer to it
    /// - the node answers a stale nonce to a request asked again after a retry-later answer
    /// - it is still not taken at `deadline`
    async fn request(&self, key: &AccountKey, amount: Fixed, deadline: Instant) -> anyhow::Result<u64> {
        usdc_units(amount)?;
        let account = key.address();
        // The nonce of a request the node asked to have sent later, which it may yet have taken.
        let mut unsettled = None;
        loop {
            if Instant::now() >= deadline {
                bail!("the node did not take the withdrawal request within {:?}", self.settle_timeout);
            }
            let (next, now) = self.next_nonce(account).await?;
            let nonce = unsettled.unwrap_or(next);
            let expires_at = now.saturating_add(REQUEST_EXPIRY_MS);
            let hash = withdrawal_hash(&self.domain, account, amount, nonce, expires_at);
            let auth = RequestAuth { nonce, expires_at, signature: key.sign(&hash) };
            let request = WithdrawalRequest { account, amount, auth: Some(auth) };
            info!(%amount, nonce, expires_at, "requesting a withdrawal");
            let error = match self.node.withdraw(&request).await {
                Ok(answer) => return queued(&answer.body),
                Err(error) => error,
            };
            match (retry_later(&error), &error) {
                (Some(wait), _) => {
                    info!(?wait, "the node asks to request later: {error}");
                    unsettled = Some(nonce);
                    sleep(wait.min(deadline.saturating_duration_since(Instant::now()))).await;
                }
                (None, CallError::Refused { status: 409, .. }) if unsettled.is_some() => {
                    bail!(
                        "the node may have taken the withdrawal request it asked to have sent again (nonce {nonce} \
                         is spent): see `trader accounts status` before asking again"
                    );
                }
                (None, CallError::Refused { status: 409, .. }) => info!("the nonce moved on; signing again"),
                (None, CallError::Failed { .. } | CallError::Unverified { .. }) => {
                    return Err(anyhow::Error::from(error)).with_context(|| {
                        format!("the request may still enter the log until {expires_at}: see `trader accounts status` before asking again")
                    });
                }
                (None, _) => return Err(error.into()),
            }
        }
    }

    /// Pays each of the account's unpaid payouts once its authorization is signed and usable,
    /// asking for a replacement of one that expired, until none is left unpaid and the queue holds
    /// nothing more of request `requested_seq`, when there is one. A payout counts as paid only
    /// while the vault still shows its nonce used, read again at each poll and before the end, so a
    /// payment whose block is undone is paid again. A read that fails is read again
    /// at the next poll, and an authorization the vault refuses for now, out of the chain's time or
    /// of a signer epoch gone, is tried again on each poll until it is paid, expires or is replaced.
    /// Error cases:
    /// - an answer is signed by another key, or an authorization does not match the account, vault
    ///   and chain
    /// - a payment would revert, reverts, or gets no receipt, for another reason
    /// - the request is still queued, or a payout unpaid, after `deadline`
    async fn pay(
        &self,
        key: &AccountKey,
        mut requested_seq: Option<u64>,
        deadline: Instant,
    ) -> anyhow::Result<Vec<Paid>> {
        let account = key.address();
        let wallet = Address::from(account.0);
        let mut paid = Vec::new();
        let mut done = BTreeSet::new();
        let mut position = None;
        // Each expired authorization the bot asked to replace, and the polls since it asked; and each
        // the vault refused for now, by payout nonce and expiry, tried again on each poll.
        let mut reissued: BTreeMap<(u64, u64), u32> = BTreeMap::new();
        let mut refused = BTreeSet::new();
        let mut unread = false;
        loop {
            let read = async {
                // The queue is read before the payouts, so a part it no longer holds is among them.
                let queued = match requested_seq {
                    Some(seq) => Some(self.node.queued_withdrawal(account, seq).await?),
                    None => None,
                };
                Ok::<_, CallError>((queued, self.node.payouts(account).await?))
            };
            let (queued, answer) = match read.await {
                Ok(read) => {
                    if std::mem::take(&mut unread) {
                        info!("the withdrawal is read again");
                    }
                    read
                }
                Err(e @ CallError::WrongSigner { .. }) => return Err(e.into()),
                Err(e) => {
                    not_read(&mut unread, &e);
                    if Instant::now() >= deadline {
                        return Err(self.overdue(&paid, position.filter(|_| requested_seq.is_some())));
                    }
                    sleep(self.poll_interval).await;
                    continue;
                }
            };
            match (requested_seq, queued) {
                (Some(seq), Some(None)) => {
                    info!(requested_seq = seq, "the withdrawal is approved");
                    requested_seq = None;
                }
                (Some(_), Some(Some(mine))) if position != Some(mine.position) => {
                    info!(position = mine.position, remaining = %mine.remaining, "the withdrawal waits in the queue");
                    position = Some(mine.position);
                }
                _ => {}
            }
            // A payment counts only while the vault still holds it: one whose block was undone is paid
            // again, and none is reported paid on a read that cannot say.
            let mut verified = true;
            for nonce in done.iter().copied().collect::<Vec<_>>() {
                match self.chain()?.view(self.vault, Vault::usedNoncesCall { account: wallet, nonce }).await {
                    Ok(true) => {}
                    Ok(false) => {
                        warn!(nonce, "the vault no longer holds the payment; paying the payout again");
                        done.remove(&nonce);
                        paid.retain(|paid| paid.nonce != nonce);
                    }
                    Err(e) => {
                        not_read(&mut unread, &e);
                        verified = false;
                        break;
                    }
                }
            }
            if !verified {
                if Instant::now() >= deadline {
                    return Err(self.overdue(&paid, position.filter(|_| requested_seq.is_some())));
                }
                sleep(self.poll_interval).await;
                continue;
            }
            let now_s = answer.time_ms / 1000;
            let unpaid: Vec<&Payout> = answer.payouts.iter().filter(|payout| !done.contains(&payout.nonce)).collect();
            if unpaid.is_empty() && answer.loaded && requested_seq.is_none() {
                return Ok(paid);
            }
            for payout in unpaid {
                let nonce = payout.nonce;
                let used = match self.chain()?.view(self.vault, Vault::usedNoncesCall { account: wallet, nonce }).await
                {
                    Ok(used) => used,
                    Err(e) => {
                        not_read(&mut unread, &e);
                        break;
                    }
                };
                if used {
                    info!(nonce, "the payout is paid already");
                    done.insert(nonce);
                    continue;
                }
                let Some(signed) = &payout.withdrawal else { continue };
                let call = self.withdraw_call(account, payout, signed)?;
                let expires_at = signed.authorization.as_ref().expect("checked by withdraw_call").expires_at;
                if expires_at <= now_s {
                    let polls = reissued.entry((nonce, expires_at)).or_insert(REISSUE_AGAIN_POLLS);
                    if *polls >= REISSUE_AGAIN_POLLS {
                        self.reissue(key, nonce, expires_at).await?;
                        *polls = 0;
                    }
                    *polls += 1;
                    continue;
                }
                if expires_at <= now_s.saturating_add(EXPIRY_MARGIN_S) {
                    continue;
                }
                let what = format!("paying withdrawal {nonce} of {} USDC", payout.amount);
                let call = Call { to: self.vault, data: call.abi_encode().into(), what };
                match self.chain()?.send(self.payer(key), &call).await {
                    Ok(receipt) => {
                        info!(nonce, amount = %payout.amount, tx = %receipt.transaction_hash, "withdrawal paid");
                        paid.push(Paid { nonce, amount: payout.amount, tx: receipt.transaction_hash });
                    }
                    Err(
                        TxError::WouldRevert { revert: Some(data), .. } | TxError::Reverted { revert: Some(data), .. },
                    ) if already_paid(&data) => {
                        // Paid by another sender since the bot read it unpaid, perhaps on an endpoint behind.
                        info!(nonce, "the payout is paid already");
                    }
                    Err(
                        TxError::WouldRevert { revert: Some(data), .. } | TxError::Reverted { revert: Some(data), .. },
                    ) if refused_for_now(&data) => {
                        // Of a signer epoch gone, which the node replaces on the signer's change; or out of
                        // the chain's time, which it may yet reach, or pass, and the bot asks for another.
                        if refused.insert((nonce, expires_at)) {
                            warn!(
                                nonce,
                                expires_at,
                                "the vault refuses the authorization for now: {}",
                                revert_reason(&data)
                            );
                        }
                        continue;
                    }
                    Err(e) => return Err(e.into()),
                }
                done.insert(nonce);
            }
            if Instant::now() >= deadline {
                return Err(self.overdue(&paid, position.filter(|_| requested_seq.is_some())));
            }
            sleep(self.poll_interval).await;
        }
    }

    /// The error of a withdrawal still waiting at its deadline: queued at `position`, or with
    /// payouts unpaid, and naming what `paid` paid.
    fn overdue(&self, paid: &[Paid], position: Option<u64>) -> anyhow::Error {
        let paid: Vec<String> = paid.iter().map(|paid| format!("{} USDC in {}", paid.amount, paid.tx)).collect();
        let paid = if paid.is_empty() { String::new() } else { format!(" (paid {})", paid.join(", ")) };
        match position {
            Some(position) => {
                let wait = self.settle_timeout;
                anyhow!(
                    "the withdrawal is still queued at position {position} after {wait:?}{paid}: see `trader accounts status`"
                )
            }
            None => anyhow!(
                "withdrawals are still unpaid after {:?}{paid}: see `trader accounts status`",
                self.settle_timeout
            ),
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
    /// A request the node did not answer, or whose answer does not verify, is left to be asked again
    /// after `REISSUE_AGAIN_POLLS` polls, as a refusal a wait cures is.
    /// Error cases:
    /// - the node refuses it for a reason a wait does not cure, such as a signature it does not take
    /// - an answer is signed by another key
    async fn reissue(&self, key: &AccountKey, nonce: u64, expires_at: u64) -> anyhow::Result<()> {
        let account = key.address();
        let hash = reissue_hash(&self.domain, account, nonce, expires_at);
        let request = ReissueBody { replaces_expires_at: expires_at, signature: key.sign(&hash) };
        info!(nonce, expires_at, "the authorization expired unpaid; asking for another");
        match self.node.reissue(account, nonce, &request).await {
            Ok(_) => Ok(()),
            // Already replaced, not yet expired by the node's clock, or not answered: the payouts show
            // which, and the bot asks again later if they still show it expired.
            Err(
                e @ (CallError::Refused { status: 400, .. }
                | CallError::RetryLater { .. }
                | CallError::Failed { .. }
                | CallError::Unverified { .. }),
            ) => {
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

/// Logs that a read failed: once as a warning, then at debug until a read works again.
fn not_read(unread: &mut bool, error: &dyn std::fmt::Display) {
    if std::mem::replace(unread, true) {
        debug!("the withdrawal is still not read: {error}");
    } else {
        warn!("the withdrawal was not read; reading it again: {error}");
    }
}

/// Whether revert data is the vault's refusal of an authorization that time or a replacement cures:
/// out of the chain's time (issued after its latest block, or expired), or of a signer epoch gone.
fn refused_for_now(data: &[u8]) -> bool {
    matches!(
        Vault::VaultErrors::abi_decode(data),
        Ok(Vault::VaultErrors::InvalidWithdrawalTime(_) | Vault::VaultErrors::InvalidSignerEpoch(_))
    )
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
