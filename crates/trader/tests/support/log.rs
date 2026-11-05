//! Messages of the public log as the node streams them, and fleets of accounts to read them: what
//! the fleet, allocation and benchmark tests drive `Fleet::apply` with, with no node and no network.

#![allow(dead_code)]

use std::sync::Arc;

use std::collections::BTreeMap;

use engine_core::Fixed;
use engine_core::account::Position;
use engine_core::market::{MarketSpec, PriceRules};
use engine_core::types::Millis;
use k256::ecdsa::SigningKey;
use serde_json::json;
use trader::bot::day::{Saved, StateFile};
use trader::bot::fleet::{AccountIn, Fleet, MarketIn, StrategyIn};
use trader::keys::AccountKey;
use trader::node::api::AccountView;
use trader::stream::message::Offset;
use trader_core::controls::Controls;
use trader_core::strategies::StrategyParams;

/// A minute boundary.
pub const T0: Millis = 1_800_000_000_000;
pub const MINUTE: Millis = 60_000;
pub const RULES: PriceRules = PriceRules { max_age_ms: 1_000, max_divergence_bps: 25, min_sources: 2 };

/// The listing of market `id`: a USD crypto market that never closes, with `RULES`.
pub fn spec(id: u32) -> MarketSpec {
    serde_json::from_value(json!({
        "id": id, "symbol": format!("M{id}-USD"), "asset_class": "crypto", "schedule": { "kind": "always_open" },
        "margin_tiers": [{ "bps": 100 }], "spread_tiers": [{ "bps": 0 }],
        "price_rules": { "max_age_ms": 1000, "max_divergence_bps": 25, "min_sources": 2 },
        "max_slippage_bps": 25, "financing": { "benchmark": "SOFR", "markup_bps": 500, "day_basis": 365 },
        "house_cap_bps": 3000, "house_cap_floor": "300000"
    }))
    .unwrap()
}

/// The markets `1..=n`, each listed with `RULES`.
pub fn markets(n: u32) -> Vec<MarketIn> {
    (1..=n).map(|id| MarketIn { spec: spec(id), mark: None }).collect()
}

/// An SMA crossover of `fast` and `slow` bars.
pub fn sma(fast: usize, slow: usize) -> StrategyParams {
    StrategyParams::SmaCrossover { fast, slow }
}

/// The key with secret `n + 1` (the zero secret is no key).
pub fn key(n: u64) -> Arc<AccountKey> {
    let mut secret = [0u8; 32];
    secret[24..].copy_from_slice(&(n + 1).to_be_bytes());
    assert!(SigningKey::from_slice(&secret).is_ok());
    Arc::new(AccountKey::from_secret(&secret).unwrap())
}

/// One strategy: `market`, `bar_ms` bars, an SMA crossover of 1 bar over 2, flat.
pub fn strategy(market: u32, bar_ms: Millis) -> StrategyIn {
    StrategyIn { market, bar_ms, params: sma(1, 2), max_slippage_bps: 25, position: Fixed::ZERO, entry: Fixed::ZERO }
}

/// Controls that size every new position at 100 USD and hold no account back.
pub fn controls() -> Controls {
    controls_with("50")
}

/// `controls` with this cap on leverage.
pub fn controls_with(max_leverage: &str) -> Controls {
    let sizing =
        serde_json::from_value(json!({ "by": "fixed_notional", "notional": "100", "max_leverage": max_leverage }));
    let risk = serde_json::from_value(json!({
        "max_open_positions": 16, "max_total_notional": "1000000", "daily_loss": { "usd": "100000" },
        "margin_buffer_bps": 0, "min_hold": "0s"
    }));
    Controls::new(risk.unwrap(), sizing.unwrap(), None).unwrap()
}

/// An account with these strategies.
pub fn account(n: u64, strategies: Vec<StrategyIn>) -> AccountIn {
    let key = key(n);
    AccountIn {
        state_file: StateFile::new(std::path::Path::new("/nonexistent"), key.address()),
        key,
        close_on_stop: false,
        balance: "1000".parse().unwrap(),
        nonce: Some(0),
        controls: controls(),
        saved: Saved::default(),
        strategies,
    }
}

/// Reads every account the fleet wants read as the node would answer: with the positions the log
/// gives, entered at 100, and a balance of 1,000; then decides what each wanted. Returns how many.
pub fn settle(fleet: &mut Fleet) -> usize {
    settle_with(fleet, false).0
}

/// `settle`, also returning the most allocations any one read cost to take and decide from: the views
/// the node would answer with are built outside the count.
pub fn settle_counting(fleet: &mut Fleet) -> (usize, u64) {
    settle_with(fleet, true)
}

fn settle_with(fleet: &mut Fleet, counting: bool) -> (usize, u64) {
    let (mut settled, mut allocations) = (0, 0);
    while let Some(account) = fleet.reads.pop_front() {
        let entry = &mut fleet.accounts[account as usize];
        entry.read_wanted = false;
        let positions: BTreeMap<_, _> = entry
            .assignments
            .clone()
            .filter_map(|a| {
                let asg = &fleet.assignments[a as usize];
                (!asg.position.is_zero())
                    .then(|| (asg.market, Position { qty: asg.position, entry: "100".parse().unwrap() }))
            })
            .collect();
        let view = AccountView { balance: "1000".parse().unwrap(), positions, orders: BTreeMap::new() };
        let take = |fleet: &mut Fleet| {
            if fleet.apply_view(account, &view) {
                fleet.decide_account(account);
            }
        };
        if counting {
            allocations = allocations.max(allocation_counter::measure(|| take(fleet)).count_total);
        } else {
            take(fleet);
        }
        settled += 1;
    }
    (settled, allocations)
}

/// The offset of seq `seq`'s command.
pub fn at(seq: u64) -> Offset {
    Offset { seq, index: 0 }
}

/// A command message as the node streams it.
pub fn command(seq: u64, ts: Millis, cmd: &str, events: &str) -> String {
    format!(r#"{{"offset":"{seq}:0","type":"command","seq":{seq},"ts":{ts},"cmd":{cmd},"events":{events}}}"#)
}

/// A `Prices` command for `market` quoting `mid` from three venues stamped `ts`.
pub fn prices(seq: u64, ts: Millis, market: u32, mid: &str) -> String {
    let quote = |source: &str| format!(r#"{{"source":"{source}","mid":"{mid}","ts":{ts}}}"#);
    let cmd =
        format!(r#"{{"type":"prices","market":{market},"quotes":[{},{},{}]}}"#, quote("a"), quote("b"), quote("c"));
    command(seq, ts, &cmd, "[]")
}

/// A `Tick` command: time passes.
pub fn tick(seq: u64, ts: Millis) -> String {
    command(seq, ts, r#"{"type":"tick"}"#, "[]")
}

/// An order of `account` with `signature` that fills, leaving `position`.
pub fn order_fill(seq: u64, ts: Millis, account: &str, signature: &str, market: u32, position: &str) -> String {
    let cmd = format!(
        r#"{{"type":"place_order","account":"{account}","client_id":7,"market":{market},"side":"buy","qty":"1","kind":{{"kind":"market","quoted":"100","max_slippage_bps":25}},"reduce_only":false,"auth":{{"nonce":0,"expires_at":{},"signature":"{signature}"}}}}"#,
        ts + 30_000
    );
    let fill = format!(
        r#"{{"type":"fill","account":"{account}","order_id":{seq},"market":{market},"side":"buy","qty":"1","price":"100","index":"100","spread_bps":0,"realised_pnl":"0","profit_fee":"0","position":"{position}","liquidation":false}}"#
    );
    command(seq, ts, &cmd, &format!("[{fill}]"))
}

/// A history bucket of the marks, log-derived.
pub fn bucket(ts: Millis, close: &str) -> trader::node::api::MarkBucket {
    trader::node::api::MarkBucket { ts, close: close.parse().unwrap(), provenance: "log_derived".into() }
}

/// Makes every market's index exact and warms every series up from a history that holds `mark` in
/// minute 0, returning the next seq. The index is exact once more than `max_age_ms` of the log has
/// passed, and the history is read five seconds after.
pub fn warm(fleet: &mut Fleet, markets: &[u32], mark: &str) -> u64 {
    let mut seq = 10;
    for (ts, tick_only) in [(T0, false), (T0 + 1_001, false), (T0 + 6_002, true)] {
        for &market in markets {
            let msg = if tick_only { tick(seq, ts) } else { prices(seq, ts, market, mark) };
            fleet.apply(&msg, at(seq), u64::try_from(ts).unwrap()).unwrap();
            seq += 1;
        }
    }
    for request in std::mem::take(&mut fleet.warm_requests) {
        fleet.warmed(request.series, request.generation, Some(&[bucket(T0, mark)]), T0 + 6_002);
    }
    seq
}

/// One message of a synthetic log: its seq, its command's time and its text.
pub struct Entry {
    pub seq: u64,
    pub ts: Millis,
    pub msg: String,
}

/// `seconds` of a log after `warm`, in the shapes the node streams: every market in `1..=markets`
/// priced from three venues every 500 ms by a random walk around 1000, a stranger's order that
/// fills and one that is refused every five seconds, and a tick every ten.
pub fn hour(markets: u32, seconds: i64, first_seq: u64) -> Vec<Entry> {
    let stranger = "0x00000000000000000000000000000000000000aa";
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut walk = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        i64::try_from(seed % 11).unwrap() - 5
    };
    let mut mids: Vec<i64> = (0..i64::from(markets)).map(|m| 100_000 + 100 * m).collect();
    let (mut seq, mut out) = (first_seq, Vec::new());
    for step in 0..seconds * 2 {
        let ts = T0 + 10_000 + step * 500;
        for market in 1..=markets {
            let mid = &mut mids[market as usize - 1];
            *mid += walk();
            out.push(Entry { seq, ts, msg: prices(seq, ts, market, &format!("{}.{:02}", *mid / 100, *mid % 100)) });
            seq += 1;
        }
        if step % 10 == 0 {
            let sig = format!("0x{}", "ab".repeat(65));
            out.push(Entry { seq, ts, msg: order_fill(seq, ts, stranger, &sig, 1, "1") });
            seq += 1;
            let rejected = format!(
                r#"[{{"type":"order_rejected","account":"{stranger}","client_id":7,"reason":{{"kind":"expired","expires_at":{ts}}}}}]"#
            );
            out.push(Entry {
                seq,
                ts,
                msg: command(
                    seq,
                    ts,
                    r#"{"type":"cancel_all","account":"0x00000000000000000000000000000000000000aa"}"#,
                    &rejected,
                ),
            });
            seq += 1;
        }
        if step % 20 == 0 {
            out.push(Entry { seq, ts, msg: tick(seq, ts) });
            seq += 1;
        }
    }
    out
}

/// `n` accounts over markets `1..=3`, as a mix of strategies: bar lengths of 1, 5 and 15 minutes,
/// averages of 2 and 5, 3 and 8 or 5 and 20 bars, and one or two markets each.
pub fn mixed(n: u64) -> Vec<AccountIn> {
    const BARS: [Millis; 3] = [MINUTE, 5 * MINUTE, 15 * MINUTE];
    const PERIODS: [(usize, usize); 3] = [(2, 5), (3, 8), (5, 20)];
    (0..n)
        .map(|i| {
            let pick = |k: u64| usize::try_from((i / 3_u64.pow(k as u32)) % 3).unwrap();
            let mut strategies = vec![StrategyIn {
                params: sma(PERIODS[pick(1)].0, PERIODS[pick(1)].1),
                ..strategy(1 + (i % 3) as u32, BARS[pick(0)])
            }];
            if i % 4 == 0 {
                strategies.push(StrategyIn {
                    params: StrategyParams::EmaCrossover { fast: PERIODS[pick(2)].0, slow: PERIODS[pick(2)].1 },
                    ..strategy(1 + ((i + 1) % 3) as u32, BARS[pick(2)])
                });
            }
            account(i, strategies)
        })
        .collect()
}
