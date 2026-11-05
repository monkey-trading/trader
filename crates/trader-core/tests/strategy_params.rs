//! The strategies' config sections and what they refuse, and the closes a strategy takes.

use engine_core::Fixed;
use trader_core::bands::MAX_CLOSE;
use trader_core::strategies::{Strategy, StrategyParams, after_unit_break, is_valid_close};
use trader_core::strategy::{ParamsError, Target};

fn fx(text: &str) -> Fixed {
    text.parse().unwrap()
}

fn parse(text: &str) -> Result<StrategyParams, toml::de::Error> {
    toml::from_str(text)
}

/// Strategies with conventional parameters take them as defaults; the two crossovers have none.
#[test]
fn conventional_parameters_are_the_defaults() {
    assert_eq!(parse("kind = 'macd'").unwrap(), StrategyParams::Macd { fast: 12, slow: 26, signal: 9 });
    assert_eq!(
        parse("kind = 'rsi_reversion'").unwrap(),
        StrategyParams::RsiReversion { period: 14, oversold: fx("30"), exit: fx("50"), overbought: fx("70") }
    );
    assert_eq!(
        parse("kind = 'bollinger_reversion'").unwrap(),
        StrategyParams::BollingerReversion { period: 20, width: fx("2") }
    );
    assert_eq!(parse("kind = 'donchian_breakout'").unwrap(), StrategyParams::DonchianBreakout { entry: 20, exit: 10 });
    assert!(parse("kind = 'sma_crossover'").is_err());
    assert!(parse("kind = 'ema_crossover'\nfast = 5").is_err());
}

/// A typo, an unknown strategy, a missing kind, and a float where a decimal string is due all fail.
#[test]
fn a_misspelt_section_is_refused() {
    assert!(parse("kind = 'macd'\nsignall = 9").is_err());
    assert!(parse("kind = 'rsi'").is_err());
    assert!(parse("period = 14").is_err());
    assert!(parse("kind = 'bollinger_reversion'\nwidth = 2.5").is_err());
    assert!(parse("kind = 'bollinger_reversion'\nwidth = '2.5'").is_ok());
}

fn build(text: &str) -> Result<Strategy, ParamsError> {
    parse(text).unwrap().build()
}

#[test]
fn parameters_out_of_range_are_refused() {
    let periods = |text| build(text).err();
    assert_eq!(periods("kind = 'ema_crossover'\nfast = 5\nslow = 5"), Some(ParamsError::Periods));
    assert_eq!(periods("kind = 'ema_crossover'\nfast = 0\nslow = 5"), Some(ParamsError::Periods));
    assert_eq!(periods("kind = 'ema_crossover'\nfast = 2\nslow = 10001"), Some(ParamsError::Periods));
    assert_eq!(periods("kind = 'macd'\nfast = 12\nslow = 12"), Some(ParamsError::Periods));
    assert_eq!(periods("kind = 'macd'\nsignal = 0"), Some(ParamsError::Period));
    assert_eq!(periods("kind = 'rsi_reversion'\nperiod = 0"), Some(ParamsError::Period));
    assert_eq!(periods("kind = 'rsi_reversion'\noversold = 0"), Some(ParamsError::Levels));
    assert_eq!(periods("kind = 'rsi_reversion'\noversold = 60"), Some(ParamsError::Levels));
    assert_eq!(periods("kind = 'rsi_reversion'\noverbought = 40"), Some(ParamsError::Levels));
    assert_eq!(periods("kind = 'rsi_reversion'\noverbought = 100"), Some(ParamsError::Levels));
    assert_eq!(periods("kind = 'rsi_reversion'\noversold = 50\nexit = 50\noverbought = 50"), Some(ParamsError::Levels));
    assert_eq!(periods("kind = 'bollinger_reversion'\nperiod = 1"), Some(ParamsError::BandPeriod));
    assert_eq!(periods("kind = 'bollinger_reversion'\nperiod = 1001"), Some(ParamsError::BandPeriod));
    assert_eq!(periods("kind = 'bollinger_reversion'\nwidth = 0"), Some(ParamsError::Width));
    assert_eq!(periods("kind = 'bollinger_reversion'\nwidth = 11"), Some(ParamsError::Width));
    assert_eq!(periods("kind = 'donchian_breakout'\nentry = 5\nexit = 6"), Some(ParamsError::ExitPeriod));
    assert_eq!(periods("kind = 'donchian_breakout'\nexit = 0"), Some(ParamsError::ExitPeriod));
    assert_eq!(periods("kind = 'donchian_breakout'\nentry = 10001\nexit = 5"), Some(ParamsError::ExitPeriod));
}

/// The widest accepted settings build, so a bound is the last value taken and not the first refused.
#[test]
fn the_bounds_themselves_are_accepted() {
    assert!(build("kind = 'bollinger_reversion'\nperiod = 1000\nwidth = 10").is_ok());
    assert!(build("kind = 'bollinger_reversion'\nperiod = 2\nwidth = '0.000000001'").is_ok());
    assert!(build("kind = 'donchian_breakout'\nentry = 10000\nexit = 10000").is_ok());
    assert!(build("kind = 'rsi_reversion'\noversold = '0.000000001'\noverbought = '99.999999999'").is_ok());
    assert!(build("kind = 'macd'\nfast = 9999\nslow = 10000\nsignal = 10000").is_ok());
}

/// A close that is not a valid mark is no data: it gives no target and leaves the strategy as it was.
#[test]
fn an_invalid_close_changes_nothing() {
    for text in [
        "kind = 'sma_crossover'\nfast = 1\nslow = 2",
        "kind = 'ema_crossover'\nfast = 1\nslow = 2",
        "kind = 'macd'\nfast = 1\nslow = 2\nsignal = 1",
        "kind = 'rsi_reversion'\nperiod = 2",
        "kind = 'bollinger_reversion'\nperiod = 2",
        "kind = 'donchian_breakout'\nentry = 2\nexit = 1",
    ] {
        let mut clean = build(text).unwrap();
        let mut disturbed = build(text).unwrap();
        for close in ["10", "11", "10.5", "12", "13", "12.5", "11"] {
            for bad in [Fixed::ZERO, fx("-1"), MAX_CLOSE + Fixed::from_raw(1), Fixed::MAX_INPUT] {
                assert_eq!(disturbed.on_close(bad), None, "{text}: {bad}");
            }
            assert_eq!(disturbed.on_close(fx(close)), clean.on_close(fx(close)), "{text} at {close}");
        }
    }
}

#[test]
fn the_valid_range_of_a_close() {
    assert!(is_valid_close(Fixed::from_raw(1)));
    assert!(is_valid_close(MAX_CLOSE));
    assert!(!is_valid_close(Fixed::ZERO));
    assert!(!is_valid_close(MAX_CLOSE + Fixed::from_raw(1)));
}

/// RSI reversion by hand: period 2 over 10, 9, 8, 9, 10, 11. Changes -1, -1 give index 0 (long, below
/// 30); +1 gives gain 0.5 over loss 0.5, index 50 (flat at the exit); +1 gives gain 0.75 over loss
/// 0.25, index 75 (short, above 70); +1 gives 0.875 over 0.125, index 87.5, so it stays short.
#[test]
fn rsi_reversion_by_hand() {
    let mut strategy = build("kind = 'rsi_reversion'\nperiod = 2").unwrap();
    let seen: Vec<_> = ["10", "9", "8", "9", "10", "11"].into_iter().map(|c| strategy.on_close(fx(c))).collect();
    use Target::*;
    assert_eq!(seen, [None, None, Some(Long), Some(Flat), Some(Short), Some(Short)]);
}

/// Donchian by hand with an entry channel of 3 closes and an exit channel of 1. The fourth close, 4,
/// is above the highest of 1, 2, 3: long. 3.5 is below the close before it, not below the lowest of
/// 2, 3, 4: flat. 3.6 is inside the channel of 3, 4, 3.5: still flat. 5 is above 4: long. 6 is above
/// 5, and the long stays. 5.5 is below 6: flat. 3 is below the lowest of 5, 6, 5.5: short. 3.5 is above
/// the close before it, not above the highest of 6, 5.5, 3: flat.
#[test]
fn donchian_by_hand() {
    let mut strategy = build("kind = 'donchian_breakout'\nentry = 3\nexit = 1").unwrap();
    let closes = ["1", "2", "3", "4", "3.5", "3.6", "5", "6", "5.5", "3", "3.5"];
    let seen: Vec<_> = closes.into_iter().map(|c| strategy.on_close(fx(c))).collect();
    use Target::*;
    let expected = [None, None, None, Some(Long), Some(Flat), Some(Flat), Some(Long), Some(Long), Some(Flat)];
    assert_eq!(seen[..9], expected);
    assert_eq!(seen[9..], [Some(Short), Some(Flat)]);
}

/// Bars to warm up on: the window for the windowed strategies, more for the exponential ones, and never
/// more than the longest period.
#[test]
fn warm_up_bars() {
    let bars = |text| parse(text).unwrap().warm_up_bars();
    assert_eq!(bars("kind = 'sma_crossover'\nfast = 5\nslow = 20"), 20);
    assert_eq!(bars("kind = 'ema_crossover'\nfast = 5\nslow = 20"), 60);
    assert_eq!(bars("kind = 'macd'"), 105);
    assert_eq!(bars("kind = 'rsi_reversion'"), 71);
    assert_eq!(bars("kind = 'bollinger_reversion'"), 20);
    assert_eq!(bars("kind = 'donchian_breakout'"), 21);
    assert_eq!(bars("kind = 'ema_crossover'\nfast = 5\nslow = 9000"), 10_000);
}

/// A close equal to a channel edge is not a breakout: 1, 2, 3 then 3 is flat, and so is 3, 2, 1 then 1.
/// A long also stays while a close only equals the exit channel's lowest: 1, 2, 3, 4 is long, and 4 again
/// does not leave it.
#[test]
fn donchian_edges_are_not_breakouts_and_exits_need_a_close_beyond() {
    let run = |closes: &[&str]| {
        let mut strategy = build("kind = 'donchian_breakout'\nentry = 3\nexit = 1").unwrap();
        closes.iter().map(|c| strategy.on_close(fx(c))).last().unwrap()
    };
    assert_eq!(run(&["1", "2", "3", "3"]), Some(Target::Flat));
    assert_eq!(run(&["3", "2", "1", "1"]), Some(Target::Flat));
    assert_eq!(run(&["1", "2", "3", "4"]), Some(Target::Long));
    assert_eq!(run(&["1", "2", "3", "4", "4"]), Some(Target::Long));
    assert_eq!(run(&["3", "2", "1", "0.5", "0.5"]), Some(Target::Short));
}

/// The levels are inclusive: an index of exactly 50 enters long at an oversold level of 50, and short at an
/// overbought level of 50.
#[test]
fn rsi_levels_are_inclusive() {
    let run = |levels: &str| {
        let mut strategy = build(&format!("kind = 'rsi_reversion'\nperiod = 2\n{levels}")).unwrap();
        ["10", "9", "10"].into_iter().map(|c| strategy.on_close(fx(c))).last().unwrap()
    };
    assert_eq!(run("oversold = 50\nexit = 50\noverbought = 60"), Some(Target::Long));
    assert_eq!(run("oversold = 40\nexit = 50\noverbought = 50"), Some(Target::Short));
    assert_eq!(run("oversold = 49\nexit = 50\noverbought = 51"), Some(Target::Flat));
}

/// A close on a band is not outside it: 2, 4, 4, 4, 5, 5, 7, 9 has bands 1 and 9 at two deviations, and its last close
/// is 9; 8, 6, 6, 6, 5, 5, 3, 1 has the same bands and ends on 1. Both stay flat.
#[test]
fn bollinger_bands_are_strict() {
    let run = |closes: &[&str]| {
        let mut strategy = build("kind = 'bollinger_reversion'\nperiod = 8\nwidth = 2").unwrap();
        closes.iter().map(|c| strategy.on_close(fx(c))).last().unwrap()
    };
    assert_eq!(run(&["2", "4", "4", "4", "5", "5", "7", "9"]), Some(Target::Flat));
    assert_eq!(run(&["8", "6", "6", "6", "5", "5", "3", "1"]), Some(Target::Flat));
}

/// A long leaves at the middle band exactly: closes 1, 2, 3, 1, 2 over three with one deviation are short
/// (3 is above 2.8165), long (1 is below 1.1835), and then flat, as 2 is the mean of 3, 1, 2.
#[test]
fn bollinger_leaves_a_position_at_the_middle() {
    let mut strategy = build("kind = 'bollinger_reversion'\nperiod = 3\nwidth = 1").unwrap();
    let seen: Vec<_> = ["1", "2", "3", "1", "2"].into_iter().map(|c| strategy.on_close(fx(c))).collect();
    use Target::*;
    assert_eq!(seen, [None, None, Some(Short), Some(Long), Some(Flat)]);
    let mut mirrored = build("kind = 'bollinger_reversion'\nperiod = 3\nwidth = 1").unwrap();
    let seen: Vec<_> = ["3", "2", "1", "3", "2"].into_iter().map(|c| mirrored.on_close(fx(c))).collect();
    assert_eq!(seen, [None, None, Some(Long), Some(Short), Some(Flat)]);
}

/// History starts after the last step of a split's size: 100 to 50 is a 2-for-1, 50 to 100 a reverse
/// split, and the ordinary moves around them, and a 39% fall, are not breaks.
#[test]
fn history_starts_after_a_split_sized_step() {
    let closes = |texts: &[&str]| texts.iter().map(|t| fx(t)).collect::<Vec<_>>();
    assert_eq!(after_unit_break(&closes(&["100", "101", "99", "100"])), 0);
    assert_eq!(after_unit_break(&closes(&["100", "101", "50", "51", "50.5"])), 2);
    assert_eq!(after_unit_break(&closes(&["50", "51", "102", "101"])), 2);
    assert_eq!(after_unit_break(&closes(&["100", "50", "100", "99"])), 2, "the last break counts");
    assert_eq!(after_unit_break(&closes(&["100", "61", "62"])), 0, "a fall of 39% is a move");
    assert_eq!(after_unit_break(&closes(&["100", "59.999999999"])), 1);
    assert_eq!(after_unit_break(&closes(&["100", "167", "168"])), 0, "a rise of 67% is a move");
    assert_eq!(after_unit_break(&closes(&["100", "167.000000001"])), 1);
    assert_eq!(after_unit_break(&closes(&["100"])), 0);
    assert_eq!(after_unit_break(&[]), 0);
}
