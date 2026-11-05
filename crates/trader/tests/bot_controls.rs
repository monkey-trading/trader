//! The bot's controls against the stand-in: the daily loss limit flattens the account and holds it
//! stopped across a restart until the next UTC day, a halt in the market's hours and the first bar
//! after it place no order, a position is held for the minimum, an account without cash opens
//! nothing, a split and a failed corporate action halt.

mod common;

use std::time::Duration;

use common::{
    MINUTE, StandIn, T0, account, account_view, btc_usd, command, config_with, own_fill, posted, prices, run, warm,
};
use serde_json::{Value, json};
use tokio::sync::oneshot;

const DAY: i64 = 24 * 60 * MINUTE;

/// A prices command at `ts`.
fn price_at(stand_in: &StandIn, ts: i64, mid: &str) {
    let seq = next_seq(stand_in);
    stand_in.append((seq, 0), command(seq, ts, prices(1, mid, ts), json!([])));
}

/// The seq after the log's last message, or the head.
fn next_seq(stand_in: &StandIn) -> u64 {
    stand_in.with(|i| i.log.last().map_or(i.head_seq, |((seq, _), _)| *seq) + 1)
}

/// The log's entry for the close-all request `body` the bot posted, which closed the position at `ts`.
fn own_close(seq: u64, ts: i64, body: &Value) -> Value {
    let fill = json!({ "type": "fill", "account": account(), "order_id": seq, "market": 1, "side": "sell",
        "qty": "1", "price": "95", "index": "95", "spread_bps": 0, "realised_pnl": "-4", "profit_fee": "0",
        "position": "0", "liquidation": false });
    let evidence = json!({ "result": "failed", "reason": "unavailable" });
    let cmd = json!({ "type": "access_close_all", "request": body, "evidence": evidence });
    command(seq, ts, cmd, json!([fill]))
}

/// How many requests the stand-in has seen, after letting the bot catch up with the log.
async fn posts_after_settling(stand_in: &StandIn) -> usize {
    let last = stand_in.with(|i| i.log.last().map(|(offset, _)| *offset));
    if let Some(last) = last {
        stand_in.applied(last).await;
    }
    stand_in.with(|i| i.posts.len())
}

/// The state file the bot keeps for the test account.
fn day_file(dir: &std::path::Path) -> Value {
    let text = std::fs::read_to_string(dir.join("state").join(format!("{}.json", account()))).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn start(
    stand_in: &std::sync::Arc<StandIn>,
    dir: &std::path::Path,
    edits: &[(&str, &str)],
) -> (oneshot::Sender<()>, tokio::task::JoinHandle<anyhow::Result<()>>) {
    let (stop, stopped) = oneshot::channel::<()>();
    let bot = tokio::spawn(run(config_with(stand_in, dir, "1m", edits), async {
        _ = stopped.await;
    }));
    (stop, bot)
}

/// A loss of the limit flattens the account at the mark that reaches it, quoted there, and tries again at
/// each bar while the position remains. Flat, it places nothing for the rest of the UTC day, though the
/// strategy says long, and a restart the same day keeps it stopped; the next day it trades again.
#[tokio::test]
async fn the_daily_loss_limit_flattens_and_holds_across_a_restart() {
    let stand_in = StandIn::start().await;
    let dir = tempfile::tempdir().unwrap();
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99"))]);
        let position = json!({ "1": { "qty": "1", "entry": "99" } });
        i.accounts.insert(account().to_string(), account_view("1000", position, json!({})));
        i.head_seq = 10;
        i.nonce = 3;
        // The closes are declined: the account keeps its position until the last one lands.
        i.settle = false;
    });
    let (stop, bot) =
        start(&stand_in, dir.path(), &[("daily_loss = { usd = \"100000\" }", "daily_loss = { usd = \"5\" }")]);
    warm(&stand_in).await;

    // The mark falls to 94: equity 995 against the day's opening 1,000 is a loss of 5, the limit.
    price_at(&stand_in, T0 + MINUTE, "94");
    let (path, body) = posted(&stand_in, 1).await;
    assert_eq!(path, "/v1/positions/close-all");
    assert_eq!(body["closes"], json!([{ "market": 1, "quoted": "94" }]));
    assert_eq!(day_file(dir.path())["day"]["tripped"], true);
    assert_eq!(day_file(dir.path())["day"]["start_equity"], "1000");

    // The close is declined or not yet in the log: it is tried again once five seconds have passed since
    // the last try, on any command, though the mark has not moved.
    price_at(&stand_in, T0 + MINUTE + 2_000, "96");
    assert_eq!(posts_after_settling(&stand_in).await, 1, "too soon after the last try");
    let seq = next_seq(&stand_in);
    stand_in.append((seq, 0), command(seq, T0 + MINUTE + 6_000, json!({ "type": "tick" }), json!([])));
    let (path, body) = posted(&stand_in, 2).await;
    assert_eq!(
        (path.as_str(), &body["closes"]),
        ("/v1/positions/close-all", &json!([{ "market": 1, "quoted": "96" }]))
    );

    // The close lands, and is the bot's own fill; no bar after it orders, whatever the strategy says.
    stand_in.with(|i| i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({}))));
    let seq = next_seq(&stand_in);
    stand_in.append((seq, 0), own_close(seq, T0 + MINUTE + 6_100, &body));
    for (_, minute, mid) in [(17, 2, "96"), (18, 3, "99"), (19, 4, "103"), (20, 5, "110")] {
        price_at(&stand_in, T0 + minute * MINUTE, mid);
    }
    assert_eq!(posts_after_settling(&stand_in).await, 2);
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap();

    // A restart on the same day, with the account flat, stays stopped.
    let again = StandIn::start().await;
    again.with(|i| {
        i.markets = json!([btc_usd(Some("110"))]);
        i.accounts.insert(account().to_string(), account_view("994", json!({}), json!({})));
        i.head_seq = 10;
        i.nonce = 8;
    });
    let (stop, bot) =
        start(&again, dir.path(), &[("daily_loss = { usd = \"100000\" }", "daily_loss = { usd = \"5\" }")]);
    warm(&again).await;
    for (_, minute, mid) in [(13, 1, "101"), (14, 2, "102"), (15, 3, "103")] {
        price_at(&again, T0 + minute * MINUTE, mid);
    }
    assert_eq!(posts_after_settling(&again).await, 0, "stopped for the day");

    // The next UTC day opens afresh, and the same bars order.
    again.with(|i| i.time_ms = (T0 + DAY) as u64);
    for (_, minute, mid) in [(16, 0, "100"), (17, 1, "101"), (18, 2, "102"), (19, 3, "103")] {
        price_at(&again, T0 + DAY + minute * MINUTE, mid);
    }
    let (path, body) = posted(&again, 1).await;
    assert_eq!((path.as_str(), body["side"].as_str()), ("/v1/orders", Some("buy")));
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap();
    assert_eq!(day_file(dir.path())["day"]["tripped"], false);
}

/// A halt in the market's hours: the bar the halt closes places no order, the first bar after the
/// market reopens is a gap and places none, and the bar after that orders.
#[tokio::test]
async fn no_order_in_a_halt_or_on_the_first_bar_after_it() {
    let stand_in = StandIn::start().await;
    let dir = tempfile::tempdir().unwrap();
    // Friday 2027-01-15 06:00 to 12:00 UTC, with a halt from 08:02 to 08:10.
    let mut market = btc_usd(Some("99"));
    market["schedule"] = json!({ "kind": "weekly", "tz": "UTC",
        "windows": [{ "open": "Fri 06:00", "close": "Fri 12:00" }], "closures": [
        { "from": "2027-01-15T08:02:00", "to": "2027-01-15T08:10:00" }] });
    stand_in.with(|i| {
        i.markets = json!([market]);
        i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({})));
        i.head_seq = 10;
        i.nonce = 3;
    });
    let (stop, bot) = start(&stand_in, dir.path(), &[]);
    warm(&stand_in).await;
    // Closes 100 then 101 make the target long, but the command that completes the second bar is in the halt.
    price_at(&stand_in, T0 + MINUTE, "101");
    price_at(&stand_in, T0 + 2 * MINUTE, "101.5");
    assert_eq!(posts_after_settling(&stand_in).await, 0, "the halt shuts the market at 08:02");
    // The market reopens at 08:10 with a gap: 102 at 08:10, 103 at 08:11 (the gap bar completes), 104 at 08:12.
    price_at(&stand_in, T0 + 10 * MINUTE, "102");
    price_at(&stand_in, T0 + 11 * MINUTE, "103");
    assert_eq!(posts_after_settling(&stand_in).await, 0, "the first bar after the halt is a gap");
    price_at(&stand_in, T0 + 12 * MINUTE, "104");
    let (path, order) = posted(&stand_in, 1).await;
    assert_eq!(path, "/v1/orders");
    assert_eq!((order["side"].as_str(), order["client_id"].as_i64()), (Some("buy"), Some(T0 + 11 * MINUTE)));
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap();
}

/// A position is held for the minimum before a strategy reverses it, counted from its fill in the log.
#[tokio::test]
async fn a_position_is_held_for_the_minimum() {
    let stand_in = StandIn::start().await;
    let dir = tempfile::tempdir().unwrap();
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99"))]);
        i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({})));
        i.head_seq = 10;
        i.nonce = 3;
    });
    let (stop, bot) = start(&stand_in, dir.path(), &[("min_hold = \"0s\"", "min_hold = \"5m\"")]);
    warm(&stand_in).await;
    price_at(&stand_in, T0 + MINUTE, "101");
    price_at(&stand_in, T0 + 2 * MINUTE, "102");
    let (_, buy) = posted(&stand_in, 1).await;
    assert_eq!(buy["side"], "buy");
    // The buy fills at 08:02:00.100; the account then holds 0.980392156.
    stand_in.with(|i| {
        let position = json!({ "1": { "qty": "0.980392156", "entry": "102" } });
        i.accounts.insert(account().to_string(), account_view("1000", position, json!({})));
    });
    let seq = next_seq(&stand_in);
    stand_in.append((seq, 0), own_fill(seq, T0 + 2 * MINUTE + 100, &buy, "0.980392156"));
    // The target turns short at 08:04 and stays so; the hold runs to 08:07:00.100.
    for (_, minute, mid) in [(16, 3, "90"), (17, 4, "80"), (18, 5, "79"), (19, 6, "78")] {
        price_at(&stand_in, T0 + minute * MINUTE, mid);
    }
    assert_eq!(posts_after_settling(&stand_in).await, 1, "held until 08:07");
    price_at(&stand_in, T0 + 8 * MINUTE, "77");
    let (_, sell) = posted(&stand_in, 2).await;
    assert_eq!(sell["side"], "sell");
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap();
}

/// An account with no cash opens nothing, however the strategy points.
#[tokio::test]
async fn an_account_without_cash_opens_nothing() {
    let stand_in = StandIn::start().await;
    let dir = tempfile::tempdir().unwrap();
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("100"))]);
        i.accounts.insert(account().to_string(), account_view("0", json!({}), json!({})));
        i.head_seq = 10;
    });
    let (stop, bot) = start(&stand_in, dir.path(), &[]);
    warm(&stand_in).await;
    for (_, minute, mid) in [(13, 1, "101"), (14, 2, "102"), (15, 3, "103")] {
        price_at(&stand_in, T0 + minute * MINUTE, mid);
    }
    assert_eq!(posts_after_settling(&stand_in).await, 0);
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap();
}

/// An order that would leave equity within the margin buffer of its requirement is not placed: 100 USD
/// of BTC needs 1 of margin at 1%, and a buffer of 1,000% needs equity of 11, which 5 is short of.
#[tokio::test]
async fn the_margin_buffer_holds_back_an_order() {
    let stand_in = StandIn::start().await;
    let dir = tempfile::tempdir().unwrap();
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("100"))]);
        i.accounts.insert(account().to_string(), account_view("5", json!({}), json!({})));
        i.head_seq = 10;
    });
    let edits = [("margin_buffer_bps = 0", "margin_buffer_bps = 100000")];
    let (stop, bot) = start(&stand_in, dir.path(), &edits);
    warm(&stand_in).await;
    for (_, minute, mid) in [(13, 1, "101"), (14, 2, "102"), (15, 3, "103")] {
        price_at(&stand_in, T0 + minute * MINUTE, mid);
    }
    assert_eq!(posts_after_settling(&stand_in).await, 0);
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap();
}

/// A split changes the units of every price the strategy has seen, so the bot halts rather than trade on them.
#[tokio::test]
async fn a_split_halts() {
    let stand_in = StandIn::start().await;
    let dir = tempfile::tempdir().unwrap();
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99"))]);
        i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({})));
        i.head_seq = 10;
    });
    let (_stop, bot) = start(&stand_in, dir.path(), &[]);
    warm(&stand_in).await;
    let split = json!({ "type": "corporate_split", "id": "s1", "market": 1, "new_units": 2, "old_units": 1 });
    let seq = next_seq(&stand_in);
    stand_in.append((seq, 0), command(seq, T0 + MINUTE, json!({ "type": "tick" }), json!([split])));
    let err = tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap_err();
    assert!(err.to_string().contains("does not follow a split"), "{err:#}");
    // A restart will not warm up from prices before the split.
    assert_eq!(day_file(dir.path())["split_at"], T0 + MINUTE);
}

/// A dividend for the account moves its cash without a fill, so the bot reads the account again; one for
/// another account does not.
#[tokio::test]
async fn a_dividend_for_the_account_reads_it_again() {
    let stand_in = StandIn::start().await;
    let dir = tempfile::tempdir().unwrap();
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99"))]);
        i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({})));
        i.head_seq = 10;
    });
    let (stop, bot) = start(&stand_in, dir.path(), &[]);
    warm(&stand_in).await;
    let dividend = |account: String, ts: i64| {
        let event = json!({ "type": "corporate_dividend", "id": "d1", "market": 1, "account": account,
            "quantity": "1", "amount": "5", "fx_mark": null });
        (ts, json!([event]))
    };
    let append = |(ts, events): (i64, Value)| {
        let seq = next_seq(&stand_in);
        stand_in.append((seq, 0), command(seq, ts, json!({ "type": "tick" }), events));
        seq
    };
    stand_in.applied((next_seq(&stand_in) - 1, 0)).await;
    let reads = stand_in.with(|i| i.account_reads);
    let seq = append(dividend(account().to_string(), T0 + 10_000));
    stand_in.applied((seq, 0)).await;
    assert_eq!(stand_in.with(|i| i.account_reads), reads + 1);
    let seq = append(dividend("0x0000000000000000000000000000000000000001".into(), T0 + 11_000));
    stand_in.applied((seq, 0)).await;
    assert_eq!(stand_in.with(|i| i.account_reads), reads + 1, "another account's dividend reads nothing");
    // The log passes the window orders signed before the start could enter it in, so the stop is quick.
    append((T0 + 40_000, json!([])));
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap();
}

/// An account that holds a position in another market is refused at the start, and halts the bot when one
/// appears: its limits are the account's, and the bot sees one market.
#[tokio::test]
async fn a_position_in_another_market_is_refused() {
    let stand_in = StandIn::start().await;
    let dir = tempfile::tempdir().unwrap();
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99"))]);
        let held = json!({ "2": { "qty": "-1", "entry": "5" } });
        i.accounts.insert(account().to_string(), account_view("1000", held, json!({})));
        i.head_seq = 10;
    });
    let err = run(config_with(&stand_in, dir.path(), "1m", &[]), async {}).await.unwrap_err();
    assert!(err.to_string().contains("holds a position in market 2"), "{err:#}");
    assert!(stand_in.with(|i| i.posts.is_empty()));
}

/// A read of the account that is not at the log values nothing: a short opened from flat, read as
/// still flat, would be valued at an entry of zero, a loss of its whole notional. The daily limit
/// does not trip on it, and once a read agrees the account is valued from its entry.
#[tokio::test]
async fn a_read_behind_the_log_trips_nothing() {
    let stand_in = StandIn::start().await;
    let dir = tempfile::tempdir().unwrap();
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99"))]);
        i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({})));
        i.head_seq = 10;
        i.nonce = 3;
    });
    let (stop, bot) =
        start(&stand_in, dir.path(), &[("daily_loss = { usd = \"100000\" }", "daily_loss = { usd = \"5\" }")]);
    warm(&stand_in).await;
    price_at(&stand_in, T0 + MINUTE, "98");
    price_at(&stand_in, T0 + 2 * MINUTE, "97");
    let (_, sell) = posted(&stand_in, 1).await;
    assert_eq!(sell["side"], "sell");
    // The sell fills, but the node's account still reads flat.
    let seq = next_seq(&stand_in);
    stand_in.append((seq, 0), own_fill(seq, T0 + 2 * MINUTE + 100, &sell, "-1.030927835"));
    price_at(&stand_in, T0 + 2 * MINUTE + 1_000, "97.5");
    assert_eq!(posts_after_settling(&stand_in).await, 1, "no close: the day's loss is not read from a stale view");
    assert_eq!(day_file(dir.path())["day"]["tripped"], false);
    // A later read agrees; the 0.5 USD loss per unit is far inside the limit.
    stand_in.with(|i| {
        let position = json!({ "1": { "qty": "-1.030927835", "entry": "97" } });
        i.accounts.insert(account().to_string(), account_view("1000", position, json!({})));
    });
    price_at(&stand_in, T0 + 2 * MINUTE + 8_000, "97.6");
    price_at(&stand_in, T0 + 2 * MINUTE + 9_000, "97.7");
    assert_eq!(posts_after_settling(&stand_in).await, 1);
    assert_eq!(day_file(dir.path())["day"]["tripped"], false);
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap();
}

/// The time a position opened is kept in the state file, so a restart finds the hold still running.
#[tokio::test]
async fn the_hold_survives_a_restart() {
    let hold = [("min_hold = \"0s\"", "min_hold = \"5m\"")];
    let stand_in = StandIn::start().await;
    let dir = tempfile::tempdir().unwrap();
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99"))]);
        i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({})));
        i.head_seq = 10;
        i.nonce = 3;
    });
    let (stop, bot) = start(&stand_in, dir.path(), &hold);
    warm(&stand_in).await;
    price_at(&stand_in, T0 + MINUTE, "101");
    price_at(&stand_in, T0 + 2 * MINUTE, "102");
    let (_, buy) = posted(&stand_in, 1).await;
    stand_in.with(|i| {
        let position = json!({ "1": { "qty": "0.980392156", "entry": "102" } });
        i.accounts.insert(account().to_string(), account_view("1000", position, json!({})));
    });
    let seq = next_seq(&stand_in);
    stand_in.append((seq, 0), own_fill(seq, T0 + 2 * MINUTE + 100, &buy, "0.980392156"));
    price_at(&stand_in, T0 + 2 * MINUTE + 1_000, "102.5");
    posts_after_settling(&stand_in).await;
    assert_eq!(day_file(dir.path())["opened"], json!({ "1": T0 + 2 * MINUTE + 100 }));
    // A crash: the bot ends with the position open.
    drop(stop);
    bot.abort();
    _ = bot.await;

    let again = StandIn::start().await;
    again.with(|i| {
        i.markets = json!([btc_usd(Some("99"))]);
        let position = json!({ "1": { "qty": "0.980392156", "entry": "102" } });
        i.accounts.insert(account().to_string(), account_view("1000", position, json!({})));
        i.head_seq = 10;
        i.nonce = 8;
    });
    let (stop, bot) = start(&again, dir.path(), &hold);
    warm(&again).await;
    for (_, minute, mid) in [(0, 3, "90"), (0, 4, "80"), (0, 5, "79"), (0, 6, "78")] {
        price_at(&again, T0 + minute * MINUTE, mid);
    }
    assert_eq!(posts_after_settling(&again).await, 0, "held until 08:07");
    price_at(&again, T0 + 8 * MINUTE, "77");
    let (_, sell) = posted(&again, 1).await;
    assert_eq!(sell["side"], "sell");
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap();
}

/// A corporate action that fails stops the market dealing, so the bot halts.
#[tokio::test]
async fn a_failed_corporate_action_halts() {
    let stand_in = StandIn::start().await;
    let dir = tempfile::tempdir().unwrap();
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("100"))]);
        i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({})));
        i.head_seq = 10;
    });
    let (_stop, bot) = start(&stand_in, dir.path(), &[]);
    warm(&stand_in).await;
    let failed = json!({ "type": "corporate_action_failed", "id": "s1", "market": 1, "at": T0, "reason": "no price" });
    let seq = next_seq(&stand_in);
    stand_in.append((seq, 0), command(seq, T0 + MINUTE, json!({ "type": "tick" }), json!([failed])));
    let err = tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap_err();
    assert!(err.to_string().contains("a corporate action failed for the market"), "{err:#}");
}
