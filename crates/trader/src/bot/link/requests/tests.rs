use std::sync::Arc;
use std::time::Duration;

use engine_core::eip712;
use engine_core::market::MarketSpec;
use engine_wire::Deployment;
use serde_json::json;
use tokio::sync::oneshot;
use trader_core::controls::Controls;
use trader_core::strategies::StrategyParams;

use super::*;
use crate::bot::day::{Saved as DaySaved, StateFile};
use crate::bot::fleet::{AccountIn, Fleet, MarketIn, StrategyIn};
use crate::bot::link::{ConsoleEnd, Kept, SavedStrategy};
use crate::config::bounds::{Bound, ParamBounds};
use crate::config::{LimitsSection, NodeUrl};
use crate::keys::AccountKey;
use crate::node::{CallError, Node, Tls};

const T0: i64 = 1_800_000_000_000;
const MINUTE: i64 = 60_000;

fn sma(fast: usize, slow: usize) -> StrategyParams {
    StrategyParams::SmaCrossover { fast, slow }
}

fn spec() -> MarketSpec {
    serde_json::from_value(json!({
        "id": 1, "symbol": "BTC-USD", "asset_class": "crypto", "schedule": { "kind": "always_open" },
        "margin_tiers": [{ "bps": 100 }], "spread_tiers": [{ "bps": 0 }],
        "price_rules": { "max_age_ms": 1000, "max_divergence_bps": 25, "min_sources": 2 },
        "max_slippage_bps": 25, "financing": { "benchmark": "SOFR", "markup_bps": 500, "day_basis": 365 },
        "house_cap_bps": 3000, "house_cap_floor": "300000"
    }))
    .unwrap()
}

/// The controls the console saves for the one strategy: running, on 1 over 2.
fn saved() -> Saved {
    Saved {
        running: true,
        strategies: vec![SavedStrategy {
            market: "BTC-USD".into(),
            kind: "sma_crossover".into(),
            running: true,
            params: sma(1, 2),
            config_params: sma(1, 2),
        }],
    }
}

/// A runner for one account trading BTC-USD on an SMA crossover of 1 over 2, whose bounds admit 2
/// over 3, with a console that saves its controls under `dir`. No call reaches a node: the runner's
/// node is never connected to.
fn runner(dir: &std::path::Path) -> Runner {
    let mut secret = [0u8; 32];
    secret[31] = 1;
    let key = Arc::new(AccountKey::from_secret(&secret).unwrap());
    let bounds: ParamBounds =
        [("fast".to_owned(), Bound::Whole([1, 5])), ("slow".to_owned(), Bound::Whole([2, 10]))].into();
    let strategy = StrategyIn {
        market: 1,
        bar_ms: MINUTE,
        params: sma(1, 2),
        max_slippage_bps: 25,
        position: Default::default(),
        entry: Default::default(),
        running: true,
        bounds: Some(bounds),
    };
    let risk = json!({ "max_open_positions": 16, "max_total_notional": "1000000",
        "daily_loss": { "usd": "100000" }, "margin_buffer_bps": 0, "min_hold": "0s" });
    let sizing = json!({ "by": "fixed_notional", "notional": "100", "max_leverage": "50" });
    let controls =
        Controls::new(serde_json::from_value(risk).unwrap(), serde_json::from_value(sizing).unwrap(), None).unwrap();
    let account = AccountIn {
        state_file: StateFile::new(dir, key.address()),
        key,
        close_on_stop: false,
        running: true,
        balance: "1000".parse().unwrap(),
        nonce: Some(0),
        controls,
        saved: DaySaved::default(),
        strategies: vec![strategy],
    };
    let fleet = Fleet::new(&[MarketIn { spec: spec(), mark: None }], vec![account], T0).unwrap();
    let limits = LimitsSection::default();
    let tls = Tls::new(NodeUrl { host: "127.0.0.1".into(), port: 1 }, None).unwrap();
    let deployment = Deployment { chain_id: 31337, vault: [0x5f; 20] };
    let node = Arc::new(Node::new(tls, [0; 20], deployment, Duration::from_secs(1), &limits));
    let mut runner = Runner::new(fleet, node, eip712::domain(31337, engine_core::types::AccountId([0x5f; 20])), 1);
    let kept = Kept {
        order: 0,
        path: Some(dir.join("accounts/account.json")),
        saved: saved(),
        strategies: vec![("BTC-USD".into(), Duration::from_millis(MINUTE as u64))],
    };
    // The console's links are not needed: the requests are handed to the runner directly.
    runner.console = Some(ConsoleEnd::new(&runner.fleet, vec![kept]).0);
    runner
}

fn request(runner: &Runner, control: Control) -> (AccountRequest, oneshot::Receiver<Result<(), Refusal>>) {
    let (reply, answer) = oneshot::channel();
    (AccountRequest { account: runner.fleet.accounts[0].address, id: 1, control, reply }, answer)
}

fn set_2_over_3() -> Control {
    Control::SetParams { strategy: 0, params: json!({ "kind": "sma_crossover", "fast": 2, "slow": 3 }) }
}

/// The control as the runner takes it from a live account, which the halted test's refusal is
/// measured against: it applies and saves.
#[tokio::test]
async fn a_parameter_change_for_a_live_account_is_applied_and_saved() {
    let dir = tempfile::tempdir().unwrap();
    let mut runner = runner(dir.path());
    let (request, answer) = request(&runner, set_2_over_3());
    runner.on_request(request).await;
    assert_eq!(answer.await.unwrap(), Ok(()));
    assert_eq!(runner.console_end(0).saved.strategies[0].params, sma(2, 3));
    assert!(dir.path().join("accounts/account.json").exists());
}

/// A control queued while the account's nonce read is out is taken after the read's refusal halts
/// the account (the runner handles an answer before a request): it is refused as `account_halted`,
/// and nothing is saved or applied.
#[tokio::test]
async fn a_control_queued_before_the_account_halted_changes_nothing() {
    for control in
        [set_2_over_3(), Control::Run { running: false }, Control::RunStrategy { strategy: 0, running: false }]
    {
        let dir = tempfile::tempdir().unwrap();
        let mut runner = runner(dir.path());
        // The nonce read is out, and the console's request waits behind it.
        runner.out = 1;
        runner.fleet.accounts[0].busy = true;
        let (request, answer) = request(&runner, control.clone());

        let refusal = CallError::Refused { target: "/v1/nonce".into(), status: 403, error: "not allowed".into() };
        runner.handle(Done::Nonce { account: 0, outcome: Err(refusal) }).unwrap();
        assert!(runner.fleet.accounts[0].failed.is_some(), "the refusal halted the account");
        runner.on_request(request).await;

        let refusal = answer.await.unwrap().unwrap_err();
        assert_eq!(refusal.code(), "account_halted", "{control:?}");
        assert_eq!(runner.console_end(0).saved, saved(), "nothing is taken into the account's controls");
        assert!(runner.fleet.accounts[0].running, "the account's running is unchanged");
        let asg = &runner.fleet.assignments[0];
        assert!(asg.running, "the strategy's running is unchanged");
        assert_eq!(runner.fleet.series[asg.series as usize].strategies[asg.strategy as usize].params, sma(1, 2));
        assert!(!dir.path().join("accounts").exists(), "nothing is written for the account");
    }
}
