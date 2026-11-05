//! What the console reads, against the stand-in: each account's figures as the node's verified
//! answers give them, with each position's mark and close-out, realised profit and loss over the
//! account's whole fill history, read page by page and then only past the newest fill, the equity
//! curve, the totals, the last figures kept through a failed read
//! and cleared by an answer from another signer, and the live feed's snapshot and changes.

mod common;

use common::console::{Running, SAME, console_config, figures_view, seed, until_account, until_started};
use common::{Answers, StandIn, account, emissions_answer, history_fill};
use serde_json::{Value, json};

/// Seeds an account holding 0.5 BTC-USD from 100 and a resting order, with equity 1012.5 against a
/// requirement of 50, an entitlement of 5 (9 at the newest checkpoint), and three fills, two pages
/// of two at a time, realising 10 less a fee of 1, then -4, then 0.
fn seed_figures(stand_in: &StandIn) {
    seed(stand_in);
    let a = account().to_string();
    stand_in.with(|i| {
        let mut view = figures_view("1000", "1012.5", "50", json!({ "1": { "qty": "0.5", "entry": "100" } }));
        view["orders"] = json!({ "9": common::resting_order(9) });
        view["closeouts"] = json!([{ "market": 1, "reach": "at", "price": "60.5" }]);
        i.accounts.insert(a.clone(), view);
        i.emissions.insert(a.clone(), emissions_answer(&a, "5", "9"));
        i.history.insert(
            a.clone(),
            vec![
                history_fill(&a, "30:0", 3_000, "sell", "0", "0"),
                history_fill(&a, "20:1", 2_000, "buy", "-4", "0"),
                history_fill(&a, "20:0", 1_000, "sell", "10", "1"),
            ],
        );
        i.history_page = Some(2);
    });
}

/// The account's figures are the node's, with unrealised profit as equity less balance, the margin
/// level as equity over the requirement, realised profit over every fill less its fees, and MONKEY
/// as the node's entitlement with nothing claimed yet known; the totals sum them.
#[tokio::test]
async fn reads_match_the_nodes_answers() {
    let stand_in = StandIn::start().await;
    seed_figures(&stand_in);
    let dir = tempfile::tempdir().unwrap();
    let running = Running::start(console_config(&stand_in, dir.path(), "")).await;
    let cookie = running.session().await;
    until_started(&running, &cookie).await;
    let a = account().to_string();
    let view = until_account(&running, &cookie, &a, |view| view["figures"].is_object()).await;
    assert_eq!(view["address"], a);
    assert_eq!(view["error"], Value::Null);
    let figures = &view["figures"];
    for (field, value) in
        [("balance", "1000"), ("equity", "1012.5"), ("margin", "50"), ("unrealised_pnl", "12.5"), ("realised_pnl", "5")]
    {
        assert_eq!(figures[field], value, "{field}: {figures}");
    }
    assert_eq!(figures["margin_level_bps"], 202_500);
    assert_eq!(
        figures["positions"],
        json!([{ "market": 1, "symbol": "BTC-USD", "qty": "0.5", "entry": "100", "mark": "99",
            "unrealised_pnl": "-0.5", "closeout": { "reach": "at", "price": "60.5" } }])
    );
    assert_eq!(figures["orders"][0]["order_id"], 9);
    assert_eq!(figures["monkey"], json!({ "entitled": "5", "newest_entitled": "9", "claimed": null }));
    assert_eq!(view["strategies"][0]["market"], "BTC-USD");

    assert_eq!(view["curve"], json!([{ "time_ms": figures["time_ms"], "equity": "1012.5" }]));
    let fills = running.get(&format!("/api/accounts/{a}/fills"), &cookie).await.body;
    let cursors: Vec<_> = fills["fills"].as_array().unwrap().iter().map(|f| f["cursor"].clone()).collect();
    assert_eq!(cursors, [json!("30:0"), json!("20:1"), json!("20:0")]);
    assert_eq!(fills["fills"][2]["symbol"], "BTC-USD");
    assert_eq!(fills["fills"][2]["realised_pnl"], "10");

    let overview = running.get("/api/accounts", &cookie).await.body;
    assert_eq!(overview["accounts"].as_array().unwrap().len(), 1);
    assert_eq!(overview["accounts"][0]["figures"], *figures);
    let totals = &overview["totals"];
    assert_eq!((totals["accounts"].as_u64(), totals["reporting"].as_u64()), (Some(1), Some(1)));
    assert_eq!((&totals["equity"], &totals["realised_pnl"]), (&json!("1012.5"), &json!("5")));
    assert_eq!(totals["margin_level_bps"], 202_500);

    // A new fill counts once: later reads ask only for fills past the newest one read.
    stand_in.with(|i| {
        i.history.get_mut(&a).unwrap().insert(0, history_fill(&a, "31:0", 4_000, "buy", "2.5", "0.5"));
    });
    until_account(&running, &cookie, &a, |view| view["figures"]["realised_pnl"] == "7").await;
    let reads = stand_in.with(|i| i.history_reads);
    until_account(&running, &cookie, &a, |_| stand_in.with(|i| i.history_reads) >= reads + 2).await;
    assert_eq!(running.get(&format!("/api/accounts/{a}"), &cookie).await.body["figures"]["realised_pnl"], "7");
    let fills = running.get(&format!("/api/accounts/{a}/fills"), &cookie).await.body;
    assert_eq!(fills["fills"].as_array().unwrap().len(), 4);
    assert_eq!(fills["fills"][0]["cursor"], "31:0");

    assert_eq!(running.get("/api/accounts/0x0000000000000000000000000000000000000001", &cookie).await.status, 404);
    assert_eq!(running.get("/api/accounts/not-an-address/fills", &cookie).await.status, 404);
    running.stop_after(&stand_in).await.unwrap();
}

/// An unverifiable answer keeps the last figures and says why; one signed by another key clears
/// them; verified answers bring them back.
#[tokio::test]
async fn a_failed_read_keeps_the_last_figures_and_another_signer_clears_them() {
    let stand_in = StandIn::start().await;
    seed_figures(&stand_in);
    let dir = tempfile::tempdir().unwrap();
    let running = Running::start(console_config(&stand_in, dir.path(), "")).await;
    let cookie = running.session().await;
    until_started(&running, &cookie).await;
    let a = account().to_string();
    until_account(&running, &cookie, &a, |view| view["figures"].is_object()).await;

    stand_in.with(|i| i.answers = Some(Answers::Unsigned));
    let view = until_account(&running, &cookie, &a, |view| view["error"].is_string()).await;
    assert!(view["error"].as_str().unwrap().contains("does not verify"), "{view}");
    assert_eq!(view["figures"]["equity"], "1012.5", "the last figures stay");

    stand_in.with(|i| i.answers = Some(Answers::OtherSigner));
    let view = until_account(&running, &cookie, &a, |view| view["figures"].is_null()).await;
    assert!(view["error"].as_str().unwrap().contains("not signed by the configured signer"), "{view}");
    let fills = running.get(&format!("/api/accounts/{a}/fills"), &cookie).await.body;
    assert_eq!(fills["fills"], json!([]));

    stand_in.with(|i| i.answers = Some(Answers::Signed));
    let view = until_account(&running, &cookie, &a, |view| view["figures"].is_object()).await;
    assert_eq!((&view["error"], &view["figures"]["realised_pnl"]), (&Value::Null, &json!("5")), "read afresh");
    running.stop_after(&stand_in).await.unwrap();
}

/// The live feed opens with every account and the totals, then sends what changes: the account with
/// its equity curve, its fills, and the totals. It closes once the session ends.
#[tokio::test]
async fn the_live_feed_follows_the_accounts() {
    let stand_in = StandIn::start().await;
    seed_figures(&stand_in);
    let dir = tempfile::tempdir().unwrap();
    let running = Running::start(console_config(&stand_in, dir.path(), "")).await;
    let cookie = running.session().await;
    until_started(&running, &cookie).await;
    let a = account().to_string();
    until_account(&running, &cookie, &a, |view| view["figures"].is_object()).await;

    let mut feed = running.live(SAME, Some(&cookie)).await.unwrap();
    let snapshot = feed.next_of("snapshot").await;
    assert_eq!(snapshot["accounts"][0]["address"], a);
    assert_eq!(snapshot["accounts"][0]["figures"]["equity"], "1012.5");
    assert_eq!(snapshot["totals"]["reporting"], 1);

    stand_in.with(|i| {
        i.accounts.get_mut(&a).unwrap()["equity"] = json!("1020");
        i.history.get_mut(&a).unwrap().insert(0, history_fill(&a, "40:0", 5_000, "sell", "1", "0"));
    });
    // One refresh may read the new equity and the next the new fill, so they may come in either tick.
    // The stand-in's time stands still, so the curve's one point takes the new equity.
    let (mut equity, mut fill) = (false, false);
    while !(equity && fill) {
        let message = feed.next().await.expect("the feed stays open");
        match message["type"].as_str() {
            Some("account") => {
                let account = &message["account"];
                equity |= account["figures"]["equity"] == "1020" && account["curve"][0]["equity"] == "1020";
            }
            Some("fills") => fill |= message["address"] == a && message["fills"][0]["cursor"] == "40:0",
            _ => {}
        }
    }
    let totals = feed.next_of("totals").await;
    assert_eq!(totals["totals"]["equity"], "1020");

    running.post(&format!("/api/accounts/{a}/stop"), &cookie).await;
    let stopped = feed.next_of("account").await;
    assert_eq!(stopped["account"]["state"], "stopped");

    let other = running.session().await;
    assert_eq!(running.post("/api/logout", &cookie).await.status, 204);
    while feed.next().await.is_some() {}
    let mut kept = running.live(SAME, Some(&other)).await.unwrap();
    assert_eq!(kept.next_of("snapshot").await["accounts"][0]["state"], "stopped");
    running.stop_after(&stand_in).await.unwrap();
}
