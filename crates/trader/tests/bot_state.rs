//! The bot's state against the stand-in: what a split, a failed corporate action, a dividend, a position
//! in another market, a read behind the log and a state file that cannot be written do to it, and what a
//! restart keeps.

mod common;

use std::time::Duration;

use common::{
    MINUTE, StandIn, T0, account, account_view, btc_usd, command, config_with, day_file, next_seq, own_fill, posted,
    posts_after_settling, price_at, run, start, warm,
};
use serde_json::{Value, json};

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

    // One that appears while the bot runs halts it at its next read of the account.
    let live = StandIn::start().await;
    live.with(|i| {
        i.markets = json!([btc_usd(Some("99"))]);
        i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({})));
        i.head_seq = 10;
    });
    let (_stop, bot) = start(&live, dir.path(), &[]);
    warm(&live).await;
    live.with(|i| {
        let held = json!({ "2": { "qty": "-1", "entry": "5" } });
        i.accounts.insert(account().to_string(), account_view("1000", held, json!({})));
    });
    for ts in [T0 + 61_000, T0 + 100_000] {
        let seq = next_seq(&live);
        live.append((seq, 0), command(seq, ts, json!({ "type": "tick" }), json!([])));
    }
    let err = tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap_err();
    assert!(err.to_string().contains("does not trade or limit"), "{err:#}");
    assert!(live.with(|i| i.posts.is_empty()));
}

/// A bar of a day on a market with hours always spans a closure, so the bot refuses to start with one.
#[tokio::test]
async fn a_daily_bar_on_a_market_with_hours_is_refused() {
    let stand_in = StandIn::start().await;
    let dir = tempfile::tempdir().unwrap();
    let mut market = btc_usd(Some("99"));
    market["schedule"] =
        json!({ "kind": "weekly", "tz": "UTC", "windows": [{ "open": "Fri 06:00", "close": "Fri 12:00" }] });
    stand_in.with(|i| {
        i.markets = json!([market]);
        i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({})));
        i.head_seq = 10;
    });
    let err = run(config_with(&stand_in, dir.path(), "1d", &[]), async {}).await.unwrap_err();
    assert!(err.to_string().contains("hold no bar of 1440 minutes"), "{err:#}");
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
    // The buy fills while the account still reads flat, so the saved entry is not the position's yet.
    let seq = next_seq(&stand_in);
    stand_in.append((seq, 0), own_fill(seq, T0 + 2 * MINUTE + 100, &buy, "0.980392156"));
    price_at(&stand_in, T0 + 2 * MINUTE + 1_000, "102.5");
    posts_after_settling(&stand_in).await;
    assert_eq!(day_file(dir.path())["opened"], json!({ "1": T0 + 2 * MINUTE + 100 }));
    assert_eq!(day_file(dir.path())["entry"], "0");
    // A later read agrees, and the entry is kept too.
    stand_in.with(|i| {
        let position = json!({ "1": { "qty": "0.980392156", "entry": "102" } });
        i.accounts.insert(account().to_string(), account_view("1000", position, json!({})));
    });
    price_at(&stand_in, T0 + 2 * MINUTE + 7_000, "102.5");
    posts_after_settling(&stand_in).await;
    assert_eq!(day_file(dir.path())["entry"], "102");
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

/// While the saved state cannot be written, no order is signed; once it can, the bot trades again.
#[tokio::test]
async fn no_order_while_the_state_cannot_be_saved() {
    use std::os::unix::fs::PermissionsExt;
    let stand_in = StandIn::start().await;
    let dir = tempfile::tempdir().unwrap();
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99"))]);
        i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({})));
        i.head_seq = 10;
        i.nonce = 3;
    });
    let (stop, bot) = start(&stand_in, dir.path(), &[]);
    warm(&stand_in).await;
    price_at(&stand_in, T0 + MINUTE, "101");
    price_at(&stand_in, T0 + 2 * MINUTE, "102");
    let (_, buy) = posted(&stand_in, 1).await;
    // The buy fills while the state directory cannot be written.
    let state = dir.path().join("state");
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o555)).unwrap();
    stand_in.with(|i| {
        let position = json!({ "1": { "qty": "0.980392156", "entry": "102" } });
        i.accounts.insert(account().to_string(), account_view("1000", position, json!({})));
    });
    let seq = next_seq(&stand_in);
    stand_in.append((seq, 0), own_fill(seq, T0 + 2 * MINUTE + 100, &buy, "0.980392156"));
    for (minute, mid) in [(3, "90"), (4, "80"), (5, "79")] {
        price_at(&stand_in, T0 + minute * MINUTE, mid);
    }
    assert_eq!(posts_after_settling(&stand_in).await, 1, "the reversal waits for the state to be saved");
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o755)).unwrap();
    price_at(&stand_in, T0 + 6 * MINUTE, "78");
    price_at(&stand_in, T0 + 7 * MINUTE, "77");
    let (_, sell) = posted(&stand_in, 2).await;
    assert_eq!(sell["side"], "sell");
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), bot).await.unwrap().unwrap().unwrap();
}

/// A saved opening time is kept for the same position only: one on the other side, or at another entry,
/// is a new position, held from the restart; the same position is held from the time saved, long past.
#[tokio::test]
async fn a_saved_opening_time_belongs_to_one_position() {
    // Saved ten minutes before the start, with a five-minute hold: held long enough if it is kept.
    let long_ago = T0 - 10 * MINUTE;
    let cases = [(1, "102", true), (-1, "102", false), (1, "101", false)];
    for (side, entry, kept) in cases {
        let stand_in = StandIn::start().await;
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("state")).unwrap();
        let saved = json!({ "day": null, "opened": { "1": long_ago }, "side": side, "entry": entry });
        std::fs::write(dir.path().join("state").join(format!("{}.json", account())), saved.to_string()).unwrap();
        stand_in.with(|i| {
            i.markets = json!([btc_usd(Some("99"))]);
            let position = json!({ "1": { "qty": "0.980392156", "entry": "102" } });
            i.accounts.insert(account().to_string(), account_view("1000", position, json!({})));
            i.head_seq = 10;
            i.nonce = 3;
        });
        let (stop, mut bot) = start(&stand_in, dir.path(), &[("min_hold = \"0s\"", "min_hold = \"5m\"")]);
        tokio::select! {
            ended = &mut bot => panic!("the bot ended: {ended:?}"),
            () = warm(&stand_in) => {}
        }
        for (minute, mid) in [(1, "98"), (2, "97"), (3, "96")] {
            price_at(&stand_in, T0 + minute * MINUTE, mid);
        }
        let posts = posts_after_settling(&stand_in).await;
        // The stand-in fills nothing, so a bar free to reverse may reverse again at the next one.
        assert!(
            (posts > 0) == kept,
            "side {side}, entry {entry}: {posts} orders with the hold {}",
            if kept { "over" } else { "running" }
        );
        // The log passes the window of orders signed before the start, so the stop is quick.
        let seq = next_seq(&stand_in);
        stand_in.append((seq, 0), command(seq, T0 + 10 * MINUTE, json!({ "type": "tick" }), json!([])));
        stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(40), bot).await.unwrap().unwrap().unwrap();
    }
}

/// A sizing whose leverage cap is not below the market's own is refused at the start.
#[tokio::test]
async fn a_leverage_cap_at_the_markets_own_is_refused() {
    let stand_in = StandIn::start().await;
    let dir = tempfile::tempdir().unwrap();
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99"))]);
        i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({})));
        i.head_seq = 10;
    });
    let edits = [("max_leverage = \"50\"", "max_leverage = \"100\"")];
    let err = run(config_with(&stand_in, dir.path(), "1m", &edits), async {}).await.unwrap_err();
    assert!(format!("{err:#}").contains("not below the market's own"), "{err:#}");
}

/// A decision can change the saved state itself, as the first bar of a UTC day does by starting the day: if
/// that cannot be saved, the order the decision called for is not signed, and once it can be, it is.
#[tokio::test]
async fn no_order_when_a_decision_changes_state_that_cannot_be_saved() {
    use std::os::unix::fs::PermissionsExt;
    let midnight = T0 + 16 * 60 * MINUTE;
    let stand_in = StandIn::start().await;
    let dir = tempfile::tempdir().unwrap();
    stand_in.with(|i| {
        i.markets = json!([btc_usd(Some("99"))]);
        i.accounts.insert(account().to_string(), account_view("1000", json!({}), json!({})));
        i.head_seq = 10;
        i.nonce = 3;
    });
    // Four closes decide, so the first target is made by the tick that completes the bar before midnight.
    let (stop, bot) = start(&stand_in, dir.path(), &[("slow = 2", "slow = 4")]);
    warm(&stand_in).await;
    // The prices read the account in turn, the last 58 s before midnight, so the tick reads nothing and the
    // day is started by the decision alone.
    for (before, mid) in [(178_000, "101"), (118_000, "102"), (58_000, "103")] {
        price_at(&stand_in, midnight - before, mid);
    }
    posts_after_settling(&stand_in).await;
    let state = dir.path().join("state");
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o555)).unwrap();
    stand_in.with(|i| i.time_ms = (midnight + 1_000) as u64);
    let seq = next_seq(&stand_in);
    stand_in.append((seq, 0), command(seq, midnight + 1_000, json!({ "type": "tick" }), json!([])));
    assert_eq!(posts_after_settling(&stand_in).await, 0, "the day cannot be saved");
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o755)).unwrap();
    price_at(&stand_in, midnight + 30_000, "104");
    price_at(&stand_in, midnight + 61_000, "105");
    let (_, buy) = posted(&stand_in, 1).await;
    assert_eq!(buy["side"], "buy");
    assert_eq!(day_file(dir.path())["day"]["day"], midnight / 86_400_000);
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(40), bot).await.unwrap().unwrap().unwrap();
}
