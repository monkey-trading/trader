//! The JSON-RPC calls the bot makes of a chain endpoint, over `alloy-rpc-client` alone: the
//! provider crate's network layer pulls in a signer stack on a `crypto-bigint` with secret-dependent
//! timing, and the bot signs with `k256` itself. Each call sends what that crate's provider did,
//! including a call and a gas estimate at `pending` unless a block is named.

use alloy_eips::BlockNumberOrTag;
use alloy_primitives::{Address, B256, Bytes, U64, U128, U256, hex};
use alloy_rpc_client::RpcClient;
use alloy_rpc_types_eth::{Block, BlockId, Filter, Log, TransactionReceipt, TransactionRequest};
use alloy_transport::TransportResult;

/// One endpoint's JSON-RPC client.
#[derive(Clone)]
pub(super) struct Endpoint(RpcClient);

impl Endpoint {
    pub(super) fn new(client: RpcClient) -> Self {
        Self(client)
    }

    pub(super) async fn get_chain_id(&self) -> TransportResult<u64> {
        Ok(self.0.request_noparams::<U64>("eth_chainId").await?.to())
    }

    pub(super) async fn get_block_number(&self) -> TransportResult<u64> {
        Ok(self.0.request_noparams::<U64>("eth_blockNumber").await?.to())
    }

    pub(super) async fn get_gas_price(&self) -> TransportResult<u128> {
        Ok(self.0.request_noparams::<U128>("eth_gasPrice").await?.to())
    }

    pub(super) async fn get_max_priority_fee_per_gas(&self) -> TransportResult<u128> {
        Ok(self.0.request_noparams::<U128>("eth_maxPriorityFeePerGas").await?.to())
    }

    /// `address`'s balance at the latest block.
    pub(super) async fn get_balance(&self, address: Address) -> TransportResult<U256> {
        self.0.request("eth_getBalance", (address, BlockId::latest())).await
    }

    /// `address`'s transaction count at `block`.
    pub(super) async fn get_transaction_count(&self, address: Address, block: BlockId) -> TransportResult<u64> {
        Ok(self.0.request::<_, U64>("eth_getTransactionCount", (address, block)).await?.to())
    }

    /// The block `number` names, with its transactions' hashes only.
    pub(super) async fn get_block_by_number(&self, number: BlockNumberOrTag) -> TransportResult<Option<Block>> {
        self.0.request("eth_getBlockByNumber", (number, false)).await
    }

    pub(super) async fn get_logs(&self, filter: &Filter) -> TransportResult<Vec<Log>> {
        self.0.request("eth_getLogs", (filter,)).await
    }

    pub(super) async fn get_transaction_receipt(&self, hash: B256) -> TransportResult<Option<TransactionReceipt>> {
        self.0.request("eth_getTransactionReceipt", (hash,)).await
    }

    /// `request` run at the pending block.
    pub(super) async fn call(&self, request: TransactionRequest) -> TransportResult<Bytes> {
        self.call_at(request, BlockId::pending()).await
    }

    /// `request` run at `block`.
    pub(super) async fn call_at(&self, request: TransactionRequest, block: BlockId) -> TransportResult<Bytes> {
        self.0.request("eth_call", (request, block)).await
    }

    /// The gas `request` uses, estimated at the pending block.
    pub(super) async fn estimate_gas(&self, request: TransactionRequest) -> TransportResult<u64> {
        Ok(self.0.request::<_, U64>("eth_estimateGas", (request, BlockId::pending())).await?.to())
    }

    /// Sends the signed, encoded transaction `raw`; the hash the endpoint gives it.
    pub(super) async fn send_raw_transaction(&self, raw: &[u8]) -> TransportResult<B256> {
        self.0.request("eth_sendRawTransaction", (hex::encode_prefixed(raw),)).await
    }
}
