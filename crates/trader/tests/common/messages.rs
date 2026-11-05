//! Messages and listings the stand-in and the tests build.

use serde_json::{Value, json};

/// A command message of the log, as the node streams it.
pub fn command(seq: u64, ts: i64, cmd: Value, events: Value) -> Value {
    json!({ "offset": format!("{seq}:0"), "type": "command", "seq": seq, "ts": ts, "cmd": cmd, "events": events })
}

/// A `Prices` command for `market` quoting `mid` from three venues stamped `ts`.
pub fn prices(market: u32, mid: &str, ts: i64) -> Value {
    let quote = |source: &str| json!({ "source": source, "mid": mid, "ts": ts });
    json!({ "type": "prices", "market": market, "quotes": [quote("a"), quote("b"), quote("c")] })
}

/// BTC-USD as the dev config lists it, with its mark.
pub fn btc_usd(mark: Option<&str>) -> Value {
    market(1, "BTC-USD", mark)
}

/// ETH-USD as the dev config lists it, with its mark.
pub fn eth_usd(mark: Option<&str>) -> Value {
    market(2, "ETH-USD", mark)
}

/// A USD crypto market priced from spot quotes that never closes.
pub fn market(id: u32, symbol: &str, mark: Option<&str>) -> Value {
    json!({
        "id": id, "symbol": symbol, "asset_class": "crypto", "schedule": { "kind": "always_open" },
        "margin_tiers": [{ "bps": 100 }], "spread_tiers": [{ "bps": 0 }],
        "price_rules": { "max_age_ms": 1000, "max_divergence_bps": 25, "min_sources": 2 },
        "max_slippage_bps": 25, "financing": { "benchmark": "SOFR", "markup_bps": 500, "day_basis": 365 },
        "house_cap_bps": 3000, "house_cap_floor": "300000",
        "open": true, "reduce_only": false, "mark": mark, "fx_mark": null, "stress_level": 0
    })
}
