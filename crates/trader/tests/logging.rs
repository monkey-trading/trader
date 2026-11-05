//! The bot's log against the stand-in chain: no line carries a secret, whatever the level names. A
//! binary of its own, as the one test that installs a subscriber: tracing caches what each call
//! site may write across a process's threads.

mod common;

use alloy_primitives::U256;
use alloy_sol_types::SolCall;
use trader::chain::contracts::Vault;
use trader::chain::{Call, Chain};
use trader::logging::Secrets;

use common::lifecycle::chain::{CHAIN_ID, VAULT};
use common::lifecycle::{Lines, Setup, config};

/// No line the chain client's work writes, at any level and whatever the level's directives name,
/// carries the key the endpoints' URLs hold in their path.
#[tokio::test]
async fn no_line_carries_an_endpoints_key() {
    let s = Setup::start(false).await;
    let key = "k3y0f32charactersfromaprovider00";
    let (down, up) = (format!("http://127.0.0.1:9/v2/{key}"), format!("{}/v2/{key}", s.chain.url()));
    let lines = Lines::default();
    let writer = lines.clone();
    let level = "trace,alloy_transport_http=trace,alloy_rpc_client=trace,reqwest::connect=trace,hyper_util=trace";
    let secrets = Secrets::of_urls([down.as_str(), up.as_str()]);
    let subscriber = trader::logging::subscriber(level, false, move || writer.clone(), secrets).unwrap();
    let _logging = tracing::subscriber::set_default(subscriber);
    let section = config(&s.node, &[down.clone(), up.clone()], s.dir.path(), false, "").chain.unwrap();
    let chain = Chain::connect(&section, CHAIN_ID).await.unwrap();
    chain.view(VAULT, Vault::usdcCall {}).await.unwrap();
    let deposit = Vault::depositCall { amount: U256::ZERO }.abi_encode().into();
    chain.send(&s.key, &Call { to: VAULT, data: deposit, what: "a deposit".into() }).await.unwrap();
    tracing::info!(target: "reqwest::connect", "connecting to {up}");
    let written = lines.text();
    assert!(written.contains("a deposit: mined") && written.contains("connecting to [redacted]"), "{written}");
    assert!(!written.contains(key), "{written}");
}
