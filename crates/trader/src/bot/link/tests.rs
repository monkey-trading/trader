use std::path::Path;

use serde_json::json;

use super::*;
use crate::config::{AccountPlan, Config};

/// The SMA crossover on periods `fast` and `slow`.
fn sma(fast: usize, slow: usize) -> StrategyParams {
    StrategyParams::SmaCrossover { fast, slow }
}

/// The example's account: BTC-USD, fast 5 over slow 20, bounds fast [2, 20] and slow [10, 100].
fn plan() -> AccountPlan {
    let cfg = Config::load(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../config/example.toml"))).unwrap();
    cfg.plans.into_iter().next().unwrap()
}

/// Saved controls for the example's strategy.
fn saved(running: bool, strategy_running: bool, params: StrategyParams, config_params: StrategyParams) -> Saved {
    Saved {
        running,
        strategies: vec![SavedStrategy {
            market: "BTC-USD".into(),
            kind: "sma_crossover".into(),
            running: strategy_running,
            params,
            config_params,
        }],
    }
}

fn config_params() -> StrategyParams {
    sma(5, 20)
}

fn set() -> StrategyParams {
    sma(3, 30)
}

/// With nothing saved the config's controls hold: running, on its periods.
#[tokio::test]
async fn nothing_saved_is_the_config() {
    let dir = tempfile::tempdir().unwrap();
    let controls = Saved::load(dir.path().join("none.json"), &plan()).await.unwrap();
    assert_eq!(controls, saved(true, true, config_params(), config_params()));
    assert!(controls.trading());
}

/// What was saved loads back: the stops, and periods within the bounds.
#[tokio::test]
async fn saved_controls_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("accounts/0xabc.json");
    for controls in [
        saved(false, true, set(), config_params()),
        saved(true, false, set(), config_params()),
        saved(true, true, set(), config_params()),
    ] {
        controls.save(path.clone()).await.unwrap();
        assert_eq!(Saved::load(path.clone(), &plan()).await.unwrap(), controls);
    }
}

/// Saved periods give way to the config's when the config's own periods changed since, or its
/// bounds no longer admit them, or it sets none; the stops hold either way.
#[tokio::test]
async fn the_configs_periods_win_when_it_changed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("controls.json");
    let edited = sma(4, 20);
    saved(false, false, set(), edited).save(path.clone()).await.unwrap();
    assert_eq!(
        Saved::load(path.clone(), &plan()).await.unwrap(),
        saved(false, false, config_params(), config_params())
    );

    let mut narrowed = plan();
    narrowed.strategies[0].bounds = Some(serde_json::from_value(json!({ "fast": [2, 20], "slow": [10, 25] })).unwrap());
    saved(true, true, set(), config_params()).save(path.clone()).await.unwrap();
    assert_eq!(
        Saved::load(path.clone(), &narrowed).await.unwrap(),
        saved(true, true, config_params(), config_params())
    );

    let mut unbounded = plan();
    unbounded.strategies[0].bounds = None;
    assert_eq!(Saved::load(path, &unbounded).await.unwrap(), saved(true, true, config_params(), config_params()));
}

/// A strategy saved for another market or kind is not the config's: its controls do not apply,
/// though the account's stop does.
#[tokio::test]
async fn another_strategys_controls_do_not_apply() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("controls.json");
    for (market, kind) in [("ETH-USD", "sma_crossover"), ("BTC-USD", "ema_crossover")] {
        let mut other = saved(false, false, set(), config_params());
        other.strategies[0].market = market.into();
        other.strategies[0].kind = kind.into();
        other.save(path.clone()).await.unwrap();
        assert_eq!(
            Saved::load(path.clone(), &plan()).await.unwrap(),
            saved(false, true, config_params(), config_params())
        );
    }
}

/// A file that holds no controls stops the start, and is left for the user.
#[tokio::test]
async fn damaged_controls_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("controls.json");
    for text in ["", "{", r#"{"running": true}"#, r#"{"running": true, "strategies": [], "extra": 1}"#] {
        std::fs::write(&path, text).unwrap();
        let err = Saved::load(path.clone(), &plan()).await.unwrap_err();
        assert!(format!("{err:#}").contains("delete it to start from the config"), "{text}: {err:#}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }
}
