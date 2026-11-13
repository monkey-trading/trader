//! An account's life outside trading: its declaration, its USDC deposits, its MONKEY claims and its
//! withdrawals. Each action takes the account's key from the caller, which owns it, and acts for
//! that one account. Every payout goes to the account's own address: the vault pays a claim or a
//! withdrawal to the account it names, and the bot names only the account whose key it holds. The
//! optional funding key pays the gas of claims and withdrawals and funds deposits with
//! `depositFor`; it never receives anything.

pub mod claim;
pub mod cli;
pub mod declare;
pub mod deposit;
pub mod withdraw;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use alloy_primitives::{Address, U256};
use anyhow::{Context, ensure};
use engine_core::access::EligibilityClass;
use engine_core::eip712::{self, Eip712Domain};
use engine_core::types::{AccountId, Millis};
use engine_core::{Fixed, fixed};
use engine_wire::Deployment;
use tokio::sync::OnceCell;
use tracing::info;

use crate::chain::Chain;
use crate::chain::contracts::{ERC20, Vault};
use crate::config::{Config, DEFAULT_POLL_INTERVAL, DEFAULT_SETTLE_TIMEOUT};
use crate::keys::AccountKey;
use crate::node::accounts::{Payout, QueuedWithdrawal};
use crate::node::attestation::attest_node;
use crate::node::{CallError, Node};

/// USDC's decimals.
pub const USDC_DECIMALS: u8 = 6;
/// One USDC base unit in `Fixed`'s raw units.
const USDC_UNIT_RAW: i128 = 10i128.pow(fixed::DECIMALS - USDC_DECIMALS as u32);
/// How long after the node's time a signed request may still be sequenced.
const REQUEST_EXPIRY_MS: Millis = 60_000;

/// What the lifecycle actions need: the node, the chain, the vault and its tokens, and the funding
/// key when one is configured.
pub struct Lifecycle {
    node: Arc<Node>,
    /// The chain `[chain]` names, which no action asks before it needs it, so the node's alone go on
    /// without it, or without the section.
    chain: Option<Chain>,
    tokens: OnceCell<Tokens>,
    domain: Eip712Domain,
    chain_id: u64,
    vault: Address,
    funding: Option<AccountKey>,
    /// One deposit at a time pays from each address, from its allowance check to its deposit's
    /// sending: the deposits of one payer share one allowance, which each sets for itself. A payer is
    /// an account's key or the funding key, so the map holds at most one entry per key.
    deposit_turns: Mutex<BTreeMap<Address, Arc<tokio::sync::Mutex<()>>>>,
    settle_timeout: Duration,
    poll_interval: Duration,
}

/// The vault's tokens, read once the chain is checked.
#[derive(Clone, Copy, Debug)]
struct Tokens {
    usdc: Address,
    monkey: Address,
}

/// What an account holds and owes, on chain and in the node.
#[derive(Debug)]
pub struct AccountStatus {
    pub account: AccountId,
    /// The declaration the account recorded, if it has.
    pub declared: Option<EligibilityClass>,
    /// The account's wallet: ether in wei, USDC in base units, MONKEY in base units.
    pub wallet_eth: U256,
    pub wallet_usdc: U256,
    pub wallet_monkey: U256,
    /// The account's balance in the node; `None` before its first credit.
    pub balance: Option<Fixed>,
    /// MONKEY the vault would mint now, and what the newest checkpoint will add, in base units.
    pub unclaimed: U256,
    pub unclaimed_newest: U256,
    /// Withdrawals waiting in the queue, and approved ones the vault has not paid. While the node
    /// is still loading payouts (`payouts_loaded` is false), `unpaid` may miss some.
    pub queued: Vec<QueuedWithdrawal>,
    pub unpaid: Vec<Payout>,
    pub payouts_loaded: bool,
}

impl Lifecycle {
    /// The lifecycle of `cfg`'s accounts: a node client of its own, the chain `[chain]` names, and
    /// the funding key (see `with_node`).
    /// Error cases:
    /// - the node's attestation does not verify (see `attest_node`)
    /// - see `with_node`
    pub async fn connect(cfg: &Config, env: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let node_cfg = &cfg.node;
        let attested = attest_node(node_cfg).await?;
        let deployment = Deployment { chain_id: node_cfg.chain_id, vault: node_cfg.vault.0 };
        let node = Node::new(attested.tls, attested.signer, deployment, node_cfg.request_timeout, &node_cfg.limits);
        Self::with_node(cfg, Arc::new(node), env).await
    }

    /// The lifecycle of `cfg`'s accounts on `node`, the chain `[chain]` names if any, and the funding
    /// key, unlocked with `env` when its password is in a variable. The chain is asked nothing yet.
    /// Error cases:
    /// - the chain's client cannot be built (see `Chain::new`)
    /// - the funding key does not load (see `AccountKey::load`)
    pub async fn with_node(
        cfg: &Config,
        node: Arc<Node>,
        env: impl Fn(&str) -> Option<String>,
    ) -> anyhow::Result<Self> {
        let node_cfg = &cfg.node;
        let chain = cfg.chain.as_ref().map(|chain| Chain::new(chain, node_cfg.chain_id)).transpose()?;
        let (settle_timeout, poll_interval) =
            cfg.chain.as_ref().map_or((DEFAULT_SETTLE_TIMEOUT, DEFAULT_POLL_INTERVAL), |chain| {
                (chain.settle_timeout, chain.poll_interval)
            });
        let funding = match &cfg.funding {
            Some(funding) => Some(AccountKey::load(&funding.key, env).await.context("the funding key")?),
            None => None,
        };
        if let Some(funding) = &funding {
            info!(funding = %funding.address(), "funding key unlocked");
        }
        Ok(Self {
            node,
            chain,
            tokens: OnceCell::new(),
            domain: eip712::domain(node_cfg.chain_id, node_cfg.vault),
            chain_id: node_cfg.chain_id,
            vault: Address::from(node_cfg.vault.0),
            funding,
            deposit_turns: Mutex::default(),
            settle_timeout,
            poll_interval,
        })
    }

    /// Whether a funding key is configured, which `Source::Funding` and the gas of claims and
    /// withdrawals need.
    pub fn has_funding(&self) -> bool {
        self.funding.is_some()
    }

    /// What `account` holds and owes.
    /// Error cases:
    /// - a chain read fails, or a node read does not verify or is refused
    pub async fn status(&self, account: AccountId) -> anyhow::Result<AccountStatus> {
        let tokens = self.tokens().await?;
        let wallet = Address::from(account.0);
        let declared = self.declared(account).await?;
        let balance = match self.node.account(account).await {
            Ok(view) => Some(view.body.balance),
            Err(CallError::Refused { status: 404, .. }) => None,
            Err(e) => return Err(e.into()),
        };
        let claimed = self.chain()?.view(self.vault, Vault::emissionsClaimedCall { account: wallet }).await?;
        let (unclaimed, unclaimed_newest) = match self.node.emissions(account).await {
            Ok(view) => {
                let view = view.body;
                let now = if view.claimable { U256::from(view.entitled.0) } else { U256::ZERO };
                (now.saturating_sub(claimed), U256::from(view.newest_entitled.0).saturating_sub(claimed))
            }
            Err(CallError::Refused { status: 404, .. }) => (U256::ZERO, U256::ZERO),
            Err(e) => return Err(e.into()),
        };
        let (unpaid, payouts_loaded) = self.unpaid(account).await?;
        Ok(AccountStatus {
            account,
            declared,
            wallet_eth: self.chain()?.balance(wallet).await?,
            wallet_usdc: self.chain()?.view(tokens.usdc, ERC20::balanceOfCall { account: wallet }).await?,
            wallet_monkey: self.chain()?.view(tokens.monkey, ERC20::balanceOfCall { account: wallet }).await?,
            balance,
            unclaimed,
            unclaimed_newest,
            queued: self.node.withdrawal_queue(account).await?,
            unpaid,
            payouts_loaded,
        })
    }

    /// `account`'s payouts the node lists as unpaid, but for those the vault has paid since, and
    /// whether the node had loaded them all.
    /// Error cases:
    /// - a node or chain read fails
    async fn unpaid(&self, account: AccountId) -> anyhow::Result<(Vec<Payout>, bool)> {
        let wallet = Address::from(account.0);
        let payouts = self.node.payouts(account).await?;
        let mut unpaid = Vec::new();
        for payout in payouts.payouts {
            let nonce = payout.nonce;
            if !self.chain()?.view(self.vault, Vault::usedNoncesCall { account: wallet, nonce }).await? {
                unpaid.push(payout);
            }
        }
        Ok((unpaid, payouts.loaded))
    }

    /// The class `account` declared, if it has.
    /// Error cases:
    /// - the read does not verify or is refused
    pub async fn declared(&self, account: AccountId) -> anyhow::Result<Option<EligibilityClass>> {
        Ok(self.node.eligibility(account).await?.body.class)
    }

    /// The vault's tokens, once the chain's endpoints are checked to be on the vault's chain; the
    /// first call checks them and reads the tokens.
    /// Error cases:
    /// - the chain cannot be reached, or is another chain than the node's
    /// - the vault's token addresses cannot be read
    async fn tokens(&self) -> anyhow::Result<Tokens> {
        let tokens = self.tokens.get_or_try_init(|| async {
            self.chain()?.check().await?;
            let usdc = self.chain()?.view(self.vault, Vault::usdcCall {}).await.context("the vault's USDC")?;
            let monkey = self.chain()?.view(self.vault, Vault::monkeyCall {}).await.context("the vault's MONKEY")?;
            anyhow::Ok(Tokens { usdc, monkey })
        });
        Ok(*tokens.await?)
    }

    /// The chain, for an action that reads it.
    /// Error cases:
    /// - the config has no `[chain]` section
    pub(crate) fn chain(&self) -> anyhow::Result<&Chain> {
        self.chain.as_ref().context("the config has no [chain] section, which this action needs")
    }

    /// The key that pays for `account`'s transactions: the funding key when there is one.
    fn payer<'a>(&'a self, key: &'a AccountKey) -> &'a AccountKey {
        self.funding.as_ref().unwrap_or(key)
    }

    /// `account`'s next request nonce, and the node's time when it gave it.
    /// Error cases:
    /// - the read does not verify or is refused, or the node's time is out of range
    async fn next_nonce(&self, account: AccountId) -> anyhow::Result<(u64, Millis)> {
        let answer = self.node.nonce(account).await?;
        Ok((answer.body.next_nonce, Millis::try_from(answer.time_ms).context("the node's time")?))
    }
}

/// How long to wait before asking the node again, when its answer asks the bot to try again later,
/// as the fresh-account budgets' 429 and a full screening's 503 do: the delay it names.
fn retry_later(error: &CallError) -> Option<Duration> {
    match error {
        CallError::RetryLater { wait, .. } => Some(*wait),
        _ => None,
    }
}

/// `amount` of USDC in base units.
/// Error cases:
/// - not above zero, past the engine's input bound, or finer than a USDC base unit
pub fn usdc_units(amount: Fixed) -> anyhow::Result<U256> {
    ensure!(amount > Fixed::ZERO && amount.is_valid_input(), "{amount} USDC is not above zero and within bounds");
    ensure!(amount.raw() % USDC_UNIT_RAW == 0, "{amount} USDC has more than six decimals");
    let units = u128::try_from(amount.raw() / USDC_UNIT_RAW).expect("a positive amount");
    Ok(U256::from(units))
}
