//! The bot's metrics against the stand-in: each order it places and each the node or the engine
//! declines, the account's fills, its equity and margin level, and the stream's lag; and the run
//! serving them on the configured address.

mod common;

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use common::{
    MINUTE, StandIn, T0, account, account_view, btc_usd, bucket, command, config, order, own_fill, prices,
    run_recording, warm, write_config,
};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use trader::config::Config;
use trader::metrics::Metrics;

#[tokio::test]
async fn the_bot_records_its_orders_fills_equity_and_lag() {
    let stand_in = StandIn::start().await;
    let account = account();
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99"))]);
        i.accounts.insert(account.to_string(), account_view("1000", json!({}), json!({})));
        i.head_seq = 10;
        i.marks = vec![bucket(T0, "100")];
        // The node refuses the first order and the engine the second; the third is accepted.
        i.statuses.push_back(400);
        i.replies.push_back(json!([{ "type": "order_rejected", "account": account, "client_id": T0 + 2 * MINUTE,
            "reason": { "kind": "slippage", "quoted": "103", "live": "104", "slippage_bps": 97 } }]));
    });
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(&stand_in, dir.path(), "1m");
    let metrics = Arc::new(Metrics::new());
    let (stop, stopped) = oneshot::channel::<()>();
    let recorder = Arc::clone(&metrics);
    let bot =
        tokio::spawn(async move { trader::bot::run(cfg, |_| None, async { _ = stopped.await }, &recorder).await });

    // Each bar from minute 1 on closes higher, so the target stays long and each bar orders a buy,
    // until one is accepted.
    stand_in.append((11, 0), command(11, T0, prices(1, "100", T0), json!([])));
    stand_in.append((12, 0), command(12, T0 + 1_001, prices(1, "100", T0 + 1_001), json!([])));
    stand_in.append((13, 0), command(13, T0 + MINUTE, prices(1, "101", T0 + MINUTE), json!([])));
    for (n, minute, mark) in [(1, 2, "102"), (2, 3, "103"), (3, 4, "104")] {
        let seq = 12 + minute as u64;
        stand_in.append((seq, 0), command(seq, T0 + minute * MINUTE, prices(1, mark, T0 + minute * MINUTE), json!([])));
        order(&stand_in, n).await;
    }
    // The engine's rejection counts as the log gives it, once, whatever the answer said.
    let mut second = order(&stand_in, 2).await;
    second["type"] = json!("place_order");
    let rejected = json!([{ "type": "order_rejected", "account": account, "client_id": T0 + 2 * MINUTE,
        "reason": { "kind": "slippage", "quoted": "103", "live": "104", "slippage_bps": 97 } }]);
    stand_in.append((17, 0), command(17, T0 + 4 * MINUTE + 1, second, rejected));
    let third = order(&stand_in, 3).await;
    // The fill leaves the account long 1 from 104, as the next read of it shows.
    let held = json!({ "1": { "qty": "1", "entry": "104" } });
    stand_in.with(|i| i.accounts.insert(account.to_string(), account_view("1000", held, json!({}))));
    stand_in.append((18, 0), own_fill(18, T0 + 4 * MINUTE + 2, &third, "1"));
    stand_in.applied((18, 0)).await;
    let read_at = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as f64;

    let text = metrics.encode();
    let labels = format!("account=\"{account}\",market=\"BTC-USD\"");
    for line in [
        format!("trader_orders_placed_total{{{labels}}} 3"),
        format!("trader_orders_declined_total{{{labels}}} 2"),
        format!("trader_fills_total{{{labels}}} 1"),
    ] {
        assert!(text.lines().any(|l| l == line), "no {line} in\n{text}");
    }
    // The lag of the last command read, the fill's, by this machine's clock.
    let lag: f64 = text
        .lines()
        .find_map(|l| l.strip_prefix("trader_stream_lag_seconds "))
        .unwrap_or_else(|| panic!("no lag in\n{text}"))
        .parse()
        .unwrap();
    let expected = (read_at - (T0 + 4 * MINUTE + 2) as f64) / 1000.0;
    assert!((expected - 10.0..=expected).contains(&lag), "lag {lag}, expected just under {expected}");

    // At a mark of 105 the account's equity is its 1,000 and the position's profit of 1, against a
    // requirement of 1% of the position's 105 notional.
    stand_in.append((19, 0), command(19, T0 + 4 * MINUTE + 3, prices(1, "105", T0 + 4 * MINUTE + 3), json!([])));
    stand_in.applied((19, 0)).await;
    let text = metrics.encode();
    let value = |name: &str| -> f64 {
        let prefix = format!("{name}{{account=\"{account}\"}} ");
        text.lines()
            .find_map(|l| l.strip_prefix(prefix.as_str()))
            .unwrap_or_else(|| panic!("no {name} in\n{text}"))
            .parse()
            .unwrap()
    };
    assert_eq!(value("trader_account_equity_usd"), 1001.0);
    assert!((value("trader_account_margin_level") - 1001.0 / 1.05).abs() < 1e-9, "{text}");

    // The stop's close of the position counts as placed too, and, with the log no longer read, the
    // engine's rejection of it as its answer gives it.
    stand_in.with(|i| {
        i.replies.push_back(json!([{ "type": "order_rejected", "account": account, "client_id": 0,
            "reason": { "kind": "market_closed" } }]));
    });
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap();
    let text = metrics.encode();
    for line in
        [format!("trader_orders_placed_total{{{labels}}} 4"), format!("trader_orders_declined_total{{{labels}}} 3")]
    {
        assert!(text.lines().any(|l| l == line), "no {line} after the stop in\n{text}");
    }
}

/// The engine's rejection of an order out at the stop counts once as declined, whether the log
/// message that carries it was read before the stop or comes after the order's answer.
#[tokio::test]
async fn the_rejection_of_an_order_out_at_the_stop_counts_once() {
    for read_before_stop in [false, true] {
        let rejected = json!([{ "type": "order_rejected", "account": account(), "client_id": T0 + 2 * MINUTE,
            "reason": { "kind": "slippage", "quoted": "102", "live": "104", "slippage_bps": 97 } }]);
        let (metrics, labels) = stop_with_an_order_out(read_before_stop, Some(rejected.clone()), |body| {
            command(16, T0 + 2 * MINUTE + 10_000, body, rejected.clone())
        })
        .await;
        let text = metrics.encode();
        let line = format!("trader_orders_declined_total{{{labels}}} 1");
        assert!(text.lines().any(|l| l == line), "read before the stop {read_before_stop}: no {line} in\n{text}");
    }
}

/// The fill of an order out at the stop, which the log the stop reads gives, counts as a fill.
#[tokio::test]
async fn the_fill_of_an_order_out_at_the_stop_counts() {
    for read_before_stop in [false, true] {
        let (metrics, labels) =
            stop_with_an_order_out(read_before_stop, None, |body| own_fill(16, T0 + 2 * MINUTE + 10_000, &body, "1"))
                .await;
        let text = metrics.encode();
        let line = format!("trader_fills_total{{{labels}}} 1");
        assert!(text.lines().any(|l| l == line), "read before the stop {read_before_stop}: no {line} in\n{text}");
    }
}

/// The fill of the stop's final close, which no read of the log follows, counts as a fill, with the
/// close as placed.
#[tokio::test]
async fn the_fill_of_the_final_close_counts() {
    let stand_in = StandIn::start().await;
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99"))]);
        let held = json!({ "1": { "qty": "1", "entry": "99" } });
        i.accounts.insert(account().to_string(), account_view("1000", held, json!({})));
        i.replies.push_back(json!([{ "type": "fill", "account": account(), "order_id": 1, "market": 1, "side": "sell",
            "qty": "1", "price": "99", "index": "99", "spread_bps": 0, "realised_pnl": "0", "profit_fee": "0",
            "position": "0", "liquidation": false }]));
    });
    stand_in.tick(T0 + MINUTE);
    let dir = tempfile::tempdir().unwrap();
    let metrics = Arc::new(Metrics::new());
    run_recording(config(&stand_in, dir.path(), "1m"), async {}, Arc::clone(&metrics)).await.unwrap();
    let text = metrics.encode();
    let labels = format!("account=\"{}\",market=\"BTC-USD\"", account());
    for line in [format!("trader_orders_placed_total{{{labels}}} 1"), format!("trader_fills_total{{{labels}}} 1")] {
        assert!(text.lines().any(|l| l == line), "no {line} in\n{text}");
    }
}

/// Runs the bot until it has an order out, answered with `answer` when given (and otherwise with
/// nothing), and stops it. The log message of the order, built by `message` from the order's body,
/// comes before the stop when `read_before_stop`, and otherwise after the stop has had the time to
/// pass a log read that did not wait for it; the log then moves past the order's expiry. Returns the
/// metrics once the bot has stopped, and the labels of the account's BTC-USD series.
async fn stop_with_an_order_out(
    read_before_stop: bool,
    answer: Option<serde_json::Value>,
    message: impl FnOnce(serde_json::Value) -> serde_json::Value,
) -> (Arc<Metrics>, String) {
    let stand_in = StandIn::start().await;
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99"))]);
        i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({})));
        i.head_seq = 10;
        i.replies.extend(answer);
    });
    let dir = tempfile::tempdir().unwrap();
    let metrics = Arc::new(Metrics::new());
    let (stop, stopped) = oneshot::channel::<()>();
    let bot = tokio::spawn(run_recording(
        config(&stand_in, dir.path(), "1m"),
        async {
            _ = stopped.await;
        },
        Arc::clone(&metrics),
    ));
    warm(&stand_in).await;
    // The order is signed at the node's time of minute 2 and 20 s, so it expires at minute 2 and 50 s.
    stand_in.with(|i| {
        i.time_ms = (T0 + 2 * MINUTE + 20_000) as u64;
        i.frame_time = Some((T0 + 2 * MINUTE) as u64);
    });
    for (seq, ts, mid) in [(14, T0 + MINUTE, "101"), (15, T0 + 2 * MINUTE, "102")] {
        stand_in.append((seq, 0), command(seq, ts, prices(1, mid, ts), json!([])));
    }
    let mut body = order(&stand_in, 1).await;
    body["type"] = json!("place_order");
    if read_before_stop {
        stand_in.append((16, 0), message(body));
        stand_in.applied((16, 0)).await;
        stop.send(()).unwrap();
    } else {
        stop.send(()).unwrap();
        // A stop that does not wait for the log has passed its read by now.
        tokio::time::sleep(Duration::from_millis(500)).await;
        stand_in.append((16, 0), message(body));
    }
    stand_in.tick(T0 + 2 * MINUTE + 51_000);
    tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap();
    (metrics, format!("account=\"{}\",market=\"BTC-USD\"", account()))
}

/// The run serves the bot's metrics on the configured address while the bot trades, and refuses to
/// start on an address it cannot bind.
#[tokio::test]
async fn the_run_serves_metrics_on_the_configured_address() {
    let stand_in = StandIn::start().await;
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("100"))]);
        i.accounts.insert(account().to_string(), account_view("5", json!({}), json!({})));
    });
    // The log passes the window orders signed before the start could still enter it in, so a stop
    // has nothing to wait for.
    stand_in.tick(T0 + MINUTE);
    let dir = tempfile::tempdir().unwrap();

    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let cfg = metrics_config(&stand_in, dir.path(), taken.local_addr().unwrap());
    let err = within(trader::serve(cfg, |_| None, async {}, None)).await.unwrap_err();
    assert!(err.to_string().starts_with(&format!("binding {}", taken.local_addr().unwrap())), "{err:#}");
    drop(taken);

    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let cfg = metrics_config(&stand_in, dir.path(), free);
    let (stop, stopped) = oneshot::channel::<()>();
    let run = tokio::spawn(trader::serve(cfg, |_| None, async { _ = stopped.await }, None));
    // The account's series appear once the bot has started.
    let series = format!("trader_orders_placed_total{{account=\"{}\",market=\"BTC-USD\"}} 0\n", account());
    let mut served = String::new();
    for _ in 0..200 {
        served = tokio::time::timeout(Duration::from_secs(2), scrape(free)).await.ok().flatten().unwrap_or_default();
        if served.contains(&series) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(served.contains(&series), "{served}");
    stop.send(()).unwrap();
    within(async { run.await.unwrap() }).await.unwrap();
}

// ── Helpers ──

/// The stand-in's config in `dir`, serving metrics at `listen`.
fn metrics_config(stand_in: &StandIn, dir: &Path, listen: SocketAddr) -> Config {
    let path = write_config(stand_in, dir, "1m", &[]);
    let text = std::fs::read_to_string(&path).unwrap() + &format!("\n[metrics]\nlisten = \"{listen}\"\n");
    std::fs::write(&path, text).unwrap();
    Config::load(&path).unwrap()
}

/// The answer to `GET /metrics` at `addr`, or `None` without one.
async fn scrape(addr: SocketAddr) -> Option<String> {
    let mut tcp = TcpStream::connect(addr).await.ok()?;
    tcp.write_all(b"GET /metrics HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n\r\n").await.ok()?;
    let mut answer = String::new();
    tcp.read_to_string(&mut answer).await.ok()?;
    Some(answer)
}

/// `run`'s result, or a panic after twenty seconds.
async fn within(run: impl Future<Output = anyhow::Result<()>>) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(20), run).await.expect("the run ended within twenty seconds")
}
