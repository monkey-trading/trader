//! The custody chain: reads of the vault and the tokens, and the transactions the bot sends them.
//! A read tries each configured endpoint in turn, only one that has answered that it is on the
//! vault's chain, and once a transaction of the bot's has landed, only one that has reached its
//! block: a read after a write sees the write. An error answer other than a revert moves on to the
//! next endpoint, as a failure to answer does. A transaction
//! is estimated first and not sent when it would revert; then it is signed with the key that pays its
//! gas, sent to every endpoint, and followed to its receipt (`send`), and one that reverted is
//! reported with the contract's reason.
//! Endpoint URLs may carry credentials, so logs and errors name an endpoint by its place in the
//! config, and no transport error is printed as it is.

pub mod contracts;
mod send;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use alloy_primitives::{Address, B256, Bytes, TxKind, U256};
use alloy_provider::transport::{RpcError, TransportError, TransportErrorKind};
use alloy_provider::{Provider, RootProvider};
use alloy_rpc_client::RpcClient;
use alloy_rpc_types_eth::{TransactionInput, TransactionRequest};
use alloy_sol_types::SolCall;
use anyhow::{Context, bail, ensure};
use rustls::RootCertStore;
use rustls::crypto::ring::default_provider;
use tokio::time::{Instant, sleep};
use tracing::{debug, warn};

use crate::config::ChainSection;

/// The chain's endpoints and the waits around a transaction.
pub struct Chain {
    endpoints: Vec<RootProvider>,
    /// The chain id each endpoint answered, or 0 before it has.
    chains: Vec<AtomicU64>,
    chain_id: u64,
    receipt_timeout: Duration,
    poll_interval: Duration,
    max_fee: u128,
    /// The highest block a transaction of the bot's landed in, which a read's endpoint must reach.
    floor: AtomicU64,
}

/// One contract call the bot sends: the contract, the calldata, and what it does, for logs.
#[derive(Clone, Debug)]
pub struct Call {
    pub to: Address,
    pub data: Bytes,
    pub what: String,
}

/// Why a read has no answer.
#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    /// An endpoint answered with an error, such as a revert, and its revert data when it gave some.
    #[error("{message} (code {code})")]
    Answer { code: i64, message: String, revert: Option<Bytes> },
    /// No endpoint answered; the last one's failure, without its URL.
    #[error("no endpoint answered: {0}")]
    Unanswered(String),
    /// An answer that is not what the call returns.
    #[error("an answer that does not decode: {0}")]
    Undecodable(String),
}

/// Why a transaction did not land.
#[derive(Debug, thiserror::Error)]
pub enum TxError {
    /// The call reverts as the chain stands, so it was not sent; with the revert's data when the
    /// endpoint gave it.
    #[error("{what} would revert: {reason}")]
    WouldRevert { what: String, reason: String, revert: Option<Bytes> },
    /// It was not sent: the endpoints refused it, or a read it needs failed.
    #[error("{what} was not sent: {reason}")]
    NotSent { what: String, reason: String },
    /// It was mined and reverted; with the revert's data when its replay gave it.
    #[error("{what} reverted in {hash}: {reason}")]
    Reverted { what: String, hash: B256, reason: String, revert: Option<Bytes> },
    /// It was sent, but no receipt came within the wait; it may still be mined.
    #[error("{what}: no receipt for {hash} within {wait:?}; it may still be mined")]
    NoReceipt { what: String, hash: B256, wait: Duration },
}

impl Chain {
    /// The chain `section` names, whose endpoints must be on `chain_id`, checked.
    /// Error cases:
    /// - see `new` and `check`
    pub async fn connect(section: &ChainSection, chain_id: u64) -> anyhow::Result<Self> {
        let chain = Self::new(section, chain_id)?;
        chain.check().await?;
        Ok(chain)
    }

    /// The chain `section` names, whose endpoints must be on `chain_id`, before any is asked.
    /// Error cases:
    /// - the HTTP client cannot be built
    pub fn new(section: &ChainSection, chain_id: u64) -> anyhow::Result<Self> {
        let tls = rustls::ClientConfig::builder_with_provider(Arc::new(default_provider()))
            .with_safe_default_protocol_versions()
            .context("TLS")?
            .with_root_certificates(RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned()))
            .with_no_client_auth();
        let http = reqwest::Client::builder()
            .tls_backend_preconfigured(tls)
            .timeout(section.rpc_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("the RPC client")?;
        let endpoints = section
            .rpc
            .iter()
            .enumerate()
            .map(|(index, url)| {
                // The config load parsed it; the error never quotes it.
                let url = reqwest::Url::parse(url.expose()).ok().with_context(|| format!("chain.rpc[{index}]"))?;
                anyhow::Ok(RootProvider::new(RpcClient::new_http_with_client(http.clone(), url)))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(Self {
            chains: endpoints.iter().map(|_| AtomicU64::new(0)).collect(),
            endpoints,
            chain_id,
            receipt_timeout: section.receipt_timeout,
            poll_interval: section.poll_interval,
            max_fee: section.max_fee_gwei.0,
            floor: AtomicU64::new(0),
        })
    }

    /// Checks every endpoint that answers is on the vault's chain; one that does not answer is
    /// checked before its first read.
    /// Error cases:
    /// - an endpoint answers with another chain id, or none answers
    pub async fn check(&self) -> anyhow::Result<()> {
        let mut answered = false;
        for index in 0..self.endpoints.len() {
            match self.chain_of(index).await {
                Ok(id) if id == self.chain_id => answered = true,
                Ok(id) => bail!("chain.rpc[{index}] is on chain {id}, not the vault's chain {}", self.chain_id),
                Err(e) => warn!(endpoint = index, "the chain id was not read: {e}"),
            }
        }
        ensure!(answered, "no chain.rpc endpoint answered");
        Ok(())
    }

    /// The chain endpoint `index` is on, read until it answers.
    /// Error cases:
    /// - the endpoint does not answer, described without its URL
    async fn chain_of(&self, index: usize) -> Result<u64, String> {
        let known = self.chains[index].load(Ordering::Relaxed);
        if known != 0 {
            return Ok(known);
        }
        let id = self.endpoints[index].get_chain_id().await.map_err(|e| describe(&e))?;
        if id != self.chain_id {
            warn!(
                endpoint = index,
                chain = id,
                "the endpoint is on another chain than the vault's; no read goes to it"
            );
        }
        self.chains[index].store(id, Ordering::Relaxed);
        Ok(id)
    }

    /// `call`'s return from `to` at the latest block.
    /// Error cases:
    /// - see `ReadError`
    pub async fn view<C: SolCall>(&self, to: Address, call: C) -> Result<C::Return, ReadError> {
        let request = request(None, to, call.abi_encode().into());
        let data = self
            .read(|endpoint| {
                let request = request.clone();
                async move { endpoint.call(request).await }
            })
            .await?;
        C::abi_decode_returns(&data).map_err(|e| ReadError::Undecodable(e.to_string()))
    }

    /// `address`'s ether balance, in wei.
    /// Error cases:
    /// - see `ReadError`
    pub async fn balance(&self, address: Address) -> Result<U256, ReadError> {
        self.read(|endpoint| async move { endpoint.get_balance(address).await }).await
    }

    /// The first answer of the endpoints, in order, to `read`, from one on the vault's chain that
    /// has reached the floor. A revert is an answer, and is returned as it is; another error
    /// answer, or a failure to answer, moves to the next endpoint, and the last error answer is
    /// returned when none gives a value. While every endpoint that answers is behind the floor, it
    /// asks again each `poll_interval`, until `receipt_timeout`.
    /// Error cases:
    /// - see `ReadError`
    async fn read<T, F, Fut>(&self, read: F) -> Result<T, ReadError>
    where
        F: Fn(RootProvider) -> Fut,
        Fut: Future<Output = Result<T, TransportError>>,
    {
        let deadline = Instant::now() + self.receipt_timeout;
        loop {
            let floor = self.floor.load(Ordering::Relaxed);
            let (mut last, mut answer, mut behind) = (String::from("no endpoint"), None, false);
            for (index, endpoint) in self.endpoints.iter().enumerate() {
                match self.chain_of(index).await {
                    Ok(id) if id == self.chain_id => {}
                    Ok(_) => continue,
                    Err(e) => {
                        debug!(endpoint = index, "no answer: {e}");
                        last = e;
                        continue;
                    }
                }
                if floor > 0 {
                    match endpoint.get_block_number().await {
                        Ok(height) if height >= floor => {}
                        Ok(height) => {
                            debug!(endpoint = index, height, floor, "behind the bot's last transaction");
                            behind = true;
                            continue;
                        }
                        Err(e) => {
                            last = describe(&e);
                            debug!(endpoint = index, "no answer: {last}");
                            continue;
                        }
                    }
                }
                match read(endpoint.clone()).await {
                    Ok(value) => return Ok(value),
                    Err(RpcError::ErrorResp(payload)) => {
                        let revert = payload.as_revert_data();
                        let message = payload.message.to_string();
                        let reverted = revert.is_some() || payload.code == 3 || message.contains("revert");
                        let error = ReadError::Answer { code: payload.code, message, revert };
                        if reverted {
                            return Err(error);
                        }
                        debug!(endpoint = index, "an error answer: {error}");
                        answer = Some(error);
                    }
                    Err(e) => {
                        last = describe(&e);
                        debug!(endpoint = index, "no answer: {last}");
                    }
                }
            }
            if let Some(answer) = answer {
                return Err(answer);
            }
            if !behind || Instant::now() >= deadline {
                let reason = if behind { format!("every endpoint that answers is behind block {floor}") } else { last };
                return Err(ReadError::Unanswered(reason));
            }
            sleep(self.poll_interval).await;
        }
    }
}

/// A call of `to` with `data`, from `from` when the call depends on its sender.
fn request(from: Option<Address>, to: Address, data: Bytes) -> TransactionRequest {
    TransactionRequest { from, to: Some(TxKind::Call(to)), input: TransactionInput::new(data), ..Default::default() }
}

/// A transport failure, described without the endpoint's URL, which the HTTP client's own errors
/// quote.
fn describe(error: &TransportError) -> String {
    match error {
        RpcError::ErrorResp(payload) => format!("{} (code {})", payload.message, payload.code),
        RpcError::Transport(TransportErrorKind::HttpError(http))
        | RpcError::Transport(TransportErrorKind::HttpErrorWithRetryAfter { error: http, .. }) => {
            format!("HTTP {}", http.status)
        }
        RpcError::Transport(TransportErrorKind::Custom(e)) => match e.downcast_ref::<reqwest::Error>() {
            Some(e) if e.is_timeout() => String::from("timed out"),
            Some(e) if e.is_connect() => String::from("no connection"),
            Some(_) => String::from("the request failed"),
            None => String::from("the transport failed"),
        },
        RpcError::DeserError { .. } => String::from("an answer that is not JSON-RPC"),
        RpcError::NullResp => String::from("a null answer"),
        _ => String::from("an RPC failure"),
    }
}
