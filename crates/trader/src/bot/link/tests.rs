use std::path::Path;

use super::*;

/// The example's config: BTC-USD, fast 5 over slow 20, bounds fast [2, 20] and slow [10, 100].
fn config() -> Config {
    Config::load(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../config/example.toml"))).unwrap()
}

/// Saved controls for the example's strategy.
fn saved(running: bool, strategy_running: bool, params: SmaParams, config_params: SmaParams) -> Saved {
    Saved {
        running,
        strategies: vec![SavedStrategy {
            market: "BTC-USD".into(),
            kind: SMA_CROSSOVER.into(),
            running: strategy_running,
            params,
            config_params,
        }],
    }
}

const CONFIG_PARAMS: SmaParams = SmaParams { fast: 5, slow: 20 };
const SET: SmaParams = SmaParams { fast: 3, slow: 30 };

/// With nothing saved the config's controls hold: running, on its periods.
#[tokio::test]
async fn nothing_saved_is_the_config() {
    let dir = tempfile::tempdir().unwrap();
    let controls = Saved::load(dir.path().join("none.json"), &config()).await.unwrap();
    assert_eq!(controls, saved(true, true, CONFIG_PARAMS, CONFIG_PARAMS));
    assert!(controls.trading());
}

/// What was saved loads back: the stops, and periods within the bounds.
#[tokio::test]
async fn saved_controls_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("accounts/0xabc.json");
    for controls in [
        saved(false, true, SET, CONFIG_PARAMS),
        saved(true, false, SET, CONFIG_PARAMS),
        saved(true, true, SET, CONFIG_PARAMS),
    ] {
        controls.save(path.clone()).await.unwrap();
        assert_eq!(Saved::load(path.clone(), &config()).await.unwrap(), controls);
    }
}

/// Saved periods give way to the config's when the config's own periods changed since, or its
/// bounds no longer admit them, or it sets none; the stops hold either way.
#[tokio::test]
async fn the_configs_periods_win_when_it_changed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("controls.json");
    let edited = SmaParams { fast: 4, slow: 20 };
    saved(false, false, SET, edited).save(path.clone()).await.unwrap();
    assert_eq!(Saved::load(path.clone(), &config()).await.unwrap(), saved(false, false, CONFIG_PARAMS, CONFIG_PARAMS));

    let mut narrowed = config();
    narrowed.strategy.bounds = Some(SmaBounds { fast: [2, 20], slow: [10, 25] });
    saved(true, true, SET, CONFIG_PARAMS).save(path.clone()).await.unwrap();
    assert_eq!(Saved::load(path.clone(), &narrowed).await.unwrap(), saved(true, true, CONFIG_PARAMS, CONFIG_PARAMS));

    let mut unbounded = config();
    unbounded.strategy.bounds = None;
    assert_eq!(Saved::load(path, &unbounded).await.unwrap(), saved(true, true, CONFIG_PARAMS, CONFIG_PARAMS));
}

/// A strategy saved for another market or kind is not the config's: its controls do not apply,
/// though the account's stop does.
#[tokio::test]
async fn another_strategys_controls_do_not_apply() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("controls.json");
    for (market, kind) in [("ETH-USD", SMA_CROSSOVER), ("BTC-USD", "ema_crossover")] {
        let mut other = saved(false, false, SET, CONFIG_PARAMS);
        other.strategies[0].market = market.into();
        other.strategies[0].kind = kind.into();
        other.save(path.clone()).await.unwrap();
        assert_eq!(
            Saved::load(path.clone(), &config()).await.unwrap(),
            saved(false, true, CONFIG_PARAMS, CONFIG_PARAMS)
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
        let err = Saved::load(path.clone(), &config()).await.unwrap_err();
        assert!(format!("{err:#}").contains("delete it to start from the config"), "{text}: {err:#}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }
}

/// The link opens on the saved controls, and its status shows them.
#[tokio::test]
async fn the_link_opens_on_the_saved_controls() {
    let dir = tempfile::tempdir().unwrap();
    let address = AccountId([0xab; 20]);
    let path = saved::path(dir.path(), address);
    assert_eq!(path, dir.path().join(format!("accounts/0x{}.json", "ab".repeat(20))));
    saved(false, true, SET, CONFIG_PARAMS).save(path).await.unwrap();
    let (mut link, console) = Link::open(&config(), address, dir.path().to_owned()).await.unwrap();
    assert_eq!((link.params(), link.trading()), (SET, false));
    assert_eq!(console.address, address);
    let status = console.status.borrow().clone();
    assert_eq!(status.state, AccountState::Starting);
    assert_eq!(
        status.strategies,
        [StrategyStatus {
            id: 0,
            kind: SMA_CROSSOVER,
            market: "BTC-USD".into(),
            bar: Duration::from_secs(60),
            running: true,
            params: SET,
            bounds: Some(SmaBounds { fast: [2, 20], slow: [10, 100] }),
            warmed_up: false,
            target: None,
        }]
    );
    link.started = true;
    link.publish(true);
    assert_eq!(console.status.borrow().state, AccountState::Stopped);
    assert!(console.status.borrow().strategies[0].warmed_up);
    link.halted("gone".into(), true);
    assert_eq!(
        (console.status.borrow().state, console.status.borrow().halt.as_deref()),
        (AccountState::Halted, Some("gone"))
    );
}
