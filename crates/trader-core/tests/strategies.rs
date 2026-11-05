//! The strategies through the crate's public API against answers recomputed independently: a
//! Python script derived each target from the whole close history with `Decimal`, not from running
//! sums, rounding at nine places as the indicators document. The closes are a seeded random walk
//! of two-decimal prices.

use engine_core::Fixed;
use trader_core::strategies::{Strategy, StrategyParams};
use trader_core::strategy::Target::{self, Flat, Long, Short};

const CLOSES: [&str; 60] = [
    "98.46", "98.43", "99.37", "99.31", "100.88", "100.27", "101.88", "101.55", "101.31", "102.33", "102.05", "102.32",
    "100.36", "98.94", "97.67", "97.06", "98.39", "99.21", "99.91", "100.17", "101.93", "101.22", "100.98", "102.21",
    "103.15", "101.54", "101.93", "100.65", "101.37", "101.05", "101.53", "102.34", "101.25", "101.21", "100.48",
    "98.73", "99.13", "98.5", "99.99", "98.04", "97.63", "99.35", "100.23", "100.65", "102.45", "102.85", "102.2",
    "103.83", "105.13", "105", "106.21", "106.75", "106.77", "107.98", "109.17", "110.48", "110.02", "111.73", "110",
    "108.79",
];

const EMA_3_8: [Option<Target>; 60] = [
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Short),
    Some(Long),
    Some(Short),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
];

const MACD_3_6_4: [Option<Target>; 60] = [
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Long),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Long),
    Some(Short),
    Some(Short),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Short),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Short),
    Some(Long),
    Some(Short),
    Some(Short),
];

const RSI_5: [Option<Target>; 60] = [
    None,
    None,
    None,
    None,
    None,
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Flat),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Flat),
    Some(Flat),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Flat),
    Some(Flat),
    Some(Flat),
    Some(Flat),
    Some(Flat),
    Some(Flat),
    Some(Flat),
    Some(Flat),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Flat),
    Some(Flat),
    Some(Flat),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
];

const BOLL_8: [Option<Target>; 60] = [
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    Some(Flat),
    Some(Flat),
    Some(Flat),
    Some(Flat),
    Some(Flat),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Flat),
    Some(Flat),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Flat),
    Some(Flat),
    Some(Flat),
    Some(Flat),
    Some(Flat),
    Some(Flat),
    Some(Flat),
    Some(Flat),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Flat),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Flat),
];

const DON_6_3: [Option<Target>; 60] = [
    None,
    None,
    None,
    None,
    None,
    None,
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Flat),
    Some(Flat),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Flat),
    Some(Long),
    Some(Long),
    Some(Flat),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Flat),
    Some(Short),
    Some(Short),
    Some(Short),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Long),
    Some(Flat),
    Some(Flat),
];

fn fx(text: &str) -> Fixed {
    text.parse().unwrap()
}

/// The targets a strategy gives on each of `CLOSES`.
fn targets(params: &str) -> Vec<Option<Target>> {
    let params: StrategyParams = toml::from_str(params).unwrap();
    let mut strategy = params.build().unwrap();
    CLOSES.iter().map(|close| strategy.on_close(fx(close))).collect()
}

#[test]
fn ema_crossover_follows_the_averages() {
    assert_eq!(targets("kind = 'ema_crossover'\nfast = 3\nslow = 8"), EMA_3_8);
    // The last averages, recomputed: fast 109.570739532 over slow 108.982380805.
    let Strategy::EmaCrossover(mut strategy) = StrategyParams::EmaCrossover { fast: 3, slow: 8 }.build().unwrap()
    else {
        unreachable!()
    };
    CLOSES.iter().for_each(|close| {
        strategy.on_close(fx(close));
    });
    assert_eq!(strategy.averages(), (Some(fx("109.570739532")), Some(fx("108.982380805"))));
}

#[test]
fn macd_follows_the_signal_line() {
    assert_eq!(targets("kind = 'macd'\nfast = 3\nslow = 6\nsignal = 4"), MACD_3_6_4);
    let Strategy::Macd(mut strategy) = StrategyParams::Macd { fast: 3, slow: 6, signal: 4 }.build().unwrap() else {
        unreachable!()
    };
    CLOSES.iter().for_each(|close| {
        strategy.on_close(fx(close));
    });
    assert_eq!(strategy.lines(), (Some(fx("0.172586433")), Some(fx("0.652292950"))));
}

#[test]
fn rsi_reversion_enters_beyond_the_levels_and_leaves_at_the_exit() {
    let params = "kind = 'rsi_reversion'\nperiod = 5\noversold = 35\nexit = 50\noverbought = 65";
    assert_eq!(targets(params), RSI_5);
    let Strategy::RsiReversion(mut strategy) =
        StrategyParams::RsiReversion { period: 5, oversold: fx("35"), exit: fx("50"), overbought: fx("65") }
            .build()
            .unwrap()
    else {
        unreachable!()
    };
    CLOSES.iter().for_each(|close| {
        strategy.on_close(fx(close));
    });
    assert_eq!(strategy.index(), Some(fx("50.461955625")));
}

#[test]
fn bollinger_reversion_enters_outside_the_bands_and_leaves_at_the_middle() {
    assert_eq!(targets("kind = 'bollinger_reversion'\nperiod = 8\nwidth = '1.5'"), BOLL_8);
}

#[test]
fn donchian_breakout_enters_on_the_channel_and_leaves_on_the_shorter_one() {
    assert_eq!(targets("kind = 'donchian_breakout'\nentry = 6\nexit = 3"), DON_6_3);
}
