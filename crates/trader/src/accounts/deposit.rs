//! USDC into an account: the payer approves the vault for the amount and deposits it, from the
//! account's own wallet with `deposit` or from the funding key's with `depositFor`, and the bot waits
//! for the node to credit it. A deposit is sent again only once the vault holds none of the
//! transactions signed for it, at a block by which another transaction took the last one's nonce:
//! it never goes in twice, whatever the endpoints' receipts say.

use alloy_primitives::{Address, B256, U256};
use alloy_rpc_types_eth::{Filter, TransactionReceipt};
use alloy_sol_types::{SolCall, SolEvent};
use anyhow::{Context, bail, ensure};
use engine_core::Fixed;
use engine_core::access::AccessReject;
use engine_core::types::AccountId;
use futures_util::future::BoxFuture;
use tokio::time::{Instant, sleep};
use tracing::{Instrument, debug, info, info_span, warn};

use crate::accounts::{Lifecycle, usdc_units};
use crate::chain::contracts::{ERC20, Vault};
use crate::chain::{Call, Sent, Spent};
use crate::keys::AccountKey;
use crate::node::CallError;
use crate::node::accounts::DepositRecord;

/// Whose wallet a deposit's USDC comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The account's own, which sends `deposit`.
    Account,
    /// The funding key's, which sends `depositFor` the account.
    Funding,
}

/// What a deposit came to.
#[derive(Debug, PartialEq, Eq)]
pub enum Deposited {
    /// The node credited it to the account's balance.
    Credited { tx: B256 },
    /// The node holds it uncredited until the account declares, then credits it.
    AwaitingDeclaration { tx: B256 },
}

impl Lifecycle {
    /// Deposits `amount` USDC into `key`'s account from `source`'s wallet, which pays the gas, and
    /// waits for the node's credit.
    /// Error cases:
    /// - the amount is not whole USDC base units above zero
    /// - the source is the funding key and the config has none
    /// - the payer's wallet holds less USDC than the amount
    /// - a transaction would revert, reverts, or gets no receipt (see `TxError`)
    /// - the receipt has no deposit of the amount to the account
    /// - the node holds the deposit for good, or does not credit it within `settle_timeout`
    pub async fn deposit(&self, key: &AccountKey, amount: Fixed, source: Source) -> anyhow::Result<Deposited> {
        let account = key.address();
        let span = info_span!("deposit", id = %account);
        async {
            let units = usdc_units(amount)?;
            let usdc = self.tokens().await?.usdc;
            let payer = match source {
                Source::Account => key,
                Source::Funding => self.funding.as_ref().context("the config has no [funding] key")?,
            };
            let from = Address::from(payer.address().0);
            // The funding key's deposits share one allowance, so each sets and spends it alone.
            let funding_turn = match source {
                Source::Funding => Some(self.funding_deposit.lock().await),
                Source::Account => None,
            };
            let held = self.chain()?.view(usdc, ERC20::balanceOfCall { account: from }).await?;
            ensure!(held >= units, "{from} holds {held} USDC base units, less than the {units} to deposit");
            let allowance = self.chain()?.view(usdc, ERC20::allowanceCall { owner: from, spender: self.vault }).await?;
            if allowance < units {
                let approve = ERC20::approveCall { spender: self.vault, value: units };
                let call =
                    Call { to: usdc, data: approve.abi_encode().into(), what: format!("approving {amount} USDC") };
                self.chain()?.send(payer, &call).await?;
            }
            let wallet = Address::from(account.0);
            let data = match source {
                Source::Account => Vault::depositCall { amount: units }.abi_encode(),
                Source::Funding => Vault::depositForCall { account: wallet, amount: units }.abi_encode(),
            };
            let call = Call { to: self.vault, data: data.into(), what: format!("depositing {amount} USDC") };
            // Where the vault's deposits stand before this one is signed: what the guard looks through.
            let chain = self.chain()?;
            let since = (chain.view(self.vault, Vault::depositCountCall {}).await?, chain.block_number().await?);
            let guard = |spent, signed| -> BoxFuture<'_, anyhow::Result<Option<B256>>> {
                Box::pin(self.landed_deposit(since, spent, (wallet, from, units), signed))
            };
            let tx = match chain.send_guarded(payer, &call, &guard).await? {
                Sent::Mined(receipt) => {
                    self.deposit_event(&receipt, wallet, units)?;
                    receipt.transaction_hash
                }
                Sent::Landed(tx) => tx,
            };
            drop(funding_turn);
            self.credit(account, tx).await
        }
        .instrument(span)
        .await
    }

    /// Which of the transactions `signed` landed the deposit `deposit` names (account, payer and
    /// amount), by the vault's state at the block `spent`, read by its hash: among the deposits from
    /// the vault's deposit count and block `since`, one from the payer of the amount whose event names
    /// the account and one of those transactions. `None` when the vault holds none from the payer of
    /// the amount, or each it holds is another's by its event.
    /// Error cases:
    /// - a read fails, or a deposit that may be one of them has no event to say whose it is
    async fn landed_deposit(
        &self,
        since: (U256, u64),
        spent: Spent,
        deposit: (Address, Address, U256),
        signed: Vec<B256>,
    ) -> anyhow::Result<Option<B256>> {
        let ((first, from_block), (wallet, payer, units)) = (since, deposit);
        let chain = self.chain()?;
        let count = chain.view_at(self.vault, Vault::depositCountCall {}, spent.hash).await?;
        let mut id = first;
        while id < count {
            let record = chain.view_at(self.vault, Vault::depositRecordsCall { id }, spent.hash).await?;
            if record.payer == payer && U256::from(record.amount) == units {
                let filter = Filter::new()
                    .address(self.vault)
                    .event_signature(Vault::Deposited::SIGNATURE_HASH)
                    .topic1(B256::from(id))
                    .from_block(from_block)
                    .to_block(spent.number);
                let logs = chain.logs(&filter).await?;
                let log = logs.first().with_context(|| format!("deposit {id}'s event was not read"))?;
                let event =
                    Vault::Deposited::decode_log(&log.inner).with_context(|| format!("deposit {id}'s event"))?;
                let tx = log.transaction_hash.with_context(|| format!("deposit {id}'s event has no transaction"))?;
                if event.account == wallet && signed.contains(&tx) {
                    return Ok(Some(tx));
                }
            }
            id += U256::from(1);
        }
        Ok(None)
    }

    /// Checks `receipt` holds the vault's deposit of `units` to `account`.
    /// Error cases:
    /// - no `Deposited` event of the vault names that account and amount
    fn deposit_event(&self, receipt: &TransactionReceipt, account: Address, units: U256) -> anyhow::Result<()> {
        let deposited = receipt
            .logs()
            .iter()
            .filter(|log| log.address() == self.vault)
            .filter_map(|log| Vault::Deposited::decode_log(&log.inner).ok())
            .any(|event| event.account == account && event.amount == units);
        ensure!(deposited, "{} holds no deposit of {units} USDC base units to {account}", receipt.transaction_hash);
        Ok(())
    }

    /// Waits for the node's custody record of deposit transaction `tx` to `account`: credited, or
    /// held until the account declares.
    /// Error cases:
    /// - the node holds it for good (a confirmed sanction), or neither credits nor holds it within
    ///   `settle_timeout`
    /// - an answer is signed by another key
    async fn credit(&self, account: AccountId, tx: B256) -> anyhow::Result<Deposited> {
        let deadline = Instant::now() + self.settle_timeout;
        let (mut waiting, mut unread) = (None, false);
        loop {
            let state = match self.node.deposit_records(account).await {
                Ok(records) => {
                    if std::mem::take(&mut unread) {
                        info!("the account's deposits are read again");
                    }
                    credit_state(&records, tx)
                }
                Err(e @ CallError::WrongSigner { .. }) => return Err(e.into()),
                Err(e) => {
                    if std::mem::replace(&mut unread, true) {
                        debug!("the account's deposits are still not read: {e}");
                    } else {
                        warn!("the account's deposits were not read: {e}");
                    }
                    None
                }
            };
            match state {
                Some(Ok(())) => {
                    info!(%tx, "deposit credited");
                    return Ok(Deposited::Credited { tx });
                }
                Some(Err(AccessReject::EligibilityRequired)) => {
                    info!(%tx, "the deposit waits for the account's declaration");
                    return Ok(Deposited::AwaitingDeclaration { tx });
                }
                Some(Err(reason)) if reason.holds_deposit() => {
                    bail!("the node holds deposit {tx} uncredited for good: {reason:?}")
                }
                Some(Err(reason)) if waiting != Some(reason) => {
                    info!(%tx, ?reason, "the deposit waits uncredited; the node screens it again");
                    waiting = Some(reason);
                }
                _ => {}
            }
            if Instant::now() >= deadline {
                return Err(anyhow::anyhow!("the node did not credit deposit {tx} within {:?}", self.settle_timeout))
                    .context("it may credit later: see `trader accounts status`");
            }
            sleep(self.poll_interval).await;
        }
    }
}

/// Where the node's records leave deposit transaction `tx`: `Ok` while the principal its records
/// moved in, less what a reorganisation took out, is above zero; the newest refusal while it holds
/// it uncredited; `None` before it records it, or once a reorganisation took it out.
fn credit_state(records: &[DepositRecord], tx: B256) -> Option<Result<(), AccessReject>> {
    // Records are newest first.
    let mine: Vec<&DepositRecord> = records.iter().filter(|record| record.receipt.tx_hash == tx).collect();
    let principal = mine.iter().fold(0i128, |sum, record| sum.saturating_add(record.principal_delta.raw()));
    if principal > 0 {
        return Some(Ok(()));
    }
    mine.first().and_then(|record| record.uncredited).map(Err)
}

#[cfg(test)]
mod tests {
    use engine_core::deposits::{DepositBeneficiary, DepositReceipt};

    use super::*;

    fn record(tx: u8, delta: i64, uncredited: Option<AccessReject>) -> DepositRecord {
        DepositRecord {
            receipt: DepositReceipt {
                block: 1,
                block_hash: B256::ZERO,
                tx_hash: B256::repeat_byte(tx),
                log_index: 0,
                transaction_deposit_index: 0,
                deposit_id: 0,
                beneficiary: DepositBeneficiary::Trader { account: AccountId([1; 20]) },
                amount: Fixed::from_int(100),
            },
            principal_delta: Fixed::from_int(delta),
            uncredited,
        }
    }

    /// A credit counts while the transaction's records moved more principal in than a
    /// reorganisation took out; until then the newest refusal says why it waits; another
    /// transaction's records count for nothing.
    #[test]
    fn the_credit_state_follows_the_transactions_records() {
        let tx = B256::repeat_byte(7);
        assert_eq!(credit_state(&[], tx), None);
        assert_eq!(credit_state(&[record(8, 100, None)], tx), None);
        let pending = record(7, 0, Some(AccessReject::EligibilityRequired));
        assert_eq!(credit_state(&[pending], tx), Some(Err(AccessReject::EligibilityRequired)));
        let newest_first = [
            record(7, 0, Some(AccessReject::SourceUnavailable)),
            record(7, 0, Some(AccessReject::EligibilityRequired)),
        ];
        assert_eq!(credit_state(&newest_first, tx), Some(Err(AccessReject::SourceUnavailable)));
        let credited =
            [record(7, 0, None), record(7, 100, None), record(7, 0, Some(AccessReject::EligibilityRequired))];
        assert_eq!(credit_state(&credited, tx), Some(Ok(())));
        // A finalisation records no principal; alone it is not the credit.
        assert_eq!(credit_state(&[record(7, 0, None)], tx), None);
        // A reorganisation that took the credit out leaves it uncredited, until it is credited again.
        let reversed = [record(7, -100, None), record(7, 100, None)];
        assert_eq!(credit_state(&reversed, tx), None);
        let reinstated = [record(7, 100, None), record(7, -100, None), record(7, 100, None)];
        assert_eq!(credit_state(&reinstated, tx), Some(Ok(())));
    }
}
