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
  fails or falls silent
- Startup reads of the market, the account's position and balance, and the market's marks history
  to warm the strategy up; from then on the position follows the account's fills in the log
- Signed market orders toward the strategy's target, on the account's next nonce, quoted at the
  last mark; a declined order is logged with the node's reason and retried on the next bar
- On SIGINT or SIGTERM, a cancel of the account's resting orders and, when configured, a close of
  its position
- The account key from an encrypted Web3 Secret Storage keystore, or, in a dev build, a hex file;
  held only as a signing key that zeroes its secret on drop, and never printed

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
- `src/bot/startup.rs`: the startup reads and the stop signals
- `src/bot/orders.rs`: the signed order, cancel and close requests
- `tests/common/mod.rs`: a local stand-in node that signs its answers and frames, with injected
  faults
- `tests/node_answers.rs`, `tests/stream.rs`, `tests/bot.rs`: the binary against the stand-in

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
