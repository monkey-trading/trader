use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::*;

/// Each series records under its names and labels, with the values recorded.
#[test]
fn each_series_records_under_its_labels() {
    let metrics = Metrics::new();
    let account = AccountId([0xab; 20]);
    let series = metrics.account(account, "BTC-USD");
    series.orders_placed.inc();
    series.orders_placed.inc();
    series.orders_declined.inc();
    series.fills.inc();
    series.set_equity("1500.5".parse().unwrap(), "300".parse().unwrap());
    metrics.stream_lag().set(0.25);
    let text = metrics.encode();
    let labels = format!("account=\"{account}\",market=\"BTC-USD\"");
    for line in [
        format!("trader_orders_placed_total{{{labels}}} 2"),
        format!("trader_orders_declined_total{{{labels}}} 1"),
        format!("trader_fills_total{{{labels}}} 1"),
        format!("trader_account_equity_usd{{account=\"{account}\"}} 1500.5"),
        format!("trader_account_margin_level{{account=\"{account}\"}} 5.001666666666667"),
        "trader_stream_lag_seconds 0.25".to_owned(),
    ] {
        assert!(text.lines().any(|l| l == line), "no {line} in\n{text}");
    }
}

/// Two markets of one account share its equity, and keep their own orders and fills.
#[test]
fn an_accounts_markets_share_its_equity() {
    let metrics = Metrics::new();
    let account = AccountId([0xab; 20]);
    let (btc, eth) = (metrics.account(account, "BTC-USD"), metrics.account(account, "ETH-USD"));
    btc.fills.inc();
    eth.set_equity("10".parse().unwrap(), "4".parse().unwrap());
    let text = metrics.encode();
    assert!(text.contains(&format!("trader_fills_total{{account=\"{account}\",market=\"BTC-USD\"}} 1\n")), "{text}");
    assert!(text.contains(&format!("trader_fills_total{{account=\"{account}\",market=\"ETH-USD\"}} 0\n")), "{text}");
    assert_eq!(text.matches("trader_account_margin_level{").count(), 1, "{text}");
    assert!(text.contains(&format!("trader_account_margin_level{{account=\"{account}\"}} 2.5\n")), "{text}");
}

/// With nothing required, the margin level is not a number; equity below zero makes it negative.
#[test]
fn the_margin_level_is_not_a_number_with_nothing_required() {
    let metrics = Metrics::new();
    let series = metrics.account(AccountId([1; 20]), "BTC-USD");
    series.set_equity("1000".parse().unwrap(), Fixed::ZERO);
    let text = metrics.encode();
    assert!(
        text.contains("trader_account_margin_level{account=\"0x0101010101010101010101010101010101010101\"} NaN\n"),
        "{text}"
    );
    series.set_equity("-50".parse().unwrap(), "100".parse().unwrap());
    let text = metrics.encode();
    assert!(
        text.contains("trader_account_equity_usd{account=\"0x0101010101010101010101010101010101010101\"} -50.0\n"),
        "{text}"
    );
    assert!(text.contains("} -0.5\n"), "{text}");
}

/// The listener answers `GET /metrics` with the registry in OpenMetrics text.
#[tokio::test]
async fn the_listener_serves_the_registry() {
    let metrics = Arc::new(Metrics::new());
    metrics.account(AccountId([2; 20]), "BTC-USD").orders_placed.inc();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve(listener, Arc::clone(&metrics)));
    let mut answer = String::new();
    let exchange = async {
        let mut tcp = TcpStream::connect(addr).await.unwrap();
        tcp.write_all(b"GET /metrics HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n\r\n").await.unwrap();
        tcp.read_to_string(&mut answer).await.unwrap();
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), exchange).await.expect("an answer within ten seconds");
    assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
    assert!(answer.contains(&format!("content-type: {CONTENT_TYPE_OPENMETRICS}\r\n")), "{answer}");
    assert!(
        answer.contains(
            "trader_orders_placed_total{account=\"0x0202020202020202020202020202020202020202\",market=\"BTC-USD\"} 1\n"
        ),
        "{answer}"
    );
    assert!(answer.trim_end().ends_with("# EOF"), "{answer}");
}
