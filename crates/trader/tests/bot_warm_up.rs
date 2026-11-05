//! The bot's warm-up against the stand-in: the bars before the current one come from the node's
//! marks history, read once the bot's index is exact and the node's cache holds every move before
//! the live window; only moves of the exact index, from log-derived buckets, make bars; and the
//! stream starts after the log head as it stood before the startup reads.

mod common;

use std::time::Duration;

use common::{
    MINUTE, StandIn, T0, account, account_view, btc_usd, bucket, check_order, command, config, order, prices, run,
};
use serde_json::json;
use tokio::sync::oneshot;

/// The bars before the current one come from the marks history: with the slow average full from
/// the history, the first bar the log completes already orders.
#[tokio::test]
async fn warms_up_from_the_marks_history() {
    let stand_in = StandIn::start().await;
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("100"))]);
        i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({})));
        i.head_seq = 10;
        // Three one-minute buckets before T0: the two-minute bars before T0 close at 105 and at 100;
        // then the mark moves to 99 at T0.
        i.marks = vec![
            bucket(T0 - 4 * MINUTE, "105"),
            bucket(T0 - 2 * MINUTE, "101"),
            bucket(T0 - MINUTE, "100"),
            bucket(T0, "99"),
        ];
    });
    let dir = tempfile::tempdir().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let bot = tokio::spawn(run(config(&stand_in, dir.path(), "2m"), async {
        _ = stopped.await;
    }));
    // Bars of two minutes: history gives 105 then 100; the log's first bar closes at 99, below the
    // average of 100 and 99, so the target is short at once.
    stand_in.append((11, 0), command(11, T0, prices(1, "99", T0), json!([])));
    stand_in.append((12, 0), command(12, T0 + 1_001, prices(1, "99", T0 + 1_001), json!([])));
    stand_in.append((13, 0), command(13, T0 + 2 * MINUTE, prices(1, "98", T0 + 2 * MINUTE), json!([])));
    check_order(&order(&stand_in, 1).await, T0, "sell", "1.020408163", "98", false, 0);
    stand_in.with(|i| i.time_ms = (T0 + 10 * MINUTE) as u64);
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap();
}

/// The current bar starts from the history's bucket in its window: a bot started at T0 + 20 s,
/// whose mark does not move again before minute 1, still has minute 0's bar, at the history's 101.
#[tokio::test]
async fn the_current_bar_starts_from_the_history() {
    let stand_in = StandIn::start().await;
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("101"))]);
        i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({})));
        i.head_seq = 10;
        i.time_ms = (T0 + 20_000) as u64;
        let bucket = |ts: i64, close: &str| {
            json!({ "ts": ts, "open": close, "high": close, "low": close,
            "close": close, "provenance": "log_derived" })
        };
        i.marks = vec![bucket(T0 - 2 * MINUTE, "99"), bucket(T0 - MINUTE, "100"), bucket(T0, "101")];
    });
    let dir = tempfile::tempdir().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let bot = tokio::spawn(run(config(&stand_in, dir.path(), "1m"), async {
        _ = stopped.await;
    }));
    for (seq, ts) in [(11, T0 + 20_000), (12, T0 + 21_001), (13, T0 + MINUTE)] {
        stand_in.append((seq, 0), command(seq, ts, prices(1, "101", ts), json!([])));
    }
    // 101 above the average of 100 and 101: long, ordered as minute 0's bar closes.
    assert_eq!(order(&stand_in, 1).await["client_id"], T0);
    stand_in.with(|i| i.time_ms = (T0 + 10 * MINUTE) as u64);
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap();
}

/// The bot warms up only once its own index is exact, so a move the node made while the bot's
/// book was still filling is in the history's bar, not lost: started 0.5 s before minute 1, with
/// minute 0 at 102 then, and moving to 99 before the book is whole, minute 0's bar is 99.
#[tokio::test]
async fn a_move_before_the_index_is_exact_comes_from_the_history() {
    let stand_in = StandIn::start().await;
    let bucket = |ts: i64, close: &str| {
        json!({ "ts": ts, "open": close, "high": close, "low": close,
        "close": close, "provenance": "log_derived" })
    };
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("102"))]);
        i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({})));
        i.head_seq = 10;
        i.time_ms = (T0 + 59_500) as u64;
        i.marks = vec![bucket(T0 - MINUTE, "100"), bucket(T0, "102")];
    });
    let dir = tempfile::tempdir().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let bot = tokio::spawn(run(config(&stand_in, dir.path(), "1m"), async {
        _ = stopped.await;
    }));
    stand_in.append((11, 0), command(11, T0 + 59_500, prices(1, "102", T0 + 59_500), json!([])));
    stand_in.append((12, 0), command(12, T0 + 59_800, prices(1, "99", T0 + 59_800), json!([])));
    stand_in.applied((12, 0)).await;
    assert_eq!(stand_in.with(|i| i.marks_reads), 0, "no warm-up before the index is exact");
    stand_in.with(|i| {
        // By the warm-up the node's history holds the move to 99 in minute 0, and to 100.5 in minute 1.
        i.marks = vec![bucket(T0 - MINUTE, "100"), bucket(T0, "99"), bucket(T0 + MINUTE, "100.5")];
        i.time_ms = (T0 + 60_600) as u64;
    });
    stand_in.append((13, 0), command(13, T0 + 60_600, prices(1, "100.5", T0 + 60_600), json!([])));
    stand_in.applied((13, 0)).await;
    assert_eq!(stand_in.with(|i| i.marks_reads), 0, "no warm-up until the node has written the marks before then");
    stand_in.append((14, 0), command(14, T0 + 2 * MINUTE, prices(1, "100.5", T0 + 2 * MINUTE), json!([])));
    // Closes 100, 99, 100.5: long. With minute 0 at 102 the target would be short.
    let first = order(&stand_in, 1).await;
    assert_eq!((first["client_id"].as_i64(), first["side"].as_str()), (Some(T0 + MINUTE), Some("buy")), "{first}");
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap();
}

/// A bucket seeded from a venue's candles is not the engine's index, and makes no bar: with minute
/// −1's seeded 200 left out, minute 0 at 100 equals minute −2's, and only minute 1's fall to 99
/// targets short.
#[tokio::test]
async fn seeded_history_makes_no_bar() {
    let stand_in = StandIn::start().await;
    let mut seeded = bucket(T0 - MINUTE, "200");
    seeded["provenance"] = json!("seeded_from_provider");
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("100"))]);
        i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({})));
        i.head_seq = 10;
        i.marks = vec![bucket(T0 - 2 * MINUTE, "100"), seeded, bucket(T0, "100")];
    });
    let dir = tempfile::tempdir().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let bot = tokio::spawn(run(config(&stand_in, dir.path(), "1m"), async {
        _ = stopped.await;
    }));
    for (seq, ts, mid) in
        [(11, T0, "100"), (12, T0 + 1_001, "100"), (13, T0 + MINUTE, "99"), (14, T0 + 2 * MINUTE, "99")]
    {
        stand_in.append((seq, 0), command(seq, ts, prices(1, mid, ts), json!([])));
    }
    let first = order(&stand_in, 1).await;
    assert_eq!((first["client_id"].as_i64(), first["side"].as_str()), (Some(T0 + MINUTE), Some("sell")), "{first}");
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap();
}

/// The first exact value of the index is where moves start from, not a move: started short at
/// 59.5 s into minute 0, with the mark moving to 99 before the bot's index is exact and staying
/// there, minute 1 has no bar, as the node's history has none; a bar for it would close at 99 and
/// target flat, buying 1 to close.
#[tokio::test]
async fn the_first_exact_mark_is_not_a_move() {
    let stand_in = StandIn::start().await;
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("102"))]);
        let short = json!({ "1": { "qty": "-1", "entry": "102" } });
        i.accounts.insert(account().to_string(), account_view("1000", short, json!({})));
        i.head_seq = 10;
        i.time_ms = (T0 + 59_500) as u64;
        i.marks = vec![bucket(T0 - MINUTE, "100"), bucket(T0, "99")];
    });
    let dir = tempfile::tempdir().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let bot = tokio::spawn(run(config(&stand_in, dir.path(), "1m"), async {
        _ = stopped.await;
    }));
    let commands = [
        (11, T0 + 59_500, "102"),
        (12, T0 + 59_800, "99"),
        (13, T0 + 60_600, "99"),
        (14, T0 + 66_000, "99"),
        (15, T0 + 2 * MINUTE, "101"),
        (16, T0 + 3 * MINUTE, "101"),
    ];
    for (seq, ts, mid) in commands {
        stand_in.append((seq, 0), command(seq, ts, prices(1, mid, ts), json!([])));
    }
    // Closes 100, 99, then minute 2's 101: long, a buy of 1.990099009 from short 1, closing it and opening 100 USD at 101.
    let first = order(&stand_in, 1).await;
    let fields = (first["client_id"].as_i64(), first["side"].as_str(), first["qty"].as_str());
    assert_eq!(fields, (Some(T0 + 2 * MINUTE), Some("buy"), Some("1.990099009")), "{first}");
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap();
}

/// The stream starts after the log head as it stood before the startup reads, so it misses nothing
/// their answers might not hold, however far the head moves while they are made.
#[tokio::test]
async fn the_stream_starts_after_the_head_before_the_reads() {
    let stand_in = StandIn::start().await;
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("100"))]);
        i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({})));
        i.head_seq = 10;
        i.head_step = 1;
    });
    stand_in.append((11, 0), command(11, T0 + MINUTE, json!({ "type": "tick" }), json!([])));
    let dir = tempfile::tempdir().unwrap();
    run(config(&stand_in, dir.path(), "1m"), async {}).await.unwrap();
    let first = stand_in.with(|i| i.connections[0].clone());
    assert!(first.starts_with("/v1/stream?from=11&"), "{first}");
}
