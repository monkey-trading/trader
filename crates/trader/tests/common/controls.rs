//! Helpers the controls tests share: scripted prices, the log's next seq, the state file, and the bot's start.

use serde_json::{Value, json};
use tokio::sync::oneshot;

use super::{MINUTE, StandIn, account, command, config_with, prices, run};

pub const DAY: i64 = 24 * 60 * MINUTE;

/// A prices command at `ts`.
pub fn price_at(stand_in: &StandIn, ts: i64, mid: &str) {
    let seq = next_seq(stand_in);
    stand_in.append((seq, 0), command(seq, ts, prices(1, mid, ts), json!([])));
}

/// The seq after the log's last message, or the head.
pub fn next_seq(stand_in: &StandIn) -> u64 {
    stand_in.with(|i| i.log.last().map_or(i.head_seq, |((seq, _), _)| *seq) + 1)
}

/// The log's entry for the close-all request `body` the bot posted, which closed the position at `ts`.
pub fn own_close(seq: u64, ts: i64, body: &Value) -> Value {
    let fill = json!({ "type": "fill", "account": account(), "order_id": seq, "market": 1, "side": "sell",
        "qty": "1", "price": "95", "index": "95", "spread_bps": 0, "realised_pnl": "-4", "profit_fee": "0",
        "position": "0", "liquidation": false });
    let evidence = json!({ "result": "failed", "reason": "unavailable" });
    let cmd = json!({ "type": "access_close_all", "request": body, "evidence": evidence });
    command(seq, ts, cmd, json!([fill]))
}

/// How many requests the stand-in has seen, after letting the bot catch up with the log.
pub async fn posts_after_settling(stand_in: &StandIn) -> usize {
    let last = stand_in.with(|i| i.log.last().map(|(offset, _)| *offset));
    if let Some(last) = last {
        stand_in.applied(last).await;
    }
    stand_in.with(|i| i.posts.len())
}

/// The state file the bot keeps for the test account.
pub fn day_file(dir: &std::path::Path) -> Value {
    let text = std::fs::read_to_string(dir.join("state").join(format!("{}.json", account()))).unwrap();
    serde_json::from_str(&text).unwrap()
}

pub fn start(
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
