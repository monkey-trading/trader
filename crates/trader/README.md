# trader

## Overview

The monkeytrading bot's binary. `trader run --config <file>` trades one or many accounts, each its
own funded key with its own markets and a strategy in each, against a monkeytrading node, in one
process over one stream connection. `trader addresses --config <file>` prints the accounts'
addresses, for funding and declaring them. Every answer the node signs and every frame of its
public log stream is checked against the configured enclave signer before the bot acts on it; an
answer that fails the check is an error, never data, and one signed by another key stops the bot.

## Features

- Signed REST calls to the node over TLS 1.3, pinned to the node's TLS key hash or checked against
  the public roots, each under a fresh challenge and verified with `engine-wire`
- One signed stream connection that reads the public log, verifies each frame with
  `FrameVerifier`, refuses gaps, and resumes after the last message it read when a connection
  fails or falls silent; a frame or message it cannot read stops the bot
- A halt, for an answer or frame signed by another key, a log message or listing the bot cannot
  follow, or every account having stopped, changes nothing: no cancel and no close, so the accounts
  are left as they are for their users, with a Critical alert and a non-zero exit
- Many accounts from `[[account]]` tables or directories of keystores (`[[account_dir]]`), each with
  its own strategies. One `/v1/stream` connection serves all of them: the log is read once, each
  `Prices` command moves its market's index once, and each market and bar length has one series of
  bars, on which each strategy of one set of parameters runs once, however many accounts trade on
  it. A bar that completes gives each account that reads it a target, which its controls turn into
  an order or none
- Startup reads of the markets and each account's positions and balance; from then on each
  position follows its account's fills in the log, and the account is read again after fills, at each decision to trade, and every minute. A
  fill from an order this process did not sign, once orders signed before it started have
  expired, means someone else trades the account, and stops that account (`account_shared`),
  not the others. A fill is the bot's when the command that made it carries one of the signatures
  it made for that account. An account that cannot start (a key that will not unlock, an account
  the node never credited, an empty balance, a market it cannot follow) or that the node refuses is
  reported (`account_failed`) and stops alone; the others carry on, and the run exits non-zero
- Requests share a small pool of kept-open HTTP/1.1 connections and are paced under the node's
  limits, set in `[node.limits]` from its `api.per_ip` and `api.per_account`: when a burst of
  orders exceeds them they queue in the order their bars completed rather than fail, each signed only
  once the limits let it go, and a
  retry-later answer (429, or 503, signed or not) holds every request back for the delay it names;
  a read is sent again after it, an order is not
- Each account tracks its own nonce, moved on after an order the engine used, and read again from
  `/v1/accounts/{address}/nonce` after any refusal, rejection or unanswered order. One request is out
  per account at a time, so nonces reach the node in order
- The node's time for an order is counted on from a verified answer to a request sent after the
  bar completed, on the monotonic clock, not read again for every order
- Five seconds of log after the index the bot derives is exact, a warm-up from the node's
  log-derived marks history, at the widest resolution that tiles a bar: bars before the one the
  index became exact in come from the history, that one starts from the history's bucket in its
  window, and the moves the bot saw since follow
- Bars built only from moves of the mark, as the node's marks history records them, so bars warmed
  up from the history and bars built live agree
- Each target passes the account's controls, applied to a read of the account that is of the log, before it becomes an order: its size from the sizing,
  the risk limits (open positions, total notional, and a daily loss that flattens the account and
  keeps it stopped until the next UTC day, across restarts), a margin buffer over the engine's
  requirement, positive cash, a minimum hold, and the market's trading hours. A market with hours
  is followed as the engine prices it, a halt or closure leaves no order, and the first bar after
  one is a gap; a split or a failed corporate action in a traded market halts the bot, and a warm-up
  starts after the last step in the history of 40% down or 67% up, which only a split makes. A split
  smaller than 2-for-1, or one while the bot is stopped, is not seen
- Signed market orders toward each strategy's target, on the account's next nonce, quoted at the
  last mark; a declined order is logged with the node's reason and retried on the next bar, and a
  reversal the engine refuses whole is retried as a reduce-only close, then an opening order. No
  order goes out for a bar more than half a minute behind the node's time, by its frame or by the
  node's time when it is sent, or before the log has passed the expiry of the last order for that
  account and market, or of any order an earlier process may have signed, so an order's fill is
  always in the position before the next decision. An order never exceeds the engine's input bound
- On SIGINT or SIGTERM, once the log has passed every order that could still enter it, a cancel of
  each account's resting orders and, when configured, a close of its positions at the node's current
  marks (read again when two seconds old, and each market under its own strategy's slippage bound),
  repeated on fresh nonces while a close leaves part of a position (the node closes at most
  a billion units a request), for at most a minute; a stop that leaves either behind, or cannot
  confirm it did not, exits non-zero with an alert. A stop can take the order expiry window (30 s),
  a few calls of `request_timeout` per account, paced under the limits, and a further close's, and a
  second signal does not cut it short, so a supervisor's
  grace period should be longer, the more so with many accounts
- After warm-up, `Fleet::apply` allocates at most twice per log message, for any number of accounts,
  both times in `serde_json` reading the message's nesting (`tests/alloc.rs` holds every message of
  a synthetic log to it, and the decision an account's read leads to, once, apart; the stream's frame verification is `engine-wire`'s), and `tests/hot_path.rs` replays an hour of a synthetic log through 1,000
  accounts and holds the time per message and the memory per account to budgets
- The account keys from an encrypted Web3 Secret Storage keystore, or, in a dev build, a hex file;
  held only as a signing key that zeroes its secret on drop, and never printed. A release build
  refuses the keys whose private halves this repository publishes

## Repository Structure

- `src/main.rs`: the entry point
- `src/lib.rs`: the command line and the runtime
- `src/config/mod.rs`: the config file's sections and loading
- `src/config/validation.rs`: parsing of the node URL and TLS key hash, and the config's checks
- `src/config/tests.rs`: the config tests, against `config/example.toml`
- `src/keys.rs`: `AccountKey`: unlocking a keystore or a dev hex file, and signing
- `src/keys/tests.rs`: the key tests
- `src/node/mod.rs`: `Node`, the signed and verified REST calls, paced and over pooled connections
- `src/node/pool.rs`: the pool of kept-open HTTP/1.1 connections
- `src/node/pace.rs`: the request pacer, and the node's time counted on from its answers
- `src/node/api.rs`: the routes the bot uses and their answers
- `src/node/tls.rs`: the pinned or publicly rooted TLS connections
- `src/stream/mod.rs`: `Stream`, the verified, gap-free, resuming log
- `src/stream/message.rs`: the stream's messages, read as borrowed views
- `src/bot/mod.rs`: `run`: startup, the run, the stop, and the exit status
- `src/bot/apply.rs`: applying one message of the log: prices, listings, bars, and events
- `src/bot/decide.rs`: a bar's targets turned into orders by each account's controls, the daily loss
  limit, and what is kept of each account
- `src/bot/day.rs`: an account's day loss and positions' opening times kept in a file
- `src/bot/fleet.rs`: `Fleet`: the accounts, markets and series, and applying one log message
  (no IO)
- `src/bot/warm.rs`: the warm-up of each series from the marks history
- `src/bot/fills.rs`: the accounts' fills and the check that they are the bot's own
- `src/bot/run.rs`: `Runner`: the fleet's decisions turned into requests, in one task
- `src/bot/orders.rs`: the signed market orders, nonces, and what an answer does to an account
- `src/bot/startup.rs`: unlocking the keys, the startup reads, and the stop signals
- `src/bot/stop.rs`: the stop: the wait for the log, then each account's signed cancel and close,
  and the check
- `tests/common/mod.rs`: a local stand-in node that signs its answers and frames, with injected
  faults, and pings after each frame, so a test can wait until the bot has applied it
- `tests/common/bot.rs`: the bot's config, run, the log and account the tests script, and the
  orders they check
- `tests/node_answers.rs`, `tests/stream.rs`, `tests/bot.rs`: the binary against the stand-in
- `tests/node_pacing.rs`: the pacer under the node's limit, retry-later answers, and the node's time
- `tests/many_accounts.rs`: several accounts against the stand-in: one stream connection, each
  account's nonce, orders queued under a limit, accounts that fail alone
- `tests/fleet.rs`: the fleet driven message by message: what accounts share and what is their own
- `tests/alloc.rs`: the counting-allocator test of the hot path
- `tests/hot_path.rs`: the benchmark and its budgets
- `tests/common/messages.rs`: the messages and market listings the stand-in and the tests build
- `tests/support/log.rs`: messages of the log, a synthetic hour of it, and fleets to read it
- `tests/bot_halts.rs`: the bot against the stand-in when its market is relisted, the log cannot be
  read, the node changes signer, or a stop cannot leave the account as it should
- `tests/bot_controls.rs`: the daily loss limit, a market's halt, the minimum hold, cash and the margin
  buffer
- `tests/bot_state.rs`: a split, a failed corporate action, a dividend, a position in another market,
  a read behind the log, a state file that cannot be written, and what a restart keeps
- `tests/bot_orders.rs`: the bot's orders when one is refused, goes unanswered, or is a reversal the
  engine refuses whole, and when someone else trades the account
- `tests/bot_warm_up.rs`: the warm-up from the marks history, which moves of the mark make bars,
  and where the stream starts

## Usage

```bash
cargo run -p trader -- run --config config/example.toml
```

`scripts/dev-stack.sh` runs a local node and prints its `[node]` section; `scripts/dev-trade-one.sh`
runs the bot against it end to end, `scripts/dev-strategies.sh` runs each strategy on its own account, and
`scripts/dev-trade-many.sh` runs twenty accounts in one process.

## Configuration

One TOML file, `config/example.toml` documents every key; each section refuses keys it does not
know, and relative paths are read from the file's directory:

- `[node]`: `url` (`https://host:port`), `chain_id` and `vault` (the custody deployment every
  signature names), `signer` (the enclave address that signs answers), `tls_key_sha256` (the
  node's TLS key hash to pin; without it the certificate must chain to a public root),
  `request_timeout` and `stream_timeout`
- `[node.limits]`: `per_ip` and `per_account` (`per_second` and `burst`, as the node's `[api]` sets
  them; the defaults are the engine's development config) and `connections` (HTTP connections
  kept open, besides the stream's; the node caps connections per address)
- `[sizing]` and `[risk]`: for every account that does not set its own `sizing` and `risk` tables
  (`[account.sizing]`, `[account.risk]`): sizing `by` (`fixed_notional` with `notional`, or
  `equity_fraction` with `bps`) and `max_leverage`, which must be below the market's own; risk
  `max_open_positions`, `max_total_notional`, `daily_loss`, `margin_buffer_bps` and `min_hold`
- `[state]`: `dir`, where each account's day loss, positions' opening times and splits are kept
- `[[account]]`: `key`, a keystore with its password from a file or an environment variable, or in
  a dev build a hex key file; `close_on_stop`; and one `[[account.strategy]]` per market, at most
  sixteen: `market` (a symbol), `bar` (whole minutes, 1m to 1d), `max_slippage_bps`, and the
  strategy by `kind` with its parameters (see `trader-core`)
- `[[account_dir]]`: `path` of a directory whose `.json` keystores are each an account, with one
  `password` source for all, `close_on_stop`, and `[[account_dir.strategy]]` tables as above
- `[logging]`: `level`, a tracing filter directive

`CONFIG_PATH`: the config file, when `--config` is not given. The keystore password variable is the
one the config names.

## Testing

```bash
cargo test -p trader
cargo test -p trader --test hot_path -- --nocapture   # the benchmark's measurements
./scripts/dev-trade-one.sh    # a dev scenario, about seventeen minutes
./scripts/dev-trade-many.sh   # twenty accounts, about seventeen minutes
./scripts/dev-strategies.sh   # another, about twenty minutes
```

## Authors

The monkeytrading contributors

## License

Licensed under the Apache License, Version 2.0; see `LICENSE`.
