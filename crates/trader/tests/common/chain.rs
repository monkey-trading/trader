//! A local stand-in for the custody chain: a JSON-RPC endpoint that decodes each signed transaction
//! the bot sends, recovers its sender, and applies the token's and the vault's calls the bot makes
//! (approve, deposit, depositFor, claimEmissions, withdraw) to its own ledger, with their events in
//! the receipt. A test scripts reverts, refusals and outages.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use alloy_consensus::{SignableTransaction, Transaction, TxEnvelope};
use alloy_eips::eip2718::Decodable2718;
use alloy_primitives::{Address, B256, Bytes, Log, U256, keccak256};
use alloy_sol_types::{SolCall, SolEvent, SolValue};
use axum::Router;
use axum::extract::State;
use axum::routing::post;
use engine_core::eip712::{self, Signature};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use trader::chain::contracts::{ERC20, Vault};

pub const CHAIN_ID: u64 = 31337;
pub const VAULT: Address = Address::ZERO;
pub const USDC: Address = Address::repeat_byte(0xcc);
pub const MONKEY: Address = Address::repeat_byte(0xdd);

/// A transaction the chain took.
#[derive(Clone, Debug)]
pub struct Sent {
    pub hash: B256,
    pub from: Address,
    pub to: Address,
    pub input: Bytes,
    pub nonce: u64,
}

/// What the chain holds and has seen.
#[derive(Default)]
pub struct Ledger {
    pub chain_id: u64,
    pub eth: BTreeMap<Address, U256>,
    pub usdc: BTreeMap<Address, U256>,
    pub allowance: BTreeMap<(Address, Address), U256>,
    pub monkey: BTreeMap<Address, U256>,
    pub monkey_claimed: BTreeMap<Address, U256>,
    pub used_nonces: BTreeSet<(Address, u64)>,
    pub nonces: BTreeMap<Address, u64>,
    pub sent: Vec<Sent>,
    /// Receipts by transaction hash, and every log in them.
    receipts: BTreeMap<B256, Value>,
    logs: Vec<Value>,
    /// The vault's deposits by id: payer and amount.
    pub deposits: Vec<(Address, U256)>,
    /// Whether receipts are answered as not held, as by an endpoint whose receipts lag its state.
    pub hide_receipts: bool,
    /// Revert data for every estimate and call of a selector, as the contract would revert.
    pub reverts: BTreeMap<[u8; 4], Vec<u8>>,
    /// The error message each raw transaction is refused with, in turn, before one is taken.
    pub refusals: VecDeque<String>,
    /// Whether the next transaction mines and reverts, though its estimate passed.
    pub revert_on_chain: Option<Vec<u8>>,
    /// Whether every request fails as an outage, and the methods that do.
    pub down: bool,
    pub failing: BTreeSet<String>,
    /// Methods answered with a provider's error that is not a revert.
    pub limited: BTreeSet<String>,
    /// Whether the next raw transaction is answered with its hash but never mined, as when another
    /// transaction from its sender takes its nonce.
    pub replace: bool,
    /// The gas every estimate gives, when not the default 100,000.
    pub estimate: Option<u64>,
    /// Whether a raw transaction is taken, and mined, but its answer fails as an outage.
    pub swallow: bool,
    /// How many blocks behind its own transactions the endpoint says it is.
    pub lag: u64,
    /// How many requests of each method it answered.
    pub methods: BTreeMap<String, usize>,
}

/// A running stand-in chain.
pub struct ChainStandIn {
    pub ledger: Mutex<Ledger>,
    pub addr: SocketAddr,
}

impl ChainStandIn {
    /// Starts one on a free loopback port.
    pub async fn start() -> Arc<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let ledger = Mutex::new(Ledger { chain_id: CHAIN_ID, ..Ledger::default() });
        let chain = Arc::new(Self { ledger, addr: listener.local_addr().unwrap() });
        let app = Router::new().route("/", post(rpc)).route("/{*path}", post(rpc)).with_state(chain.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        chain
    }

    /// Its URL.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.addr.port())
    }

    /// Runs `edit` on the ledger.
    pub fn with<T>(&self, edit: impl FnOnce(&mut Ledger) -> T) -> T {
        edit(&mut self.ledger.lock().unwrap())
    }

    /// The transactions taken so far.
    pub fn sent(&self) -> Vec<Sent> {
        self.with(|l| l.sent.clone())
    }

    /// Waits until `count` transactions were taken; the last one.
    pub async fn sent_count(&self, count: usize) -> Sent {
        for _ in 0..500 {
            if let Some(sent) = self.with(|l| (l.sent.len() >= count).then(|| l.sent[count - 1].clone())) {
                return sent;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("the bot sent fewer than {count} transactions");
    }
}

/// Answers one JSON-RPC request.
async fn rpc(State(chain): State<Arc<ChainStandIn>>, body: axum::Json<Value>) -> axum::response::Response {
    use axum::response::IntoResponse;
    let request = body.0;
    let (method, params, id) =
        (request["method"].as_str().unwrap_or_default(), &request["params"], request["id"].clone());
    let answer = chain.with(|ledger| {
        *ledger.methods.entry(method.to_owned()).or_default() += 1;
        if ledger.down || ledger.failing.contains(method) {
            return None;
        }
        if ledger.limited.contains(method) {
            return Some(Err(json!({ "code": -32005, "message": "limit exceeded" })));
        }
        let answer = ledger.answer(method, params);
        (!(ledger.swallow && method == "eth_sendRawTransaction")).then_some(answer)
    });
    match answer {
        None => (axum::http::StatusCode::SERVICE_UNAVAILABLE, "down").into_response(),
        Some(Ok(result)) => axum::Json(json!({ "jsonrpc": "2.0", "id": id, "result": result })).into_response(),
        Some(Err(error)) => axum::Json(json!({ "jsonrpc": "2.0", "id": id, "error": error })).into_response(),
    }
}

impl Ledger {
    /// The result of `method`, or its JSON-RPC error.
    fn answer(&mut self, method: &str, params: &Value) -> Result<Value, Value> {
        let quantity = |n: u128| json!(format!("0x{n:x}"));
        match method {
            "eth_chainId" => Ok(quantity(self.chain_id.into())),
            "eth_gasPrice" => Ok(quantity(100_000_000)),
            "eth_maxPriorityFeePerGas" => Ok(quantity(0)),
            "eth_blockNumber" => Ok(quantity(self.height().saturating_sub(self.lag).into())),
            "eth_getBalance" => {
                let address: Address = serde_json::from_value(params[0].clone()).unwrap();
                Ok(json!(self.eth.get(&address).copied().unwrap_or_default()))
            }
            "eth_getTransactionCount" => {
                let address: Address = serde_json::from_value(params[0].clone()).unwrap();
                Ok(quantity(self.nonces.get(&address).copied().unwrap_or_default().into()))
            }
            "eth_estimateGas" | "eth_call" => {
                let call = &params[0];
                let to: Address = serde_json::from_value(call["to"].clone()).unwrap();
                let from: Address = serde_json::from_value(call["from"].clone()).unwrap_or_default();
                let input: Bytes = serde_json::from_value(call["input"].clone()).unwrap_or_default();
                if let Some(revert) = input.get(..4).and_then(|selector| self.reverts.get(selector)) {
                    return Err(revert_error(revert));
                }
                if method == "eth_estimateGas" {
                    return Ok(quantity(self.estimate.unwrap_or(100_000).into()));
                }
                self.view(to, from, &input).map(|data| json!(Bytes::from(data))).map_err(|data| revert_error(&data))
            }
            "eth_sendRawTransaction" => {
                let raw: Bytes = serde_json::from_value(params[0].clone()).unwrap();
                if std::mem::take(&mut self.replace) {
                    let (from, nonce) = sender(&raw);
                    let next = self.nonces.entry(from).or_default();
                    *next = (*next).max(nonce + 1);
                    let envelope = TxEnvelope::decode_2718(&mut &raw[..]).expect("a signed transaction");
                    return Ok(json!(envelope.tx_hash()));
                }
                if let Some(message) = self.refusals.pop_front() {
                    // A spent nonce is one another transaction from the sender took.
                    if message.contains("nonce too low") {
                        let (from, nonce) = sender(&raw);
                        let next = self.nonces.entry(from).or_default();
                        *next = (*next).max(nonce + 1);
                    }
                    return Err(json!({ "code": -32000, "message": message }));
                }
                self.take(&raw)
            }
            "eth_getTransactionReceipt" => {
                let hash: B256 = serde_json::from_value(params[0].clone()).unwrap();
                let receipt = self.receipts.get(&hash).filter(|_| !self.hide_receipts);
                Ok(receipt.cloned().unwrap_or(Value::Null))
            }
            "eth_getLogs" => {
                // By address and the topics given; every block the stand-in has is in range.
                let filter = &params[0];
                let topics: Vec<Option<B256>> = filter["topics"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|topic| serde_json::from_value(topic.clone()).ok())
                    .collect();
                let matches = |log: &&Value| {
                    log["address"] == filter["address"]
                        && topics
                            .iter()
                            .enumerate()
                            .all(|(at, topic)| topic.is_none_or(|topic| log["topics"][at] == json!(topic)))
                };
                Ok(json!(self.logs.iter().filter(matches).cloned().collect::<Vec<_>>()))
            }
            other => Err(json!({ "code": -32601, "message": format!("{other} is not served here") })),
        }
    }

    /// A read-only call's return data, or its revert data.
    fn view(&self, to: Address, _from: Address, input: &[u8]) -> Result<Vec<u8>, Vec<u8>> {
        let get = |map: &BTreeMap<Address, U256>, address: Address| map.get(&address).copied().unwrap_or_default();
        Ok(match (to, input.get(..4).unwrap_or_default()) {
            (VAULT, s) if s == Vault::usdcCall::SELECTOR => USDC.abi_encode(),
            (VAULT, s) if s == Vault::monkeyCall::SELECTOR => MONKEY.abi_encode(),
            (VAULT, s) if s == Vault::emissionsClaimedCall::SELECTOR => {
                let call = Vault::emissionsClaimedCall::abi_decode(input).unwrap();
                get(&self.monkey_claimed, call.account).abi_encode()
            }
            (VAULT, s) if s == Vault::depositCountCall::SELECTOR => U256::from(self.deposits.len()).abi_encode(),
            (VAULT, s) if s == Vault::depositRecordsCall::SELECTOR => {
                let id = Vault::depositRecordsCall::abi_decode(input).unwrap().id;
                let (payer, amount) = self.deposits.get(id.to::<usize>()).copied().unwrap_or_default();
                // A uint96 encodes as the uint256 of the same value.
                (payer, amount).abi_encode_params()
            }
            (VAULT, s) if s == Vault::usedNoncesCall::SELECTOR => {
                let call = Vault::usedNoncesCall::abi_decode(input).unwrap();
                self.used_nonces.contains(&(call.account, call.nonce)).abi_encode()
            }
            (USDC, s) if s == ERC20::balanceOfCall::SELECTOR => {
                get(&self.usdc, ERC20::balanceOfCall::abi_decode(input).unwrap().account).abi_encode()
            }
            (MONKEY, s) if s == ERC20::balanceOfCall::SELECTOR => {
                get(&self.monkey, ERC20::balanceOfCall::abi_decode(input).unwrap().account).abi_encode()
            }
            (USDC, s) if s == ERC20::allowanceCall::SELECTOR => {
                let call = ERC20::allowanceCall::abi_decode(input).unwrap();
                self.allowance.get(&(call.owner, call.spender)).copied().unwrap_or_default().abi_encode()
            }
            _ => return Err(Vec::new()),
        })
    }

    /// Takes a signed transaction: checks its chain and nonce, applies it, and mines its receipt.
    fn take(&mut self, raw: &[u8]) -> Result<Value, Value> {
        let envelope = TxEnvelope::decode_2718(&mut &raw[..]).expect("a signed EIP-2718 transaction");
        let TxEnvelope::Eip1559(signed) = &envelope else { panic!("the bot sends EIP-1559 transactions") };
        let tx = signed.tx();
        assert_eq!(tx.chain_id, self.chain_id, "signed for the endpoint's chain");
        assert!(tx.value().is_zero(), "the bot sends no ether");
        let (from, _) = sender(raw);
        let expected = self.nonces.get(&from).copied().unwrap_or_default();
        if tx.nonce < expected {
            return Err(json!({ "code": -32000, "message": "nonce too low" }));
        }
        let to = tx.to.to().copied().expect("a call, not a creation");
        let hash = *signed.hash();
        self.nonces.insert(from, tx.nonce + 1);
        self.sent.push(Sent { hash, from, to, input: tx.input.clone(), nonce: tx.nonce });
        let block = format!("0x{:x}", self.height());
        let outcome = match self.revert_on_chain.take() {
            Some(revert) => Err(revert),
            None => self.apply(from, to, &tx.input),
        };
        let (status, logs) = match outcome {
            Ok(logs) => ("0x1", logs),
            Err(revert) => {
                // The replay of the call at its block reverts the same way.
                self.reverts.insert(tx.input[..4].try_into().unwrap(), revert);
                ("0x0", Vec::new())
            }
        };
        let logs: Vec<Value> = logs
            .into_iter()
            .enumerate()
            .map(|(index, log)| {
                json!({ "address": log.address, "topics": log.data.topics(), "data": log.data.data, "blockNumber": block,
                    "blockHash": B256::repeat_byte(1), "transactionHash": hash, "transactionIndex": "0x0",
                    "logIndex": format!("0x{index:x}"), "removed": false })
            })
            .collect();
        self.logs.extend(logs.iter().cloned());
        self.receipts.insert(
            hash,
            json!({ "transactionHash": hash, "transactionIndex": "0x0", "blockHash": B256::repeat_byte(1),
                "blockNumber": block, "from": from, "to": to, "cumulativeGasUsed": "0x5208", "gasUsed": "0x5208",
                "effectiveGasPrice": "0x5f5e100", "contractAddress": null, "logs": logs,
                "logsBloom": format!("0x{}", "0".repeat(512)), "type": "0x2", "status": status }),
        );
        Ok(json!(hash))
    }

    /// The latest block: one for each transaction taken, after 16 before any.
    fn height(&self) -> u64 {
        16 + self.sent.len() as u64
    }

    /// Applies the call `from` makes to `to`; its logs, or its revert data.
    fn apply(&mut self, from: Address, to: Address, input: &[u8]) -> Result<Vec<Log>, Vec<u8>> {
        let log = |address: Address, data| Log { address, data };
        let selector: [u8; 4] = input[..4].try_into().unwrap();
        match (to, selector) {
            (USDC, ERC20::approveCall::SELECTOR) => {
                let call = ERC20::approveCall::abi_decode(input).unwrap();
                self.allowance.insert((from, call.spender), call.value);
                let event = ERC20::Approval { owner: from, spender: call.spender, value: call.value };
                Ok(vec![log(USDC, event.encode_log_data())])
            }
            (VAULT, Vault::depositCall::SELECTOR) => {
                let amount = Vault::depositCall::abi_decode(input).unwrap().amount;
                self.pull(from, amount)?;
                let id = self.record(from, amount);
                Ok(vec![log(VAULT, Vault::Deposited { id, account: from, from, amount }.encode_log_data())])
            }
            (VAULT, Vault::depositForCall::SELECTOR) => {
                let call = Vault::depositForCall::abi_decode(input).unwrap();
                self.pull(from, call.amount)?;
                let id = self.record(from, call.amount);
                let event = Vault::Deposited { id, account: call.account, from, amount: call.amount };
                Ok(vec![log(VAULT, event.encode_log_data())])
            }
            (VAULT, Vault::claimEmissionsCall::SELECTOR) => {
                let call = Vault::claimEmissionsCall::abi_decode(input).unwrap();
                let claimed = self.monkey_claimed.get(&call.account).copied().unwrap_or_default();
                let amount = call.entitled - claimed;
                self.monkey_claimed.insert(call.account, call.entitled);
                *self.monkey.entry(call.account).or_default() += amount;
                let event = Vault::EmissionsClaimed {
                    account: call.account,
                    amount,
                    entitled: call.entitled,
                    seq: call.checkpointSeq,
                };
                Ok(vec![log(VAULT, event.encode_log_data())])
            }
            (VAULT, Vault::withdrawCall::SELECTOR) => {
                let w = Vault::withdrawCall::abi_decode(input).unwrap().withdrawal;
                self.used_nonces.insert((w.account, w.nonce));
                *self.usdc.entry(w.account).or_default() += w.amount;
                let event = Vault::WithdrawalPaid {
                    account: w.account,
                    amount: w.amount,
                    nonce: w.nonce,
                    approvedSeq: w.approvedSeq,
                };
                Ok(vec![log(VAULT, event.encode_log_data())])
            }
            (to, selector) => panic!("the bot called {to} with {selector:?}, which it never should"),
        }
    }

    /// Records a deposit of `amount` from `payer`; its id.
    fn record(&mut self, payer: Address, amount: U256) -> U256 {
        self.deposits.push((payer, amount));
        U256::from(self.deposits.len() - 1)
    }

    /// Moves `amount` of `from`'s USDC into the vault against its allowance.
    fn pull(&mut self, from: Address, amount: U256) -> Result<(), Vec<u8>> {
        let allowed = self.allowance.get(&(from, VAULT)).copied().unwrap_or_default();
        let held = self.usdc.get(&from).copied().unwrap_or_default();
        if allowed < amount || held < amount {
            return Err(keccak256("short")[..4].to_vec());
        }
        self.allowance.insert((from, VAULT), allowed - amount);
        self.usdc.insert(from, held - amount);
        *self.usdc.entry(VAULT).or_default() += amount;
        Ok(())
    }
}

/// The sender of a signed EIP-1559 transaction, recovered from its signature, and its nonce.
fn sender(raw: &[u8]) -> (Address, u64) {
    let TxEnvelope::Eip1559(signed) = TxEnvelope::decode_2718(&mut &raw[..]).expect("a signed transaction") else {
        panic!("the bot sends EIP-1559 transactions")
    };
    let sig = signed.signature();
    let mut bytes = [0u8; 65];
    bytes[..32].copy_from_slice(&sig.r().to_be_bytes::<32>());
    bytes[32..64].copy_from_slice(&sig.s().to_be_bytes::<32>());
    bytes[64] = 27 + u8::from(sig.v());
    let from = eip712::recover(&signed.tx().signature_hash(), &Signature::from(bytes)).expect("a valid signature");
    (Address::from(from.0), signed.tx().nonce)
}

/// A JSON-RPC revert error carrying `data`.
fn revert_error(data: &[u8]) -> Value {
    json!({ "code": 3, "message": "execution reverted", "data": Bytes::from(data.to_vec()) })
}
