//! The console over many accounts, against the stand-in: it lists every account the bot trades, in
//! the config's order, each with its own controls, and a stop of one leaves the others trading.

mod common;

use common::console::{Running, figures_view, seed, until_account, within};
use common::{MINUTE, StandIn, T0, address, command, config_many, posted, prices, warm};
use hyper::Method;
use serde_json::json;
use trader::config::Config;

/// The console section of these tests' configs: a free loopback port, data beside the config.
const CONSOLE: &str = "\n[console]\nbind = \"127.0.0.1:0\"\ndata_dir = \"data\"\nrefresh_interval = \"1s\"\n";

/// Two accounts trade the same bars; the console stops one, and only the other orders when the
/// crossover turns, though both decide on it.
#[tokio::test]
async fn one_accounts_stop_leaves_the_others_trading() {
    let stand_in = StandIn::start().await;
    seed(&stand_in);
    let (first, second) = (address(0).to_string(), address(1).to_string());
    stand_in.with(|i| {
        i.accounts.insert(second.clone(), figures_view("1000", "1000", "0", json!({})));
    });
    let dir = tempfile::tempdir().unwrap();
    let running = Running::start(config_many(&stand_in, dir.path(), "1m", &[0, 1], CONSOLE)).await;
    let cookie = running.session().await;
    let overview = running.get("/api/accounts", &cookie).await.body;
    let listed: Vec<_> = overview["accounts"].as_array().unwrap().iter().map(|a| a["address"].clone()).collect();
    assert_eq!(listed, [json!(first), json!(second)], "every account, in the config's order");
    assert_eq!(overview["totals"]["accounts"], 2);

    assert_eq!(running.post(&format!("/api/accounts/{second}/stop"), &cookie).await.status, 204);
    assert_eq!(running.get(&format!("/api/accounts/{second}"), &cookie).await.body["state"], "stopped");
    assert_eq!(running.get(&format!("/api/accounts/{first}"), &cookie).await.body["state"], "running");

    // The one-bar average rises above the two-bar one: both accounts' strategies turn long.
    within(warm(&stand_in)).await;
    for (seq, minute, mid) in [(14, 1, "101"), (15, 2, "102")] {
        let ts = T0 + minute * MINUTE;
        stand_in.append((seq, 0), command(seq, ts, prices(1, mid, ts), json!([])));
        stand_in.applied((seq, 0)).await;
    }
    let (path, order) = posted(&stand_in, 1).await;
    assert_eq!((path.as_str(), &order["account"]), ("/v1/orders", &json!(first)));
    until_account(&running, &cookie, &second, |view| view["strategies"][0]["target"] == "long").await;
    let orders: Vec<_> = stand_in.with(|i| i.posts.iter().map(|(_, body)| body["account"].clone()).collect());
    assert_eq!(orders, [json!(first)], "the stopped account decided, and placed nothing");
    running.stop_after(&stand_in).await.unwrap();
}

/// An account that could not start is listed in its place, halted, with why: one the node never
/// credited under its address, and one whose key will not unlock under the label of its key file.
/// Its controls do nothing, and the totals count it without figures.
#[tokio::test]
async fn accounts_that_could_not_start_are_listed_halted() {
    let stand_in = StandIn::start().await;
    seed(&stand_in);
    let dir = tempfile::tempdir().unwrap();
    let cfg = config_many(&stand_in, dir.path(), "1m", &[0, 1, 2], CONSOLE);
    // Nothing credited the second account at the node, and the third key file holds no key.
    std::fs::write(dir.path().join("account2.hex"), "no key").unwrap();
    let running = Running::start(cfg).await;
    let cookie = running.session().await;
    until_account(&running, &cookie, &address(0).to_string(), |view| view["state"] == "running").await;
    until_account(&running, &cookie, &address(0).to_string(), |view| view["figures"] != json!(null)).await;
    let overview = running.get("/api/accounts", &cookie).await.body;
    let accounts = overview["accounts"].as_array().unwrap();
    assert_eq!(accounts.len(), 3, "{overview}");
    assert_eq!((&accounts[0]["address"], &accounts[0]["state"]), (&json!(address(0).to_string()), &json!("running")));

    let credited = &accounts[1];
    assert_eq!(credited["address"], json!(address(1).to_string()));
    assert_eq!((&credited["state"], &credited["code"]), (&json!("halted"), &json!("no_balance")));
    assert!(credited["halt"].as_str().unwrap().contains("never credited"), "{credited}");
    assert_eq!(
        (&credited["figures"], &credited["strategies"], &credited["label"]),
        (&json!(null), &json!([]), &json!(null))
    );

    let locked = &accounts[2];
    assert_eq!(
        (&locked["address"], &locked["state"], &locked["code"]),
        (&json!(null), &json!("halted"), &json!("key_locked"))
    );
    assert!(locked["label"].as_str().unwrap().ends_with("account2.hex"), "{locked}");
    assert_eq!(locked["error"], locked["halt"]);
    assert_eq!((&overview["totals"]["accounts"], &overview["totals"]["reporting"]), (&json!(3), &json!(1)));

    let second = address(1).to_string();
    for action in ["start", "stop", "flatten"] {
        let answer = running.post(&format!("/api/accounts/{second}/{action}"), &cookie).await;
        assert_eq!((answer.status, &answer.body["code"]), (422, &json!("account_halted")), "{action}: {answer:?}");
    }
    let params = json!({ "kind": "sma_crossover", "fast": 1, "slow": 2 });
    let answer = running
        .call(
            Method::PUT,
            &format!("/api/accounts/{second}/strategies/0/params"),
            common::console::SAME,
            Some(&cookie),
            Some(params),
        )
        .await;
    assert_eq!((answer.status, &answer.body["code"]), (422, &json!("account_halted")), "{answer:?}");
    // The bot's exit status says that accounts could not start.
    let ended = running.stop_after(&stand_in).await.unwrap_err();
    assert!(ended.to_string().contains("2 accounts could not start"), "{ended:#}");
}

/// A key the config lists twice starts once: its address names the account that trades, which the
/// console reads and controls, and the second listing shows beside it as halted.
#[tokio::test]
async fn a_key_listed_twice_names_the_account_that_trades() {
    let stand_in = StandIn::start().await;
    seed(&stand_in);
    let dir = tempfile::tempdir().unwrap();
    let running = Running::start(config_many(&stand_in, dir.path(), "1m", &[0, 0], CONSOLE)).await;
    let cookie = running.session().await;
    let a = address(0).to_string();
    until_account(&running, &cookie, &a, |view| view["state"] == "running").await;
    let overview = running.get("/api/accounts", &cookie).await.body;
    let states: Vec<_> = overview["accounts"].as_array().unwrap().iter().map(|a| a["state"].clone()).collect();
    assert_eq!(states, [json!("running"), json!("halted")], "{overview}");
    assert_eq!(running.post(&format!("/api/accounts/{a}/stop"), &cookie).await.status, 204);
    assert_eq!(running.get(&format!("/api/accounts/{a}"), &cookie).await.body["state"], "stopped");
    let ended = running.stop_after(&stand_in).await.unwrap_err();
    assert!(ended.to_string().contains("1 accounts could not start"), "{ended:#}");
}

/// A flatten waiting for the log to pass the account's last order is refused when the account halts
/// meanwhile, while another account keeps the bot running: nothing is signed for a halted account.
#[tokio::test]
async fn a_waiting_flatten_is_refused_when_the_account_halts() {
    let stand_in = StandIn::start().await;
    seed(&stand_in);
    let (first, second) = (address(0).to_string(), address(1).to_string());
    stand_in.with(|i| {
        i.accounts.insert(second.clone(), figures_view("1000", "1000", "0", json!({})));
    });
    let dir = tempfile::tempdir().unwrap();
    let running = Running::start(config_many(&stand_in, dir.path(), "1m", &[0, 1], CONSOLE)).await;
    let cookie = running.session().await;
    until_account(&running, &cookie, &first, |view| view["state"] == "running").await;
    let flatten = format!("/api/accounts/{first}/flatten");
    let (answer, ()) = tokio::join!(running.post(&flatten, &cookie), async {
        until_account(&running, &cookie, &first, |view| view["state"] == "flattening").await;
        // A fill from an order this bot did not sign halts the account, in the same message that
        // takes the log past the order it waited for.
        let fill = json!({ "type": "fill", "account": first, "order_id": 77, "market": 1, "side": "buy",
            "qty": "1", "price": "99", "index": "99", "spread_bps": 0, "realised_pnl": "0", "profit_fee": "0",
            "position": "1", "liquidation": false });
        stand_in.append((11, 0), command(11, T0 + 40_000, json!({ "type": "tick" }), json!([fill])));
        stand_in.applied((11, 0)).await;
    });
    assert_eq!((answer.status, &answer.body["code"]), (422, &json!("account_halted")), "{answer:?}");
    assert_eq!(running.get(&format!("/api/accounts/{first}"), &cookie).await.body["state"], "halted");
    let posts: Vec<_> = stand_in.with(|i| i.posts.iter().map(|(path, _)| path.clone()).collect());
    assert!(posts.is_empty(), "nothing was signed: {posts:?}");
    running.stop_after(&stand_in).await.ok();
}

/// Two accounts read one series. Changing one's parameters builds its strategy afresh and warms the
/// series up again, so neither account decides until it has, but only the one asked keeps new
/// parameters.
#[tokio::test]
async fn a_parameter_change_warms_up_the_series_both_accounts_share() {
    let stand_in = StandIn::start().await;
    seed(&stand_in);
    let (first, second) = (address(0).to_string(), address(1).to_string());
    stand_in.with(|i| {
        i.accounts.insert(second.clone(), figures_view("1000", "1000", "0", json!({})));
    });
    let dir = tempfile::tempdir().unwrap();
    config_many(&stand_in, dir.path(), "1m", &[0, 1], CONSOLE);
    let path = dir.path().join("bot.toml");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        text.replace("max_slippage_bps = 25\n", "max_slippage_bps = 25\nbounds = { fast = [1, 5], slow = [2, 50] }\n"),
    )
    .unwrap();
    let running = Running::start(Config::load(&path).unwrap()).await;
    let cookie = running.session().await;
    until_account(&running, &cookie, &first, |view| view["state"] == "running").await;
    within(warm(&stand_in)).await;
    until_account(&running, &cookie, &first, |view| view["strategies"][0]["warmed_up"] == true).await;
    until_account(&running, &cookie, &second, |view| view["strategies"][0]["warmed_up"] == true).await;

    let body = json!({ "kind": "sma_crossover", "fast": 2, "slow": 3 });
    let set = running
        .call(
            Method::PUT,
            &format!("/api/accounts/{first}/strategies/0/params"),
            common::console::SAME,
            Some(&cookie),
            Some(body.clone()),
        )
        .await;
    assert_eq!(set.status, 204, "{set:?}");
    let asked = running.get(&format!("/api/accounts/{first}"), &cookie).await.body;
    let other = running.get(&format!("/api/accounts/{second}"), &cookie).await.body;
    assert_eq!(asked["strategies"][0]["params"], body);
    assert_eq!(other["strategies"][0]["params"], json!({ "kind": "sma_crossover", "fast": 1, "slow": 2 }));
    for view in [&asked, &other] {
        assert_eq!(view["strategies"][0]["warmed_up"], false, "{view}");
    }
    // Once the history is read again, both decide again, each on its own parameters.
    let ts = T0 + MINUTE;
    stand_in.append((14, 0), command(14, ts, prices(1, "101", ts), json!([])));
    stand_in.append((15, 0), command(15, ts + 6_000, json!({ "type": "tick" }), json!([])));
    stand_in.applied((15, 0)).await;
    for a in [&first, &second] {
        until_account(&running, &cookie, a, |view| view["strategies"][0]["warmed_up"] == true).await;
    }
    running.stop_after(&stand_in).await.unwrap();
}
