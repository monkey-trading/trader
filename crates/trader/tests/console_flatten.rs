//! A console flatten against the stand-in, in the cases a flatten of many closes brings: fills that
//! come after every close was sent, an account that halts while it runs, and a stop that comes while
//! it is still reporting its closes; and how its closes count in the metrics.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::console::{Running, console_config, figures_view, seed, until_account, until_started, within};
use common::{MINUTE, StandIn, T0, account, address, command, config_many};
use engine_core::access::{AccessEvidence, SourceFailure};
use engine_core::command::{CloseAllRequest, Command};
use serde_json::{Value, json};
use trader::metrics::Metrics;

/// The console section of these tests' configs: a free loopback port, data beside the config.
const CONSOLE: &str = "\n[console]\nbind = \"127.0.0.1:0\"\ndata_dir = \"data\"\nrefresh_interval = \"1s\"\n";

/// The paths of the requests the bot sent the node.
fn posted(stand_in: &StandIn) -> Vec<String> {
    stand_in.with(|i| i.posts.iter().map(|(path, _)| path.clone()).collect())
}

/// An account holding `qty` BTC-USD, and a node that closes at most one unit a request.
fn holding(stand_in: &StandIn, address: &str, qty: &str) {
    stand_in.with(|i| {
        i.close_cap = Some("1".parse().unwrap());
        let view = figures_view("1000", "1000", "0", json!({ "1": { "qty": qty, "entry": "100" } }));
        i.accounts.insert(address.to_owned(), view);
    });
}

/// A position that takes six closes is flattened before any of their fills reaches the log: every
/// close's signature is kept until its fill has come, so no fill reads as another's and halts the account.
#[tokio::test]
async fn every_close_of_a_long_flatten_is_known_when_its_fill_arrives() {
    let stand_in = StandIn::start().await;
    seed(&stand_in);
    let a = account().to_string();
    holding(&stand_in, &a, "6");
    let dir = tempfile::tempdir().unwrap();
    let running = Running::start(console_config(&stand_in, dir.path(), "")).await;
    let cookie = running.session().await;
    until_started(&running, &cookie).await;

    // The node's time is the log's, so each close expires thirty seconds after the log's minute.
    stand_in.with(|i| i.time_ms = (T0 + MINUTE) as u64);
    stand_in.tick(T0 + MINUTE);
    let answer = running.post(&format!("/api/accounts/{a}/flatten"), &cookie).await;
    assert_eq!(answer.status, 204, "{answer:?}");
    let closes: Vec<Value> = stand_in
        .with(|i| i.posts.iter().filter(|(path, _)| path.ends_with("close-all")).map(|(_, b)| b.clone()).collect());
    assert_eq!(closes.len(), 6, "one unit a close");

    // The fills come now, in the order the closes were sent, after all of them.
    for (n, body) in closes.into_iter().enumerate() {
        let seq = stand_in.with(|i| i.log.last().map_or(i.head_seq, |((seq, _), _)| *seq) + 1);
        let request: CloseAllRequest = serde_json::from_value(body).unwrap();
        let evidence = AccessEvidence::Failed { reason: SourceFailure::NotConfigured };
        let cmd = serde_json::to_value(Command::AccessCloseAll { request, evidence }).unwrap();
        let fill = json!({ "type": "fill", "account": a, "order_id": seq, "market": 1, "side": "sell", "qty": "1",
            "price": "99", "index": "99", "spread_bps": 0, "realised_pnl": "-1", "profit_fee": "0",
            "position": (5 - n).to_string(), "liquidation": false });
        stand_in.append((seq, 0), command(seq, T0 + MINUTE + (n as i64 + 1) * 1_000, cmd, json!([fill])));
        stand_in.applied((seq, 0)).await;
    }
    let view = running.get(&format!("/api/accounts/{a}"), &cookie).await.body;
    assert_eq!((&view["state"], &view["halt"]), (&json!("stopped"), &Value::Null), "{view}");
    running.stop_after(&stand_in).await.ok();
}

/// A flatten's close counts as placed; the node's refusal of one as declined, from its answer; and the
/// engine's rejection of one once, as the log gives it, though its answer gave it too.
#[tokio::test]
async fn a_flattens_closes_count_once_in_the_metrics() {
    let stand_in = StandIn::start().await;
    seed(&stand_in);
    let a = account().to_string();
    holding(&stand_in, &a, "1");
    let rejected = json!([{ "type": "order_rejected", "account": a, "client_id": 0,
        "reason": { "kind": "market_closed" } }]);
    stand_in.with(|i| {
        // Neither close moves the position: the node refuses the first, and the engine rejects the second.
        i.settle = false;
        i.statuses.push_back(400);
        i.replies.push_back(rejected.clone());
    });
    let dir = tempfile::tempdir().unwrap();
    let metrics = Arc::new(Metrics::new());
    let running = Running::start_recording(console_config(&stand_in, dir.path(), ""), Arc::clone(&metrics)).await;
    let cookie = running.session().await;
    until_started(&running, &cookie).await;

    let flatten = format!("/api/accounts/{a}/flatten");
    for minute in [1, 2] {
        // The second flatten waits for the log to pass the first close's expiry.
        stand_in.with(|i| i.time_ms = (T0 + minute * MINUTE) as u64);
        stand_in.tick(T0 + minute * MINUTE);
        let answer = running.post(&flatten, &cookie).await;
        assert_eq!(answer.body["code"], "flatten_incomplete", "{answer:?}");
    }
    assert_eq!(posted(&stand_in), ["/v1/positions/close-all"; 2]);
    let body = stand_in.with(|i| i.posts.last().unwrap().1.clone());
    let request: CloseAllRequest = serde_json::from_value(body).unwrap();
    let evidence = AccessEvidence::Failed { reason: SourceFailure::NotConfigured };
    let cmd = serde_json::to_value(Command::AccessCloseAll { request, evidence }).unwrap();
    let seq = stand_in.with(|i| i.log.last().map_or(i.head_seq, |((seq, _), _)| *seq) + 1);
    stand_in.append((seq, 0), command(seq, T0 + 2 * MINUTE + 1_000, cmd, rejected));
    stand_in.applied((seq, 0)).await;
    let text = metrics.encode();
    for (name, count) in [("placed", 2), ("declined", 2)] {
        let line = format!("trader_orders_{name}_total{{account=\"{a}\",market=\"BTC-USD\"}} {count}");
        assert!(text.lines().any(|l| l == line), "no {line} in\n{text}");
    }
    running.stop_after(&stand_in).await.ok();
}

/// An account that halts while its flatten is closing it is closed no further: the next close is not
/// signed, and the flatten answers that the account halted.
#[tokio::test]
async fn a_flatten_under_way_signs_nothing_more_once_its_account_halts() {
    let stand_in = StandIn::start().await;
    seed(&stand_in);
    let (first, second) = (address(0).to_string(), address(1).to_string());
    holding(&stand_in, &first, "3");
    stand_in.with(|i| {
        i.accounts.insert(second.clone(), figures_view("1000", "1000", "0", json!({})));
        // The first close's answer is held, so the account halts while it is out.
        i.post_delay = Some(Duration::from_secs(2));
    });
    let dir = tempfile::tempdir().unwrap();
    let running = Running::start(config_many(&stand_in, dir.path(), "1m", &[0, 1], CONSOLE)).await;
    let cookie = running.session().await;
    until_account(&running, &cookie, &first, |view| view["state"] == "running").await;

    stand_in.tick(T0 + MINUTE);
    let flatten = format!("/api/accounts/{first}/flatten");
    let (answer, ()) = tokio::join!(running.post(&flatten, &cookie), async {
        within(async {
            while posted(&stand_in).is_empty() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        // A fill of an order this bot did not sign halts the account.
        let seq = stand_in.with(|i| i.log.last().map_or(i.head_seq, |((seq, _), _)| *seq) + 1);
        let fill = json!({ "type": "fill", "account": first, "order_id": 77, "market": 1, "side": "buy", "qty": "1",
            "price": "99", "index": "99", "spread_bps": 0, "realised_pnl": "0", "profit_fee": "0",
            "position": "4", "liquidation": false });
        stand_in.append((seq, 0), command(seq, T0 + 2 * MINUTE, json!({ "type": "tick" }), json!([fill])));
        stand_in.applied((seq, 0)).await;
    });
    assert_eq!((answer.status, &answer.body["code"]), (422, &json!("account_halted")), "{answer:?}");
    assert_eq!(posted(&stand_in).len(), 1, "no close after the halt: {:?}", posted(&stand_in));
    running.stop_after(&stand_in).await.ok();
}

/// A stop that comes while a flatten of more closes than the bot holds reports for is still sending
/// them: the stop takes the reports in as they come, so the flatten, and the stop, end.
#[tokio::test]
async fn a_stop_does_not_wait_for_a_flatten_that_waits_to_report() {
    let stand_in = StandIn::start().await;
    seed(&stand_in);
    let a = account().to_string();
    holding(&stand_in, &a, "100");
    let dir = tempfile::tempdir().unwrap();
    let mut running = Running::start(console_config(&stand_in, dir.path(), "")).await;
    let cookie = running.session().await;
    until_started(&running, &cookie).await;

    stand_in.tick(T0 + MINUTE);
    running.post_in_background(&format!("/api/accounts/{a}/flatten"), &cookie);
    // No frame arrives: the flatten's reports fill the channel, and it waits for room.
    within(async {
        while posted(&stand_in).len() < 64 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    running.signal_stop();
    // The log moves on past every close's expiry, as it must for the stop to go on.
    tokio::time::sleep(Duration::from_millis(500)).await;
    stand_in.tick(T0 + 10 * MINUTE);
    let ended = tokio::time::timeout(Duration::from_secs(60), &mut running.task).await;
    // Whatever the stop made of the account, it ended.
    assert!(ended.is_ok(), "the stop never ended");
}
