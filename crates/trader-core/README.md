# trader-core

## Overview

The monkeytrading bot's decision logic: a market's index mark derived from the public log, bars of
marks, the indicators, the six strategies that turn bar closes into a target position, the order
that moves a position to its target, and the controls that size and limit it: sizing, risk limits,
a margin buffer, positive cash, a minimum hold, and a market's trading hours. It does no IO, so it
is tested without a network, and a recorded log replays through it to the same decisions. Amounts are
the engine's `Fixed`, from the pinned `engine-core`.

## Features

- The index mark of a USD market priced from spot quotes, derived from the log's `Prices` commands
  with the engine's own `compute_index`, exact once the log has passed the market's quote age limit;
  a market with trading hours keeps only quotes from one open session and has no mark while shut
- Bars of whole windows of sequenced time, aligned to the Unix epoch, closed by the first command
  past them
- Indicators updated in constant time per close from fixed windows: simple and exponential moving
  averages, the relative strength index with Wilder's smoothing, Bollinger bands from running sums,
  and sliding highest and lowest values
- SMA crossover, EMA crossover, MACD signal-line crossover, RSI reversion, Bollinger band reversion
  and Donchian channel breakout, each choosing long, short or flat, with known-answer tests derived
  independently of the code; a `[strategy]` section names one by `kind`
- Sizing by a fixed USD notional or a fraction of equity, held to a leverage cap that must be below
  the market's own
- Risk limits per account: open positions, total notional, and a daily loss limit that flattens the
  account and stops it until the next UTC day
- A margin buffer and positive cash: no order adds exposure unless equity stays a configured share
  above the margin requirement worked out as the engine works it out, and cash is positive
- A minimum hold before a strategy closes a position, and no order while the market is shut or on the
  first bar after a closure
- The market order, side, size and reduce-only flag, that takes a position to its target
- No clocks, randomness, files, environment, or network: time and prices arrive as arguments

## Repository Structure

- `src/lib.rs`: the crate's documentation and modules
- `src/marks.rs`: `IndexMarks`, a market's stored venue quotes and index mark
- `src/bars.rs`: `Bar` and `BarBuilder`
- `src/sma.rs`, `src/ema.rs`, `src/rsi.rs`, `src/bands.rs`, `src/extremes.rs`: the indicators and
  their known-answer tests
- `src/strategy.rs`: `SmaCrossover`, `Target`, `ParamsError`, and `order_for`
- `src/strategies/`: the other strategies, `StrategyParams` (the config section), and `Strategy`
- `src/margin/`: equity and margin requirement as the engine works them out
- `src/sizing/`: `Sizing`, the size of a new position and its leverage cap
- `src/risk/`: `RiskParams` and the daily loss limit's `DayLoss`
- `src/hours/`: whether a market is open, and `GapTracker` for the first bar after a closure
- `src/controls/`: `Controls::decide`, the order a target becomes once the controls have had their say
- `tests/sma_crossover.rs`: the SMA crossover through the public API, from marks to orders, against
  answers worked out by hand
- `tests/strategies.rs`: the other strategies against answers recomputed independently
- `tests/strategy_params.rs`: the strategies' config sections, bounds and edge cases

## Usage

A library crate, used by `trader`:

```rust
let mut bars = BarBuilder::new(60_000, 0);
let mut strategy = params.build()?;
let mut controls = Controls::new(risk, sizing, restored_day)?;
if let Some(bar) = bars.on_command(ts, Some(mark))
    && let Some(target) = strategy.on_close(bar.close)
{
    match controls.decide(&account, &BarTarget { ts, market, spec: &spec, mark, target, gap, close_first, max_slippage_bps }) {
        Decision::Order(order) => { /* sign and send it */ }
        Decision::Flatten => { /* close every position */ }
        Decision::AtTarget | Decision::Refused(_) => {}
    }
}
```

## Configuration

The sections the crate defines, which `trader` reads from its config file; each refuses keys it does
not know, and decimal values are strings (`width = "1.5"`):

- `[strategy]`, with `kind` and that strategy's parameters: `sma_crossover` and `ema_crossover`
  (`fast`, `slow`), `macd` (`fast`, `slow`, `signal`: 12, 26, 9), `rsi_reversion` (`period`,
  `oversold`, `exit`, `overbought`: 14, 30, 50, 70), `bollinger_reversion` (`period`, `width`: 20, 2)
  and `donchian_breakout` (`entry`, `exit`: 20, 10)
- `[sizing]`, with `by = "fixed_notional"` and `notional`, or `by = "equity_fraction"` and `bps`, and
  `max_leverage` either way
- `[risk]`: `max_open_positions`, `max_total_notional`, `daily_loss` (`{ usd = "..." }` or
  `{ equity_bps = ... }`), `margin_buffer_bps` (default 10000) and `min_hold` (default `1h`)

## Testing

```bash
cargo test -p trader-core
```

## Authors

The monkeytrading contributors

## License

Licensed under the Apache License, Version 2.0; see `LICENSE`.
