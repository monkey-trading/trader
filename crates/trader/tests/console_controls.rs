//! The console's controls, against the stand-in: a stopped account or strategy places no order
//! until it starts again, and stays stopped across a restart; a strategy's periods change only
//! within the config's bounds; and a flatten, once the log has passed the account's last order,
//! cancels its resting orders and closes every position, each request signed by the account.

mod common;

use common::console::{
    BOUNDS, Running, SAME, console_config, figures_view, printed_password, seed, until_account, until_started, within,
};
use common::{MINUTE, StandIn, T0, account, btc_usd, command, domain, prices, resting_order, warm};
use engine_core::command::CloseAllRequest;
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

/// Periods change within the config's bounds only, and the strategy warms up again on them; a
/// config without bounds lets none change.
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

    for (body, error) in [
        (json!({ "fast": 1, "slow": 60 }), "the fast period must lie within [1, 5] and the slow within [2, 50]"),
        (json!({ "fast": 6, "slow": 10 }), "the fast period must lie within [1, 5] and the slow within [2, 50]"),
        (json!({ "fast": 3, "slow": 3 }), "the fast period must be at least 1 and below the slow period"),
    ] {
        let answer = set(body.clone()).await;
        assert_eq!(answer.status, 422, "{body}: {answer:?}");
        assert!(answer.body["error"].as_str().unwrap().starts_with(error), "{body}: {answer:?}");
    }
    for body in [json!({ "fast": 2 }), json!({ "fast": 2, "slow": 3, "bar": "1h" }), json!({ "fast": -1, "slow": 3 })] {
        assert_eq!(set(body.clone()).await.status, 422, "{body}");
    }
    let view = running.get(&format!("/api/accounts/{a}"), &cookie).await.body;
    assert_eq!(view["strategies"][0]["params"], json!({ "fast": 1, "slow": 2 }), "a refusal changes nothing");
    assert_eq!(view["strategies"][0]["bounds"], json!({ "fast": [1, 5], "slow": [2, 50] }));

    let reads = stand_in.with(|i| i.marks_reads);
    assert_eq!(set(json!({ "fast": 2, "slow": 3 })).await.status, 204);
    let view = running.get(&format!("/api/accounts/{a}"), &cookie).await.body;
    assert_eq!(view["strategies"][0]["params"], json!({ "fast": 2, "slow": 3 }));
    assert_eq!(view["strategies"][0]["warmed_up"], false);
    // Warming up again reads the marks history once the index has been exact a while.
    price(&stand_in, 14, 1, "101").await;
    stand_in.append((15, 0), command(15, T0 + MINUTE + 6_000, json!({ "type": "tick" }), json!([])));
    stand_in.applied((15, 0)).await;
    assert_eq!(stand_in.with(|i| i.marks_reads), reads + 1);
    until_account(&running, &cookie, &a, |view| view["strategies"][0]["warmed_up"] == true).await;
    let missing = running
        .call(
            Method::PUT,
            &format!("/api/accounts/{a}/strategies/1/params"),
            SAME,
            Some(&cookie),
            Some(json!({ "fast": 2, "slow": 3 })),
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
    let answer = running.call(Method::PUT, &path, SAME, Some(&cookie), Some(json!({ "fast": 1, "slow": 3 }))).await;
    assert_eq!((answer.status, &answer.body["error"]), (422, &json!("the config sets no bounds for the periods")));
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
            Some(json!({ "fast": 2, "slow": 3 })),
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
    assert_eq!(view["strategies"][0]["params"], json!({ "fast": 2, "slow": 3 }));
    again.stop_after(&stand_in).await.unwrap();
}

/// A flatten stops the account and waits for the log to pass any order it may have signed, then
/// cancels its resting orders and closes its position in every market at the node's mark, signed
/// by the account; it fails, saying what is left, when the node leaves something open.
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
    let running = Running::start(console_config(&stand_in, dir.path(), "")).await;
    let cookie = running.session().await;
    until_started(&running, &cookie).await;

    let flatten = format!("/api/accounts/{a}/flatten");
    let (answer, ()) = tokio::join!(running.post(&flatten, &cookie), async {
        until_account(&running, &cookie, &a, |view| view["state"] == "flattening").await;
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

    // A close the node does not settle leaves the position, and the flatten says so.
    stand_in.with(|i| {
        i.settle = false;
        i.accounts.get_mut(&a).unwrap()["positions"] = positions;
    });
    let answer = running.post(&format!("/api/accounts/{a}/flatten"), &cookie).await;
    assert_eq!(answer.status, 500, "{answer:?}");
    assert_eq!(answer.body["error"], "0 resting orders and positions in 2 markets are left");
    stand_in.with(|i| {
        i.settle = true;
        i.accounts.get_mut(&a).unwrap()["positions"] = json!({});
    });
    running.stop_after(&stand_in).await.unwrap();
}
