# trader

## Overview

The monkeytrading bot's binary. `trader run --config <file>` trades one account in one market with
an SMA crossover against a monkeytrading node. Every answer the node signs and every frame of its
public log stream is checked against the configured enclave signer before the bot acts on it; an
answer that fails the check is an error, never data, and one signed by another key stops the bot.

## Features

- Signed REST calls to the node over TLS 1.3, pinned to the node's TLS key hash or checked against
  the public roots, each under a fresh challenge and verified with `engine-wire`
- One signed stream connection that reads the public log, verifies each frame with
  `FrameVerifier`, refuses gaps, and resumes after the last message it read when a connection
  fails or falls silent; a frame or message it cannot read stops the bot
- A halt, for an answer or frame signed by another key, a log message or listing the bot cannot
  follow, or someone else's fill, changes nothing: no cancel and no close, so the account is left
  as it is for its user, with a Critical alert and a non-zero exit
- Startup reads of the market and the account's position and balance; from then on the position
  follows the account's fills in the log, and the balance is read again after each of them. A
  fill from an order this process did not sign, once orders signed before it started have
  expired, means someone else trades the account, and stops the bot. A fill is the bot's when the
  command that made it carries one of the signatures it made
- Five seconds of log after the index the bot derives is exact, a warm-up from the node's
  log-derived marks history, at the widest resolution that tiles a bar: bars before the one the
  index became exact in come from the history, that one starts from the history's bucket in its
  window, and the moves the bot saw since follow
- Bars built only from moves of the mark, as the node's marks history records them, so bars warmed
  up from the history and bars built live agree
- Signed market orders toward the strategy's target, on the account's next nonce, quoted at the
  last mark; a declined order is logged with the node's reason and retried on the next bar, and a
  reversal the engine refuses whole is retried as a reduce-only close, then an opening order. No
  order goes out for a bar more than half a minute behind the node's time, by its frame or by the
  nonce answer, or before the log has passed the expiry of the last order, or of any order an
  earlier process may have signed, so an order's fill is always in the position before the next
  decision. An order never exceeds the engine's input bound
- On SIGINT or SIGTERM, once the log has passed every order that could still enter it, a cancel of
  the account's resting orders and, when configured, a close of its position at the node's current
  mark; a stop that leaves either behind, or cannot confirm it did not, exits non-zero with an
  alert. A stop can take the order expiry window (30 s) and a few calls of `request_timeout`, and
  a second signal does not cut it short, so a supervisor's grace period should be longer
- The account key from an encrypted Web3 Secret Storage keystore, or, in a dev build, a hex file;
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
- `src/node/mod.rs`: `Node`, the signed and verified REST calls
- `src/node/api.rs`: the routes the bot uses and their answers
- `src/node/tls.rs`: the pinned or publicly rooted TLS connections
- `src/stream/mod.rs`: `Stream`, the verified, gap-free, resuming log
- `src/stream/message.rs`: the stream's messages, read as borrowed views
- `src/bot/mod.rs`: the bot: applying the log, bars, and decisions
- `src/bot/startup.rs`: the startup reads, the warm-up from the marks history, and the stop signals
- `src/bot/fills.rs`: the account's fills, the balance read after them, and the check that they
  are the bot's own
- `src/bot/orders.rs`: the signed market order
- `src/bot/stop.rs`: the stop: the wait for the log, the signed cancel and close, and the check
- `tests/common/mod.rs`: a local stand-in node that signs its answers and frames, with injected
  faults, and pings after each frame, so a test can wait until the bot has applied it
- `tests/common/bot.rs`: the bot's config, run, the log and account the tests script, and the
  orders they check
- `tests/node_answers.rs`, `tests/stream.rs`, `tests/bot.rs`: the binary against the stand-in
- `tests/bot_halts.rs`: the bot against the stand-in when its market is relisted, the log cannot be
  read, the node changes signer, or a stop cannot leave the account as it should
- `tests/bot_orders.rs`: the bot's orders when one is refused, goes unanswered, or is a reversal the
  engine refuses whole, and when someone else trades the account
- `tests/bot_warm_up.rs`: the warm-up from the marks history, which moves of the mark make bars,
  and where the stream starts

## Usage

```bash
cargo run -p trader -- run --config config/example.toml
```

`scripts/dev-stack.sh` runs a local node and prints its `[node]` section; `scripts/dev-trade-one.sh`
runs the bot against it end to end.

## Configuration

One TOML file, `config/example.toml` documents every key; each section refuses keys it does not
know, and relative paths are read from the file's directory:

- `[node]`: `url` (`https://host:port`), `chain_id` and `vault` (the custody deployment every
  signature names), `signer` (the enclave address that signs answers), `tls_key_sha256` (the
  node's TLS key hash to pin; without it the certificate must chain to a public root),
  `request_timeout` and `stream_timeout`
- `[account]`: `key`, a keystore with its password from a file or an environment variable, or in a
  dev build a hex key file; `close_on_stop`
- `[strategy]`: `market` (a symbol), `bar` (whole minutes, 1m to 1d), `fast` and `slow` (periods in
  bars), `qty` (the position size), `max_slippage_bps`
- `[logging]`: `level`, a tracing filter directive

`CONFIG_PATH`: the config file, when `--config` is not given. The keystore password variable is the
one the config names.

## Testing

```bash
cargo test -p trader
./scripts/dev-trade-one.sh   # the dev scenario, about seventeen minutes
```

## Authors

The monkeytrading contributors

## License

Licensed under the Apache License, Version 2.0; see `LICENSE`.
