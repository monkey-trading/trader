# trader

## Overview

The monkeytrading bot's binary. `trader run --config <file>` trades one account in one market with
an SMA crossover against a monkeytrading node, and serves its console on the host's address. Every
answer the node signs and every frame of its public log stream is checked against the configured
enclave signer before the bot acts on it; an answer that fails the check is an error, never data,
and one signed by another key stops the bot.

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
  mark, repeated on fresh nonces while a close leaves part of the position (the node closes at most
  a billion units a request), for at most a minute; a stop that leaves either behind, or cannot
  confirm it did not, exits non-zero with an alert. A stop can take the order expiry window (30 s),
  a few calls of `request_timeout`, and a further close's, and
  a second signal does not cut it short, so a supervisor's grace period should be longer
- The account key from an encrypted Web3 Secret Storage keystore, or, in a dev build, a hex file;
  held only as a signing key that zeroes its secret on drop, and never printed. A release build
  refuses the keys whose private halves this repository publishes
- The console, with a `[console]` section: HTTPS on `bind` (every interface, port 8443, by
  default) with the certificate and key the config names, or a self-signed pair generated on first
  run into `data_dir`; every start prints the certificate's SHA-256 fingerprint, to compare with
  the browser's
- One login password, generated on first run and printed once, kept only as its Argon2id hash in
  `data_dir/login.argon2`; a session is a random token in an `HttpOnly`, `Secure`,
  `SameSite=Strict` cookie, held by the bot only as a hash and ended by logging out or after twelve
  hours. A client gets five attempts at once and one more every twelve seconds, and one password
  check runs at a time
- Every API route but the login needs the session; every request that changes state, and the live
  feed, needs the console's own `Origin`; every answer carries a strict Content-Security-Policy and
  the other security headers
- A read API, per account and in total: balance, equity, margin and the margin level, positions
  with their market's mark, their unrealised profit as the engine values it, and where the account
  closes out, resting orders, unrealised profit
  (equity less balance) and realised profit (over the account's whole fill history, after profit
  fees), the latest fills, and the MONKEY entitlement in token base units, each from the node's
  verified answers, read every `refresh_interval`; the vault's claimed amount needs a chain read the
  bot does not make yet. Each account's view carries its equity curve: the latest read of each
  minute for the last 24 hours, in memory, so a restart starts it again. `GET /api/session` says whether the
  session is live. A live WebSocket feed sends what changes, once a second
- A control API that starts and stops an account or its strategy, sets the strategy's periods
  within the config's bounds, and flattens an account: it stops it, waits for the log to pass its
  last order, cancels its resting orders and closes every position. The account's own task acts on
  each request, and the controls are saved in `data_dir/accounts/`, so a restart keeps a stop. No
  route moves funds away from an account's own key or returns a key
- The console app's build, `console/dist`, embedded at compile time and served at `/`, with any
  page path routed to its `index.html`; a binary built without it serves a page saying so

## Repository Structure

- `build.rs`: embeds the console app's build, `console/dist`, when there is one
- `src/main.rs`: the entry point
- `src/lib.rs`: the command line, the runtime, and the bot run beside its console
- `src/files.rs`: files only the bot's user can read, written whole
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
- `src/bot/link.rs`: the account's side of the console: the status it publishes, the requests it
  takes, and the flatten
- `src/bot/link/saved.rs`: the controls the console set, saved per account; `link/tests.rs` tests both
- `src/console/mod.rs`: preparing the console's data directory, and the server's accept loop
- `src/console/tls.rs`: the configured or generated certificate, and its fingerprint
- `src/console/login.rs`: the generated password, its Argon2id hash, and the login rate limit;
  `login/tests.rs` tests them
- `src/console/session.rs`: sessions and their cookie
- `src/console/guard.rs`: the request span, the security headers, and the `Origin` and session checks
- `src/console/routes.rs`: the API's routes
- `src/console/server.rs`: the state the routes share
- `src/console/live.rs`: the live WebSocket feed
- `src/console/board.rs`: the figures, account views, totals and equity curves the API serves;
  `board/tests.rs` tests them
- `src/console/refresh.rs`: the reads of each account from the node, and its fill history;
  `refresh/tests.rs` tests the history's cursors
- `src/console/assets.rs`: the embedded console app
- `tests/common/mod.rs`: a local stand-in node that signs its answers and frames, with injected
  faults, and pings after each frame, so a test can wait until the bot has applied it
- `tests/common/bot.rs`: the bot's config, run, the log and account the tests script, and the
  orders they check
- `tests/common/reads.rs`: the stand-in's account history and MONKEY entitlement
- `tests/common/console.rs`: the bot run with its console, and an HTTPS and WebSocket client for it
- `tests/console_access.rs`: the login, its rate limit, sessions, the `Origin` check, the security
  headers, that no answer carries a key or no route moves funds, the unbuilt page, and that no log
  line carries the password
- `tests/console_reads.rs`: the figures against the node's answers, marks and close-outs, the fill
  history, the equity curve, failed reads, and the live feed
- `tests/console_controls.rs`: stopping and starting an account and its strategy, the periods'
  bounds, a restart, and the flatten
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
runs the bot against it end to end, and `scripts/dev-console-api.sh` runs it with its console.

With a `[console]` section, the bot prints its certificate's SHA-256 fingerprint at every start and,
on the first, the login password: open `https://<host>:8443`, check the fingerprint the browser
shows for the certificate against the printed one, and log in. Delete `data_dir/login.argon2` and
restart for a new password.

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
  bars), `qty` (the position size), `max_slippage_bps`; `[strategy.bounds]`: `fast` and `slow`,
  each `[lowest, highest]`, the periods the console may set
- `[console]`: `bind` (default `0.0.0.0:8443`), `data_dir` (default `data`), `refresh_interval`
  (default `5s`, at least a second; each account's equity curve keeps the latest read of each
  minute for the last 24 hours, in memory), and `tls = { cert, key }`, PEM files to serve instead of
  the generated certificate
- `[logging]`: `level`, a tracing filter directive

`CONFIG_PATH`: the config file, when `--config` is not given. The keystore password variable is the
one the config names.

## Testing

```bash
cargo test -p trader
./scripts/dev-trade-one.sh   # the dev scenario, about seventeen minutes
./scripts/dev-console-api.sh # the console's dev scenario, about ten minutes
```

## Authors

The monkeytrading contributors

## License

Licensed under the Apache License, Version 2.0; see `LICENSE`.
