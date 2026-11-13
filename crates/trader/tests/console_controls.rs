//! The console's controls, against the stand-in: a stopped account or strategy places no order
//! until it starts again, and stays stopped across a restart; a strategy's periods change only
//! within the config's bounds; and a flatten, once the log has passed the account's last order,
//! cancels its resting orders and closes every position, each request signed by the account.

mod common;

use std::time::Duration;

use common::console::{
    BOUNDS, Running, SAME, console_config, console_config_edited, console_config_eth, figures_view, printed_password,
    seed, until_account, until_started, within,
};
use common::{MINUTE, StandIn, T0, account, btc_usd, command, domain, prices, resting_order, warm};
use engine_core::access::{AccessEvidence, SourceFailure};
use engine_core::command::{CloseAllRequest, Command};
use engine_core::eip712::{close_all_hash, recover};
use hyper::Method;
use serde_json::{Value, json};

/// Appends a price of `mid` at `minute` past T0 as `seq`, and waits until the bot has applied it.
async fn price(stand_in: &StandIn, seq: u64, minute: i64, mid: &str) {
    let ts = T0 + minute * MINUTE;
    stand_in.append((seq, 0), command(seq, ts, prices(1, mid, ts), json!([])));
    stand_in.applied((seq, 0)).await;
}

/// The paths of the requests the bot sent the node.
fn posted(stand_in: &StandIn) -> Vec<String> {
    stand_in.with(|i| i.posts.iter().map(|(path, _)| path.clone()).collect())
}

/// A stopped account follows the log but places no order; started again, it orders on the next bar.
#[tokio::test]
async fn a_stopped_account_places_no_order_until_started() {
    let stand_in = StandIn::start().await;
    seed(&stand_in);
    let dir = tempfile::tempdir().unwrap();
    let running = Running::start(console_config(&stand_in, dir.path(), "")).await;
    let cookie = running.session().await;
    until_started(&running, &cookie).await;
    let a = account().to_string();
    assert_eq!(running.post(&format!("/api/accounts/{a}/stop"), &cookie).await.status, 204);
    assert_eq!(running.get(&format!("/api/accounts/{a}"), &cookie).await.body["state"], "stopped");
    let saved: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join(format!("data/accounts/{a}.json"))).unwrap()).unwrap();
    assert_eq!(saved["running"], false);

    // These bars would buy: the one-bar average rises above the two-bar one.
    within(warm(&stand_in)).await;
    price(&stand_in, 14, 1, "101").await;
    price(&stand_in, 15, 2, "102").await;
    let view = running.get(&format!("/api/accounts/{a}"), &cookie).await.body;
    assert_eq!((&view["strategies"][0]["target"], &view["strategies"][0]["warmed_up"]), (&json!("long"), &json!(true)));
    assert_eq!(posted(&stand_in), Vec::<String>::new(), "a stopped account orders nothing");

    assert_eq!(running.post(&format!("/api/accounts/{a}/start"), &cookie).await.status, 204);
    assert_eq!(running.get(&format!("/api/accounts/{a}"), &cookie).await.body["state"], "running");
    price(&stand_in, 16, 3, "103").await;
    assert_eq!(posted(&stand_in), ["/v1/orders"]);
    running.stop_after(&stand_in).await.unwrap();
}

/// A decision already queued when the account stops, behind an order still out, is dropped: once
/// the stop is answered, no order follows.
#[tokio::test]
async fn a_queued_decision_is_dropped_when_the_account_stops() {
    let stand_in = StandIn::start().await;
    seed(&stand_in);
    let dir = tempfile::tempdir().unwrap();
    let running = Running::start(console_config(&stand_in, dir.path(), "")).await;
    let cookie = running.session().await;
    until_started(&running, &cookie).await;
    let a = account().to_string();
    within(warm(&stand_in)).await;
    // The node's time stays near the log's start, so the first order expires before the next bars
    // close, and its answer takes three seconds, holding the account.
    stand_in.with(|i| {
        i.time_ms = (T0 + 5_000) as u64;
        i.post_delay = Some(Duration::from_secs(3));
    });
    price(&stand_in, 14, 1, "101").await;
    price(&stand_in, 15, 2, "102").await;
    within(async {
        while posted(&stand_in).is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    // The averages turn short while the long order's answer is awaited: the decision queues.
    price(&stand_in, 16, 3, "90").await;
    price(&stand_in, 17, 4, "80").await;
    assert_eq!(running.get(&format!("/api/accounts/{a}"), &cookie).await.body["strategies"][0]["target"], "short");
    assert_eq!(running.post(&format!("/api/accounts/{a}/stop"), &cookie).await.status, 204);
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(posted(&stand_in), ["/v1/orders"], "the queued decision is dropped");
    running.stop_after(&stand_in).await.unwrap();
}

/// A control that cannot be saved changes nothing; a stop holds all the same, and a flatten goes
/// on, each saying the stop was not saved.
#[tokio::test]
async fn controls_that_cannot_be_saved() {
    use std::os::unix::fs::PermissionsExt;
    let stand_in = StandIn::start().await;
    seed(&stand_in);
    let dir = tempfile::tempdir().unwrap();
    let running = Running::start(console_config(&stand_in, dir.path(), BOUNDS)).await;
    let cookie = running.session().await;
    until_started(&running, &cookie).await;
    let a = account().to_string();
    let accounts = dir.path().join("data/accounts");
    std::fs::create_dir_all(&accounts).unwrap();
    std::fs::set_permissions(&accounts, std::fs::Permissions::from_mode(0o500)).unwrap();

    let path = format!("/api/accounts/{a}/strategies/0/params");
    let answer = running
        .call(Method::PUT, &path, SAME, Some(&cookie), Some(json!({ "kind": "sma_crossover", "fast": 2, "slow": 3 })))
        .await;
    assert_eq!(answer.status, 500, "{answer:?}");
    assert!(answer.body["error"].as_str().unwrap().starts_with("nothing changed"), "{answer:?}");
    assert_eq!(answer.body["code"], "controls_unsaved");
    let view = running.get(&format!("/api/accounts/{a}"), &cookie).await.body;
    assert_eq!(view["strategies"][0]["params"], json!({ "kind": "sma_crossover", "fast": 1, "slow": 2 }));

    let answer = running.post(&format!("/api/accounts/{a}/stop"), &cookie).await;
    assert_eq!(answer.status, 500, "{answer:?}");
    assert!(answer.body["error"].as_str().unwrap().starts_with("the stop holds, but was not saved"), "{answer:?}");
    assert_eq!(answer.body["code"], "stop_unsaved");
    assert_eq!(running.get(&format!("/api/accounts/{a}"), &cookie).await.body["state"], "stopped");

    stand_in.tick(T0 + MINUTE);
    let answer = running.post(&format!("/api/accounts/{a}/flatten"), &cookie).await;
    assert_eq!(answer.status, 500, "{answer:?}");
    let error = answer.body["error"].as_str().unwrap();
    assert!(error.starts_with("the account is flattened; the stop holds, but was not saved"), "{error}");
    assert_eq!(answer.body["code"], "flattened_stop_unsaved");

    std::fs::set_permissions(&accounts, std::fs::Permissions::from_mode(0o700)).unwrap();
    running.stop_after(&stand_in).await.unwrap();
}

/// A stopped strategy places no order while its account runs; started again, it orders. Only the
/// account's strategies can be stopped.
#[tokio::test]
async fn a_stopped_strategy_places_no_order_until_started() {
    let stand_in = StandIn::start().await;
    seed(&stand_in);
    let dir = tempfile::tempdir().unwrap();
    let running = Running::start(console_config(&stand_in, dir.path(), "")).await;
    let cookie = running.session().await;
    until_started(&running, &cookie).await;
    let a = account().to_string();
    let missing = running.post(&format!("/api/accounts/{a}/strategies/1/stop"), &cookie).await;
    assert_eq!((missing.status, &missing.body["error"]), (404, &json!("the account has no strategy 1")));
    assert_eq!(missing.body["code"], "no_strategy");
    assert_eq!(running.post(&format!("/api/accounts/{a}/strategies/0/stop"), &cookie).await.status, 204);
    let view = running.get(&format!("/api/accounts/{a}"), &cookie).await.body;
    assert_eq!((&view["state"], &view["strategies"][0]["running"]), (&json!("running"), &json!(false)));

    within(warm(&stand_in)).await;
    price(&stand_in, 14, 1, "101").await;
    price(&stand_in, 15, 2, "102").await;
    assert_eq!(posted(&stand_in), Vec::<String>::new(), "a stopped strategy orders nothing");
    assert_eq!(running.post(&format!("/api/accounts/{a}/strategies/0/start"), &cookie).await.status, 204);
    price(&stand_in, 16, 3, "103").await;
    assert_eq!(posted(&stand_in), ["/v1/orders"]);
    running.stop_after(&stand_in).await.unwrap();
}

/// Parameters change within the config's bounds only, and the series warms up again from the marks
/// history; a config without bounds lets none change.
#[tokio::test]
async fn periods_change_within_the_bounds_only() {
    let stand_in = StandIn::start().await;
    seed(&stand_in);
    let dir = tempfile::tempdir().unwrap();
    let running = Running::start(console_config(&stand_in, dir.path(), BOUNDS)).await;
    let cookie = running.session().await;
    until_started(&running, &cookie).await;
    within(warm(&stand_in)).await;
    let a = account().to_string();
    let path = format!("/api/accounts/{a}/strategies/0/params");
    let set = |body: Value| running.call(Method::PUT, &path, SAME, Some(&cookie), Some(body));

    let sma = |fast: Value, slow: Value| json!({ "kind": "sma_crossover", "fast": fast, "slow": slow });
    for (body, code, error) in [
        (sma(json!(1), json!(60)), "out_of_bounds", "slow must lie within [2, 50]"),
        (sma(json!(6), json!(10)), "out_of_bounds", "fast must lie within [1, 5]"),
        (sma(json!(3), json!(3)), "params_invalid", "the fast period must be at least 1 and below the slow period"),
        (json!({ "kind": "macd", "fast": 2, "slow": 3 }), "kind_mismatch", "the strategy is \"sma_crossover\""),
        (json!({ "kind": "sma_crossover", "fast": 2 }), "params_incomplete", "give the kind and every parameter"),
        (
            json!({ "kind": "sma_crossover", "fast": 2, "slow": 3, "bar": "1h" }),
            "params_incomplete",
            "give the kind and every parameter",
        ),
        (sma(json!(-1), json!(3)), "params_invalid", "invalid value"),
        (sma(json!("2"), json!(3)), "params_invalid", "invalid type"),
    ] {
        let answer = set(body.clone()).await;
        assert_eq!((answer.status, &answer.body["code"]), (422, &json!(code)), "{body}: {answer:?}");
        assert!(answer.body["error"].as_str().unwrap().starts_with(error), "{body}: {answer:?}");
    }
    let answer = set(json!("not an object")).await;
    assert_eq!((answer.status, &answer.body["code"]), (422, &json!("kind_mismatch")), "{answer:?}");
    let view = running.get(&format!("/api/accounts/{a}"), &cookie).await.body;
    assert_eq!(
        view["strategies"][0]["params"],
        json!({ "kind": "sma_crossover", "fast": 1, "slow": 2 }),
        "a refusal changes nothing"
    );
    assert_eq!(view["strategies"][0]["bounds"], json!({ "fast": [1, 5], "slow": [2, 50] }));

    until_account(&running, &cookie, &a, |view| view["strategies"][0]["warmed_up"] == true).await;
    let reads = stand_in.with(|i| i.marks_reads);
    assert_eq!(set(json!({ "kind": "sma_crossover", "fast": 2, "slow": 3 })).await.status, 204);
    let view = running.get(&format!("/api/accounts/{a}"), &cookie).await.body;
    assert_eq!(view["strategies"][0]["params"], json!({ "kind": "sma_crossover", "fast": 2, "slow": 3 }));
    // The new parameters warm up again from the node's marks history, and decide nothing meanwhile.
    assert_eq!(view["strategies"][0]["warmed_up"], false);
    price(&stand_in, 14, 1, "101").await;
    stand_in.append((15, 0), command(15, T0 + MINUTE + 6_000, json!({ "type": "tick" }), json!([])));
    stand_in.applied((15, 0)).await;
    until_account(&running, &cookie, &a, |view| view["strategies"][0]["warmed_up"] == true).await;
    assert!(stand_in.with(|i| i.marks_reads) > reads, "the history was read again");
    let missing = running
        .call(
            Method::PUT,
            &format!("/api/accounts/{a}/strategies/1/params"),
            SAME,
            Some(&cookie),
            Some(json!({ "kind": "sma_crossover", "fast": 2, "slow": 3 })),
        )
        .await;
    assert_eq!(missing.status, 404);
    running.stop_after(&stand_in).await.unwrap();

    // Without bounds the config's periods hold.
    let stand_in = StandIn::start().await;
    seed(&stand_in);
    let dir = tempfile::tempdir().unwrap();
    let running = Running::start(console_config(&stand_in, dir.path(), "")).await;
    let cookie = running.session().await;
    until_started(&running, &cookie).await;
    let answer = running
        .call(Method::PUT, &path, SAME, Some(&cookie), Some(json!({ "kind": "sma_crossover", "fast": 1, "slow": 3 })))
        .await;
    assert_eq!((answer.status, &answer.body["error"]), (422, &json!("slow is not one the console may set")));
    assert_eq!(answer.body["code"], "read_only");
    running.stop_after(&stand_in).await.unwrap();
}

/// A restart keeps what the console set: the stop and the periods. The certificate is the same,
/// and the password is not printed again.
#[tokio::test]
async fn a_restart_keeps_the_controls() {
    let stand_in = StandIn::start().await;
    seed(&stand_in);
    let dir = tempfile::tempdir().unwrap();
    let running = Running::start(console_config(&stand_in, dir.path(), BOUNDS)).await;
    let password = printed_password(&running.printed);
    let fingerprint = running.printed.lines().next().unwrap().to_owned();
    let cookie = running.session().await;
    until_started(&running, &cookie).await;
    let a = account().to_string();
    assert_eq!(running.post(&format!("/api/accounts/{a}/stop"), &cookie).await.status, 204);
    let set = running
        .call(
            Method::PUT,
            &format!("/api/accounts/{a}/strategies/0/params"),
            SAME,
            Some(&cookie),
            Some(json!({ "kind": "sma_crossover", "fast": 2, "slow": 3 })),
        )
        .await;
    assert_eq!(set.status, 204);
    running.stop_after(&stand_in).await.unwrap();

    let again = Running::start(console_config(&stand_in, dir.path(), BOUNDS)).await;
    assert!(!again.printed.contains("console password"), "{}", again.printed);
    assert!(fingerprint.ends_with(again.printed.lines().next().unwrap().rsplit(' ').next().unwrap()));
    assert_eq!(again.get("/api/accounts", &cookie).await.status, 401, "sessions end with the process");
    let (answer, cookie) = again.login(&password).await;
    assert_eq!(answer.status, 204);
    let view = until_started(&again, &cookie.unwrap()).await;
    assert_eq!(view["state"], "stopped");
    assert_eq!(view["strategies"][0]["params"], json!({ "kind": "sma_crossover", "fast": 2, "slow": 3 }));
    again.stop_after(&stand_in).await.unwrap();
}

/// A flatten stops the account and waits for the log to pass any order it may have signed, then
/// cancels its resting orders and closes its position in every market at the node's mark, signed
/// by the account, closing again while a close shrinks a position it did not take whole; it fails,
/// saying what is left, when the node leaves something open. No start goes through while it waits,
/// and the close's fill in the log is the account's own.
#[tokio::test]
async fn flatten_closes_every_position() {
    let stand_in = StandIn::start().await;
    seed(&stand_in);
    let a = account().to_string();
    let mut eth = btc_usd(Some("2000"));
    (eth["id"], eth["symbol"]) = (json!(2), json!("ETH-USD"));
    let positions = json!({ "1": { "qty": "0.5", "entry": "100" }, "2": { "qty": "-1", "entry": "2100" } });
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99")), eth]);
        let mut view = figures_view("1000", "1100", "50", positions.clone());
        view["orders"] = json!({ "9": resting_order(9) });
        i.accounts.insert(a.clone(), view);
    });
    let dir = tempfile::tempdir().unwrap();
    let running = Running::start(console_config_eth(&stand_in, dir.path(), "")).await;
    let cookie = running.session().await;
    until_started(&running, &cookie).await;

    let flatten = format!("/api/accounts/{a}/flatten");
    let (answer, ()) = tokio::join!(running.post(&flatten, &cookie), async {
        until_account(&running, &cookie, &a, |view| view["state"] == "flattening").await;
        let start = running.post(&format!("/api/accounts/{a}/start"), &cookie).await;
        assert_eq!((start.status, &start.body["code"]), (422, &json!("flatten_in_progress")), "{start:?}");
        stand_in.applied(stand_in.tick(T0 + 20_000)).await;
        assert!(posted(&stand_in).is_empty(), "nothing before the log passes the account's last order");
        stand_in.tick(T0 + MINUTE);
    });
    assert_eq!(answer.status, 204, "{answer:?}");
    assert_eq!(posted(&stand_in), ["/v1/orders/cancel-all", "/v1/positions/close-all"]);
    let body = stand_in.with(|i| i.posts[1].1.clone());
    assert_eq!(body["closes"], json!([{ "market": 1, "quoted": "99" }, { "market": 2, "quoted": "2000" }]));
    let close: CloseAllRequest = serde_json::from_value(body).unwrap();
    let auth = close.auth.clone().unwrap();
    assert_eq!(
        recover(&close_all_hash(&domain(), &close, auth.nonce, auth.expires_at), &auth.signature),
        Some(account())
    );
    assert_eq!(running.get(&format!("/api/accounts/{a}"), &cookie).await.body["state"], "stopped");

    // The close's fill comes through the log, from the close the flatten signed.
    let seq = stand_in.with(|i| i.log.last().map_or(i.head_seq, |((seq, _), _)| *seq) + 1);
    let evidence = AccessEvidence::Failed { reason: SourceFailure::NotConfigured };
    let cmd = serde_json::to_value(Command::AccessCloseAll { request: close, evidence }).unwrap();
    let fill = json!({ "type": "fill", "account": a, "order_id": seq, "market": 1, "side": "sell", "qty": "0.5",
        "price": "99", "index": "99", "spread_bps": 0, "realised_pnl": "-0.5", "profit_fee": "0", "position": "0",
        "liquidation": false });
    stand_in.append((seq, 0), command(seq, T0 + MINUTE + 1_000, cmd, json!([fill])));
    stand_in.applied((seq, 0)).await;
    let view = running.get(&format!("/api/accounts/{a}"), &cookie).await.body;
    assert_eq!((&view["state"], &view["halt"]), (&json!("stopped"), &Value::Null), "{view}");

    // A close the node does not settle leaves the position, and the flatten says so.
    stand_in.with(|i| {
        i.settle = false;
        i.accounts.get_mut(&a).unwrap()["positions"] = positions;
    });
    let answer = running.post(&format!("/api/accounts/{a}/flatten"), &cookie).await;
    assert_eq!(answer.status, 500, "{answer:?}");
    let error = answer.body["error"].as_str().unwrap();
    assert!(error.contains("in market 1") && error.contains("in market 2") && error.ends_with("are left"), "{error}");
    assert_eq!(answer.body["code"], "flatten_incomplete");

    // A position larger than one close takes is closed again while each close shrinks it.
    stand_in.with(|i| {
        i.settle = true;
        i.close_cap = Some("1".parse().unwrap());
        i.accounts.get_mut(&a).unwrap()["positions"] = json!({ "1": { "qty": "-2.5", "entry": "100" } });
    });
    let closes = posted(&stand_in).len();
    let answer = running.post(&format!("/api/accounts/{a}/flatten"), &cookie).await;
    assert_eq!(answer.status, 204, "{answer:?}");
    assert_eq!(posted(&stand_in)[closes..], ["/v1/positions/close-all"; 3]);
    running.stop_after(&stand_in).await.unwrap();
}

/// A resting order in a market the account held nothing in can fill before the flatten's cancel:
/// the flatten closes that position too.
#[tokio::test]
async fn flatten_closes_a_position_opened_before_its_cancel() {
    let stand_in = StandIn::start().await;
    seed(&stand_in);
    let a = account().to_string();
    let mut eth = btc_usd(Some("2000"));
    (eth["id"], eth["symbol"]) = (json!(2), json!("ETH-USD"));
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99")), eth]);
        let mut view = figures_view("1000", "1100", "50", json!({ "1": { "qty": "0.5", "entry": "100" } }));
        view["orders"] = json!({ "9": resting_order(9) });
        i.accounts.insert(a.clone(), view);
        i.filled_before_cancel = Some(("2".into(), json!({ "qty": "-1", "entry": "2100" })));
    });
    let dir = tempfile::tempdir().unwrap();
    let running = Running::start(console_config_eth(&stand_in, dir.path(), "")).await;
    let cookie = running.session().await;
    until_started(&running, &cookie).await;

    stand_in.tick(T0 + MINUTE);
    let answer = running.post(&format!("/api/accounts/{a}/flatten"), &cookie).await;
    assert_eq!(answer.status, 204, "{answer:?}");
    assert_eq!(posted(&stand_in), ["/v1/orders/cancel-all", "/v1/positions/close-all", "/v1/positions/close-all"]);
    let closes: Vec<_> = stand_in.with(|i| i.posts[1..].iter().map(|(_, body)| body["closes"].clone()).collect());
    assert_eq!(closes, [json!([{ "market": 1, "quoted": "99" }]), json!([{ "market": 2, "quoted": "2000" }])]);
    running.stop_after(&stand_in).await.unwrap();
}

/// A strategy of another kind takes its parameters as the config writes them, whole numbers as
/// numbers and decimals as strings, each within its own bounds, and the series warms up again
/// from the marks history on the change.
#[tokio::test]
async fn another_kinds_parameters_change_as_the_config_writes_them() {
    let stand_in = StandIn::start().await;
    seed(&stand_in);
    let dir = tempfile::tempdir().unwrap();
    let edits =
        [("kind = \"sma_crossover\"\nfast = 1\nslow = 2", "kind = \"bollinger_reversion\"\nperiod = 3\nwidth = \"2\"")];
    let bounds = "bounds = { period = [2, 6], width = [\"1.5\", \"3\"] }\n";
    let running = Running::start(console_config_edited(&stand_in, dir.path(), bounds, &edits)).await;
    let cookie = running.session().await;
    let view = until_started(&running, &cookie).await;
    let a = account().to_string();
    let was = json!({ "kind": "bollinger_reversion", "period": 3, "width": "2" });
    assert_eq!(
        (&view["strategies"][0]["kind"], &view["strategies"][0]["params"]),
        (&json!("bollinger_reversion"), &was)
    );
    assert_eq!(view["strategies"][0]["bounds"], json!({ "period": [2, 6], "width": ["1.5", "3"] }));
    let path = format!("/api/accounts/{a}/strategies/0/params");
    let set = |body: Value| running.call(Method::PUT, &path, SAME, Some(&cookie), Some(body));

    let wider = json!({ "kind": "bollinger_reversion", "period": 4, "width": "2.5" });
    assert_eq!(set(wider.clone()).await.status, 204);
    let view = running.get(&format!("/api/accounts/{a}"), &cookie).await.body;
    assert_eq!(view["strategies"][0]["params"], wider);
    assert_eq!(view["strategies"][0]["warmed_up"], false, "the new strategy warms up before it decides");
    for (body, code) in [
        (json!({ "kind": "bollinger_reversion", "period": 4, "width": "3.5" }), "out_of_bounds"),
        (json!({ "kind": "bollinger_reversion", "period": 7, "width": "2.5" }), "out_of_bounds"),
        (json!({ "kind": "bollinger_reversion", "period": 4, "width": "wide" }), "params_invalid"),
        (json!({ "kind": "bollinger_reversion", "period": 4 }), "params_incomplete"),
    ] {
        let answer = set(body.clone()).await;
        assert_eq!((answer.status, &answer.body["code"]), (422, &json!(code)), "{body}: {answer:?}");
    }
    running.stop_after(&stand_in).await.unwrap();
}

/// An account the daily loss limit stopped shows when the stop ends and what the day has lost.
#[tokio::test]
async fn the_daily_loss_stop_shows_in_the_status() {
    let stand_in = StandIn::start().await;
    seed(&stand_in);
    let a = account().to_string();
    stand_in.with(|i| {
        let view = figures_view("1000", "999.5", "5", json!({ "1": { "qty": "0.5", "entry": "100" } }));
        i.accounts.insert(a.clone(), view);
    });
    let dir = tempfile::tempdir().unwrap();
    let edits = [("daily_loss = { usd = \"100000\" }", "daily_loss = { usd = \"5\" }")];
    let running = Running::start(console_config_edited(&stand_in, dir.path(), "", &edits)).await;
    let cookie = running.session().await;
    let view = until_started(&running, &cookie).await;
    assert_eq!(view["day_stop"], Value::Null);
    within(warm(&stand_in)).await;
    // The mark falls: the open position loses ten, past the limit of five.
    price(&stand_in, 14, 1, "80").await;
    let view = until_account(&running, &cookie, &a, |view| view["day_stop"] != Value::Null).await;
    let until = (T0 / 86_400_000 + 1) * 86_400_000;
    assert_eq!(view["day_stop"], json!({ "until_ms": until, "loss": "9.5" }), "{view}");
    assert_eq!(view["state"], "running", "the account itself is not stopped, its limit is");
    running.stop_after(&stand_in).await.unwrap();
}

/// A position in a market the account does not trade, opened by a resting order that filled before
/// the flatten's cancel, is no position the flatten can close: it fails, saying what is left.
#[tokio::test]
async fn flatten_reports_a_position_in_a_market_it_does_not_trade() {
    let stand_in = StandIn::start().await;
    seed(&stand_in);
    let a = account().to_string();
    let mut eth = btc_usd(Some("2000"));
    (eth["id"], eth["symbol"]) = (json!(2), json!("ETH-USD"));
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99")), eth]);
        let mut view = figures_view("1000", "1000", "0", json!({}));
        view["orders"] = json!({ "9": resting_order(9) });
        i.accounts.insert(a.clone(), view);
        i.filled_before_cancel = Some(("2".into(), json!({ "qty": "-1", "entry": "2100" })));
    });
    let dir = tempfile::tempdir().unwrap();
    let running = Running::start(console_config(&stand_in, dir.path(), "")).await;
    let cookie = running.session().await;
    until_started(&running, &cookie).await;

    stand_in.tick(T0 + MINUTE);
    let answer = running.post(&format!("/api/accounts/{a}/flatten"), &cookie).await;
    assert_eq!((answer.status, &answer.body["code"]), (500, &json!("flatten_incomplete")), "{answer:?}");
    assert!(answer.body["error"].as_str().unwrap().contains("in market 2"), "{answer:?}");
    running.stop_after(&stand_in).await.ok();
}
