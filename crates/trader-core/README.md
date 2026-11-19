# trader-core

The bot's decision logic: a market's reference price derived from the public log, bars, the
indicators, the six strategies, and the controls that turn a strategy's target into an order or
into no order.

## What it is for

The [`trader`](../trader/README.md) binary feeds this crate the log's price updates and each
account's state, and sends the orders it returns. The crate performs no input or output: it reads
no clock, file, environment variable or network, and uses no randomness. Time and prices reach it
as arguments, so the strategies are tested without a network, and a recorded log replays through
it to the same decisions. Amounts are the engine's fixed-point `Fixed` type, from the pinned
`engine-core` crate, so the crate computes margin as the engine does.

## Strategies

Each strategy reads the closing price of each bar and chooses a target position: long, short or
flat. Its parameters are counted in bars. The crossover strategies are flat only while their two
lines are equal. The reversion and breakout strategies keep their target between the signals that
the table lists.

| `kind` | What it computes | Long | Short | Flat |
| --- | --- | --- | --- | --- |
| `sma_crossover` | Simple moving averages of the closes over `fast` and `slow` bars | While the fast average is above the slow one | While the fast average is below the slow one | While the two averages are equal |
| `ema_crossover` | Exponential moving averages over `fast` and `slow` bars | While the fast average is above the slow one | While the fast average is below the slow one | While the two averages are equal |
| `macd` | The MACD line, the difference of two exponential averages, and its signal line | While the MACD line is above its signal line | While it is below | While the two lines are equal |
| `rsi_reversion` | The relative strength index over `period` bars, with Wilder's smoothing | When the index falls to `oversold` | When the index rises to `overbought` | When the index reaches `exit` |
| `bollinger_reversion` | Bollinger bands, `width` standard deviations around the average of `period` closes | On a close below the lower band | On a close above the upper band | On a close that reaches the middle band |
| `donchian_breakout` | The highest and lowest of the previous closes | On a close above the highest of the previous `entry` closes | On a close below their lowest | A long position on a close below the lowest of the previous `exit` closes, and a short one on a close above their highest |

Each indicator is updated in constant time per bar, from a fixed window. The strategies' tests
check them against answers worked out independently of the code.
[Configuration](../../docs/configuration.md#accountstrategy) gives each strategy's parameters,
defaults and limits.

## Using it

```rust
let mut bars = BarBuilder::new(60_000, 0);
let mut strategy = params.build()?;
let mut controls = Controls::new(risk, sizing, restored_day)?;
if let Some(bar) = bars.on_command(ts, Some(mark))
    && let Some(target) = strategy.on_close(bar.close)
{
    let bar_target = BarTarget { ts, market, spec: &spec, mark, target, gap, close_first, max_slippage_bps };
    match controls.decide(&account, &bar_target) {
        Decision::Order(order) => { /* sign and send the order */ }
        Decision::Flatten => { /* close every position */ }
        Decision::AtTarget | Decision::Refused(_) => {}
    }
}
```

- `BarBuilder` closes a bar at the first instruction in the log past the bar's window. Windows are
  whole multiples of the bar length, aligned to the Unix epoch.
- `StrategyParams::build` builds a strategy from its config section.
- `Controls::decide` applies the account's controls to a target: the position's size, the leverage
  cap, the risk limits, the margin buffer, positive cash, the minimum holding time and the market's
  trading hours. It returns an order, a request to close every position when the daily loss limit
  is reached, or the reason for placing nothing.

## Layout

| Module | Responsibility |
| --- | --- |
| `marks` | A market's stored venue quotes and its reference price, derived as the engine derives it, with no price while a market with trading hours is closed |
| `bars` | Bars of the reference price over windows of the server's time |
| `sma`, `ema`, `rsi`, `bands`, `extremes` | The indicators |
| `strategy`, `strategies` | The strategies, their config section and their parameter checks, and the order that moves a position to its target |
| `sizing` | The size of a new position, and its leverage cap |
| `risk` | The risk limits, and the daily loss limit |
| `margin` | Equity and the margin requirement, as the engine computes them |
| `hours` | Whether a market is open, and the first bar after a closure |
| `controls` | The decision that turns a target into an order, a close of every position, or nothing |

Each module's own documentation describes its files.

## Testing

```shell
cargo test -p trader-core
```

The unit tests sit beside each module. The integration tests drive the crate through its public
API: `tests/sma_crossover.rs` from prices to orders against answers worked out by hand,
`tests/strategies.rs` against answers recomputed independently, and `tests/strategy_params.rs` for
the config sections, their bounds and their edge cases.

## License

Apache License 2.0; see [`LICENSE`](../../LICENSE).
