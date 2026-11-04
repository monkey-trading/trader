//! The bot against the stand-in when something goes wrong: a new listing of its market, a log
//! message it cannot read, a node that signs with another key mid-run, and a stop that cannot
//! confirm the account is left as it should be.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{
    MINUTE, StandIn, T0, account, account_view, btc_usd, bucket, command, config, posted, prices, resting_order, run,
    warm,
};
use engine_core::command::{AdminCommand, Command};
use engine_core::market::{MarketSpec, PriceRules, QuoteCurrency};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use trader::bot::Halt;

/// An applied listing's price rules take effect: with four sources needed, three venues give no
/// mark and nothing is ordered; back at two, the crossover orders again. A rejected listing changes
/// nothing, and an applied one the bot cannot follow halts it, after it cancels the resting order.
#[tokio::test]
async fn relistings_change_the_rules_or_halt() {
    let stand_in = StandIn::start().await;
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99"))]);
        i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({ "9": resting_order(9) })));
        i.head_seq = 10;
        i.marks = vec![bucket(T0, "100")];
    });
    let dir = tempfile::tempdir().unwrap();
    let (_stop, stopped) = oneshot::channel::<()>();
    let bot = tokio::spawn(run(config(&stand_in, dir.path(), "1m"), async {
        _ = stopped.await;
    }));
    let four = PriceRules { min_sources: 4, ..btc_spec().price_rules };
    stand_in.append((11, 0), command(11, T0, prices(1, "100", T0), json!([])));
    stand_in.append((12, 0), relisting(12, T0 + 1, MarketSpec { price_rules: four, ..btc_spec() }, true));
    stand_in.append((13, 0), command(13, T0 + 1_001, prices(1, "100", T0 + 1_001), json!([])));
    stand_in.append((14, 0), command(14, T0 + MINUTE, prices(1, "101", T0 + MINUTE), json!([])));
    stand_in.append((15, 0), command(15, T0 + 2 * MINUTE, prices(1, "102", T0 + 2 * MINUTE), json!([])));
    stand_in.append((16, 0), relisting(16, T0 + 2 * MINUTE + 1, btc_spec(), true));
    stand_in.append((17, 0), command(17, T0 + 3 * MINUTE, prices(1, "103", T0 + 3 * MINUTE), json!([])));
    // Under four sources the mark stays at minute 0's 100 until the rules are back, so the bars are
    // minutes 0 and 2, and the order goes out as minute 2's closes: under the first rules, minutes
    // 0 and 1 would have ordered as minute 1's closed.
    let (path, first) = posted(&stand_in, 1).await;
    assert_eq!((path.as_str(), &first["client_id"]), ("/v1/orders", &json!(T0 + 2 * MINUTE)), "{first}");

    stand_in.with(|i| i.time_ms = (T0 + 10 * MINUTE) as u64);
    let yen = MarketSpec { quote_currency: QuoteCurrency::Jpy { fx_market: 9 }, ..btc_spec() };
    stand_in.append((18, 0), relisting(18, T0 + 5 * MINUTE + 1, yen.clone(), false));
    stand_in.append((19, 0), relisting(19, T0 + 5 * MINUTE + 2, yen, true));
    let err = tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap_err();
    assert!(err.to_string().contains("not a USD market priced from spot quotes"), "{err:#}");
    // The halt leaves the account as it is: its resting order stays.
    let posts = stand_in.with(|i| i.posts.clone());
    assert_eq!(posts.iter().map(|(path, _)| path.as_str()).collect::<Vec<_>>(), ["/v1/orders"]);
}

/// A log message the pinned engine cannot read halts the bot, which leaves its resting order.
#[tokio::test]
async fn an_unreadable_message_halts_without_touching_the_account() {
    let stand_in = StandIn::start().await;
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99"))]);
        i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({ "9": resting_order(9) })));
        i.head_seq = 10;
    });
    let dir = tempfile::tempdir().unwrap();
    stand_in.append((11, 0), command(11, T0, json!({ "type": "tick" }), json!([{ "type": "no_such_event" }])));
    stand_in.append((12, 0), command(12, T0 + MINUTE, json!({ "type": "tick" }), json!([])));
    let err = run(config(&stand_in, dir.path(), "1m"), std::future::pending()).await.unwrap_err();
    assert!(format!("{err:#}").contains("the log message at 11:0"), "{err:#}");
    let posts = stand_in.with(|i| i.posts.clone());
    assert!(posts.is_empty(), "{posts:?}");
}

/// Once the node's answers are signed by another key mid-run, the bot halts at the next call and
/// sends nothing more, not even its stop's cancel.
#[tokio::test]
async fn another_signer_mid_run_halts_without_sending() {
    let stand_in = StandIn::start().await;
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99"))]);
        i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({ "9": resting_order(9) })));
        i.head_seq = 10;
    });
    let dir = tempfile::tempdir().unwrap();
    let bot = tokio::spawn(run(config(&stand_in, dir.path(), "1m"), std::future::pending()));
    // The stream is up once the first frame is asked for; the answers then change signer.
    while stand_in.with(|i| i.connections.is_empty()) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    stand_in.with(|i| i.answers = Some(common::Answers::OtherSigner));
    for (seq, ts, mid) in
        [(11, T0, "100"), (12, T0 + 1_001, "100"), (13, T0 + MINUTE, "101"), (14, T0 + 2 * MINUTE, "102")]
    {
        stand_in.append((seq, 0), command(seq, ts, prices(1, mid, ts), json!([])));
    }
    let err = tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap_err();
    assert!(err.to_string().contains("not signed by the configured signer"), "{err:#}");
    assert!(stand_in.with(|i| i.posts.is_empty()));
}

/// A stream frame signed by another key halts the bot as a signer mismatch, not as an unreadable log,
/// while the node's answers stay the signer's, and nothing is sent, not even its stop's cancel.
#[tokio::test]
async fn a_frame_from_another_signer_halts_as_a_mismatch_without_sending() {
    let stand_in = StandIn::start().await;
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99"))]);
        i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({ "9": resting_order(9) })));
        i.head_seq = 10;
        i.faults.insert((0, (12, 0)), common::Fault::OtherSigner);
    });
    let dir = tempfile::tempdir().unwrap();
    for (seq, ts, mid) in [(11, T0, "100"), (12, T0 + 1_001, "100")] {
        stand_in.append((seq, 0), command(seq, ts, prices(1, mid, ts), json!([])));
    }
    let err = run(config(&stand_in, dir.path(), "1m"), std::future::pending()).await.unwrap_err();
    assert!(matches!(err.downcast_ref::<Halt>(), Some(Halt::WrongSigner)), "{err:#}");
    assert!(stand_in.with(|i| i.posts.is_empty()));
}

/// The same frame while the stop waits for the log to pass the last order's expiry halts the same
/// way, and the stop's cancel is not sent.
#[tokio::test]
async fn a_frame_from_another_signer_during_the_stop_halts_without_sending() {
    let stand_in = StandIn::start().await;
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99"))]);
        i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({ "9": resting_order(9) })));
        i.head_seq = 10;
        i.faults.insert((0, (11, 0)), common::Fault::OtherSigner);
    });
    let dir = tempfile::tempdir().unwrap();
    stand_in.tick(T0 + 20_000);
    let err = run(config(&stand_in, dir.path(), "1m"), async {}).await.unwrap_err();
    assert!(matches!(err.downcast_ref::<Halt>(), Some(Halt::WrongSigner)), "{err:#}");
    assert!(stand_in.with(|i| i.posts.is_empty()));
}

/// A stop that cannot read the account still cancels, then says it cannot confirm the result.
#[tokio::test]
async fn a_stop_without_the_account_cancels_and_is_incomplete() {
    let stand_in = StandIn::start().await;
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99"))]);
        i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({})));
    });
    let dir = tempfile::tempdir().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let bot = tokio::spawn(run(config(&stand_in, dir.path(), "1m"), async {
        _ = stopped.await;
    }));
    while stand_in.with(|i| i.connections.is_empty()) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    stand_in.with(|i| i.accounts.clear());
    stand_in.tick(T0 + MINUTE);
    stop.send(()).unwrap();
    let err = tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap_err();
    assert!(err.to_string().contains("the stop is incomplete"), "{err:#}");
    let posts = stand_in.with(|i| i.posts.clone());
    assert_eq!(posts.iter().map(|(path, _)| path.as_str()).collect::<Vec<_>>(), ["/v1/orders/cancel-all"]);
}

/// A close the engine declines leaves the position, and the stop says so.
#[tokio::test]
async fn a_declined_close_is_an_incomplete_stop() {
    let stand_in = StandIn::start().await;
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99"))]);
        let position = json!({ "1": { "qty": "1", "entry": "99" } });
        i.accounts.insert(account().to_string(), account_view("1000", position, json!({})));
        i.settle = false;
        i.replies.push_back(json!([{ "type": "order_rejected", "account": account(), "client_id": 0,
            "reason": { "kind": "slippage", "quoted": "99", "live": "90", "slippage_bps": 909 } }]));
    });
    stand_in.tick(T0 + MINUTE);
    let dir = tempfile::tempdir().unwrap();
    let err = run(config(&stand_in, dir.path(), "1m"), async {}).await.unwrap_err();
    assert!(err.to_string().contains("a position of 1 are left"), "{err:#}");
    let posts = stand_in.with(|i| i.posts.clone());
    assert_eq!(posts.iter().map(|(path, _)| path.as_str()).collect::<Vec<_>>(), ["/v1/positions/close-all"]);
}

/// A position larger than one close takes, long or short, is closed by further reduce-only closes on
/// fresh nonces until none is left, each quoted at the node's mark.
#[tokio::test]
async fn a_stop_closes_what_one_close_leaves_open() {
    for (held, nonces) in [("2500000000", 3), ("-2500000000", 3)] {
        let stand_in = StandIn::start().await;
        stand_in.with(|i| {
            i.markets = json!([btc_usd(Some("99"))]);
            let position = json!({ "1": { "qty": held, "entry": "99" } });
            i.accounts.insert(account().to_string(), account_view("1000", position, json!({})));
            i.close_cap = Some(engine_core::Fixed::MAX_INPUT);
            i.nonce = 7;
        });
        stand_in.tick(T0 + MINUTE);
        let dir = tempfile::tempdir().unwrap();
        run(config(&stand_in, dir.path(), "1m"), async {}).await.unwrap();
        let posts = stand_in.with(|i| i.posts.clone());
        assert_eq!(posts.len(), nonces, "{held}: {posts:?}");
        for (n, (path, body)) in posts.iter().enumerate() {
            assert_eq!(path, "/v1/positions/close-all", "{held}");
            assert_eq!(body["closes"], json!([{ "market": 1, "quoted": "99" }]), "{held}");
            assert_eq!(body["auth"]["nonce"], 7 + n as u64, "{held}: a fresh nonce for each close");
        }
        assert!(stand_in.with(|i| i.accounts[&account().to_string()]["positions"].as_object().unwrap().is_empty()));
    }
}

/// A stop that never sees the log pass the last order's expiry cannot say that order is settled,
/// and reports the stop incomplete.
#[tokio::test]
async fn a_stop_that_cannot_see_the_log_pass_the_last_order_is_incomplete() {
    let stand_in = StandIn::start().await;
    let (bot, _dir) = order_then_stop(&stand_in).await;
    let err = tokio::time::timeout(Duration::from_secs(60), bot).await.unwrap().unwrap().unwrap_err();
    assert!(err.to_string().contains("the log was not seen past the last order's expiry"), "{err:#}");
}

/// The stop waits until the log passes the last order's expiry, and only then cancels.
#[tokio::test]
async fn a_stop_waits_for_the_log_to_pass_the_last_order() {
    let stand_in = StandIn::start().await;
    let (bot, _dir) = order_then_stop(&stand_in).await;
    // The order expires at minute 2 and 50 s: a command at 40 s leaves the stop waiting.
    let early = stand_in.tick(T0 + 2 * MINUTE + 40_000);
    stand_in.applied(early).await;
    assert_eq!(stand_in.with(|i| i.posts.len()), 1, "nothing is sent while the order can enter the log");
    stand_in.tick(T0 + 2 * MINUTE + 51_000);
    tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap();
    let posts = stand_in.with(|i| i.posts.clone());
    assert_eq!(
        posts.iter().map(|(path, _)| path.as_str()).collect::<Vec<_>>(),
        ["/v1/orders", "/v1/orders/cancel-all"]
    );
}

/// A stop just after a start waits for the log to pass the expiry of any order an earlier process
/// signed, then cancels.
#[tokio::test]
async fn a_stop_waits_for_orders_signed_before_the_start() {
    let stand_in = StandIn::start().await;
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99"))]);
        i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({ "9": resting_order(9) })));
        i.head_seq = 10;
    });
    let dir = tempfile::tempdir().unwrap();
    let bot = tokio::spawn(run(config(&stand_in, dir.path(), "1m"), async {}));
    // Started at T0: an earlier process's order can enter the log until T0 + 30 s.
    let early = stand_in.tick(T0 + 20_000);
    stand_in.applied(early).await;
    assert!(stand_in.with(|i| i.posts.is_empty()), "nothing is sent while such an order can enter the log");
    stand_in.tick(T0 + 31_000);
    tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap();
    let posts = stand_in.with(|i| i.posts.clone());
    assert_eq!(posts.iter().map(|(path, _)| path.as_str()).collect::<Vec<_>>(), ["/v1/orders/cancel-all"]);
}

// ── Helpers ──

/// BTC-USD's listing, as the stand-in lists it.
fn btc_spec() -> MarketSpec {
    let mut view = btc_usd(None);
    for field in ["open", "reduce_only", "mark", "fx_mark", "stress_level"] {
        view.as_object_mut().unwrap().remove(field);
    }
    serde_json::from_value(view).unwrap()
}

/// An admin command at `seq` listing `spec`, applied or rejected.
fn relisting(seq: u64, ts: i64, spec: MarketSpec, applied: bool) -> Value {
    let cmd = serde_json::to_value(Command::Admin(AdminCommand::UpsertMarket(spec))).unwrap();
    let event =
        if applied { json!({ "type": "admin_applied" }) } else { json!({ "type": "admin_rejected", "reason": "no" }) };
    command(seq, ts, cmd, json!([event]))
}

/// Starts the bot with a resting order on the account, has it order at minute 2 on a nonce the
/// node answers twenty seconds later, so the order expires at minute 2 and 50 s, and stops it.
/// The returned directory holds the bot's config and key until it is dropped.
async fn order_then_stop(stand_in: &Arc<StandIn>) -> (JoinHandle<anyhow::Result<()>>, TempDir) {
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99"))]);
        i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({ "9": resting_order(9) })));
        i.head_seq = 10;
    });
    let dir = tempfile::tempdir().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let bot = tokio::spawn(run(config(stand_in, dir.path(), "1m"), async {
        _ = stopped.await;
    }));
    warm(stand_in).await;
    stand_in.with(|i| {
        i.time_ms = (T0 + 2 * MINUTE + 20_000) as u64;
        i.frame_time = Some((T0 + 2 * MINUTE) as u64);
    });
    for (seq, ts, mid) in [(14, T0 + MINUTE, "101"), (15, T0 + 2 * MINUTE, "102")] {
        stand_in.append((seq, 0), command(seq, ts, prices(1, mid, ts), json!([])));
    }
    let (path, _) = posted(stand_in, 1).await;
    assert_eq!(path, "/v1/orders");
    stop.send(()).unwrap();
    (bot, dir)
}
