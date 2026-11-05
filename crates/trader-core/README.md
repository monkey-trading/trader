# trader-core

## Overview

The monkeytrading bot's decision logic: a market's index mark derived from the public log, bars of
marks, the moving-average indicator, the SMA crossover strategy, and the order that moves a
position to the strategy's target. It does no IO, so it is tested without a network, and a recorded
log replays through it to the same decisions. Amounts are the engine's `Fixed`, from the pinned
`engine-core`.

## Features

- The index mark of a USD market priced from spot quotes that never closes, derived from the log's
  `Prices` commands as the engine's `compute_index` derives it (the same median, without
  allocating, checked against it), exact once the log has passed the
  market's quote age limit
- Bars of whole windows of sequenced time, aligned to the Unix epoch, closed by the first command
  past them
- A simple moving average updated in constant time from a fixed window
- A series of one market's bars at one bar length with an average of each period asked for, shared
  by every strategy that reads it
- An SMA crossover that targets long, short or flat, comparing the averages exactly
- The market order, side, size and reduce-only flag, that takes a position to its target
- No clocks, randomness, files, environment, or network: time and prices arrive as arguments

## Repository Structure

- `src/lib.rs`: the crate's documentation and modules
- `src/marks.rs`: `IndexMarks`, a market's stored venue quotes and index mark
- `src/bars.rs`: `Bar` and `BarBuilder`
- `src/sma.rs`: `Sma`, the simple moving average, and its known-answer tests
- `src/series.rs`: `Series`, the bars and averages many strategies share
- `src/strategy.rs`: `SmaCrossover`, `Target`, `crossover_target`, and `order_for`
- `tests/sma_crossover.rs`: the strategy through the public API, from marks to orders, against
  answers worked out by hand

## Usage

A library crate, used by `trader`:

```rust
let mut bars = BarBuilder::new(60_000, 0);
let mut strategy = SmaCrossover::new(5, 20)?;
if let Some(bar) = bars.on_command(ts, Some(mark))
    && let Some(target) = strategy.on_close(bar.close)
{
    let order = order_for(target, size, position, false);
}
```

## Testing

```bash
cargo test -p trader-core
```

## Authors

The monkeytrading contributors

## License

Licensed under the Apache License, Version 2.0; see `LICENSE`.
