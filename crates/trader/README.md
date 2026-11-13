# trader

## Overview

The monkeytrading bot's binary. `trader run --config <file>` trades one or many accounts, each its
own funded key with its own markets and a strategy in each, against a monkeytrading node, in one
process over one stream connection, and serves its console on the host's address. `trader
addresses --config <file>` prints the accounts' addresses, for funding and declaring them. Every answer the node signs and every frame of its
public log stream is checked against the configured enclave signer before the bot acts on it; an
answer that fails the check is an error, never data, and one signed by another key stops the bot.
`trader accounts <action> --config <file>` takes each configured account from an empty wallet to
trading and back: it records the account's declaration, deposits USDC into the vault, claims its
MONKEY, and withdraws, every payout going to the account's own address.

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
- `trader accounts declare --class <professional|eligible-client>` shows the class's statement
  and records it, signed by the account, only for an account the user confirms, in the terminal or
  with `--confirm <address>`; it needs the node alone, and no `[chain]` section
- `trader accounts deposit --amount <USDC>` approves the vault for the amount and deposits it from
  the account's wallet (`Vault.deposit`), or with `--from-funding` from the funding key's
  (`Vault.depositFor`), then waits for the node's credit; one the node holds until the account
  declares is reported as such. A deposit is sent again only once the vault holds none of the
  transactions signed for it, at a block by which another transaction took the last one's nonce:
  it never goes in twice, whatever the endpoints' receipts say
- `trader accounts claim` claims each account's MONKEY with the node's proof (`Vault.claimEmissions`);
  with `[claims]`, `trader run` claims once the unclaimed amount reaches a threshold
- `trader accounts withdraw [--amount <USDC>]` requests a signed withdrawal and pays each part the
  FIFO queue approves with `Vault.withdraw` as its authorization comes, until the queue holds
  nothing more of the request, reading again after a failed read; it asks for a new authorization
  when one expired unpaid (again after a while if none comes), waits for one the vault refuses at
  the chain's time or signer epoch to be replaced, and without an amount it pays what is already
  approved. A payout or claim another sender made first counts as done
- `trader accounts status` shows each account's declaration, wallet, balance, unclaimed MONKEY and
  withdrawals
- Every payout goes to the account's own address: the vault pays a claim or withdrawal to the
  account it names, the bot names only the account whose key it holds, it checks each payout's
  authorization names that account, and its token binding has no transfer. The optional funding key
  pays gas for claims and withdrawals and funds deposits with `depositFor`, and receives nothing
- Chain access over one or more JSON-RPC endpoints, on ring TLS, connected on first use: a read
  goes only to an endpoint that answered it is on the vault's chain, moves past one that fails or
  gives an error answer other than a revert, and after a transaction of the bot's lands, past one
  behind its block. Every transaction is estimated first and not sent when it would revert, is sent
  to every endpoint and followed to its receipt even when no endpoint answered, and is estimated
  and signed again only once another transaction from the same key has taken its nonce and every
  endpoint, poll after poll, holds no receipt for it; one that reverts is reported with the
  contract's reason. No fee over the configured cap is offered, so an endpoint that misquotes the
  price cannot spend the payer's ether. An endpoint's URL, and each part of it that may carry a
  key, is replaced in every log line, output line and error, whatever the log level names
- Declarations keep to the node's fresh-account budget for one client (`node.limits.fresh_accounts`,
  from its `api.fresh_accounts.per_client`). A stale nonce is followed by the request again on the
  current one, and a retry-later answer from the node, as those budgets give, by the request again
  within the action's deadline: a withdrawal on the same nonce, which the node takes at most once,
  so a stale nonce then is reported rather than followed. An answer that never came is checked
  against the node's record before anything is asked again
- A command fails when any account's action did, a refused declaration included; the funding key
  is unlocked only for the actions it pays for
- The account keys from an encrypted Web3 Secret Storage keystore, or, in a dev build, a hex file;
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
  feed, needs the console's own `Origin`; a request's body must arrive within ten seconds and hold
  at most 4 KiB; every answer carries a strict Content-Security-Policy and the other security
  headers. A refusal's body is `{"error": "<message>", "code": "<code>"}`: a client matches on the
  code (such as `session_required`, `foreign_origin`, `rate_limited`, `body_timeout`,
  `flatten_in_progress`, `account_halted` or `out_of_bounds`) and shows the message
- A read API over every account in the config, in the config's order, and in total: balance,
  equity, margin and the margin level, positions with their market's mark, their unrealised profit
  as the engine values it, and where the account closes out, resting orders, unrealised profit
  (equity less balance) and realised profit (over the account's whole fill history, after profit
  fees), the latest fills, and the MONKEY entitlement in token base units, each from the node's
  verified answers, read every `refresh_interval` through the same paced connections, at most at
  half the node's per-address rate so the orders always have the rest, which makes a round of
  reads over many accounts take longer; an answer signed by another key stops the bot as one it
  reads for itself does. The vault's claimed amount needs a chain read the bot does not make yet.
  Each account's status carries its state (`running`, `stopped`, `flattening` or `halted`), each
  strategy's kind, parameters (`{kind, ...}`: whole numbers as numbers, decimals as strings),
  `bounds` (each settable parameter's `[lowest, highest]`), warm-up and target, and, while the daily
  loss limit holds it stopped, `day_stop` (`until_ms` and the day's `loss`). An account that could
  not start is listed too, halted, with its reason and a `code` (`key_locked`, `account_refused`,
  `account_unread`, `no_balance`, `position_elsewhere`, `controls_unreadable` or
  `market_unsupported`), no figures and no controls, and, when its key would not unlock, a `label`
  naming its key file in place of an address; the totals count it among `accounts`, not
  `reporting`. Each account keeps an equity curve: the latest read of each minute for the last 24
  hours, in memory, so a restart starts it again. An account's read carries the whole curve; the
  list of accounts and the live feed carry only its newest point, as a `curve` of at most one, for a
  client to append to the curve it read. `GET /api/session` says whether the session is live. A live
  WebSocket feed sends what changes, once a second
- A control API that starts and stops an account or one of its strategies, sets a strategy's
  parameters within its `bounds`, and flattens an account: it stops it, waits for the log to pass
  its last order, cancels its resting orders and closes every position, and no start goes through
  until it ends. The runner acts on each request, and the controls are saved in
  `data_dir/accounts/`, so a restart keeps a stop; a stop that cannot be saved holds, and says so. A
  parameter change builds the strategy afresh and warms its series up again from the node's marks
  history, so the strategy decides nothing until it has. No route moves funds away from an
  account's own key or returns a key
- The console app's build, `console/dist`, embedded at compile time and served at `/`, with any
  page path routed to its `index.html`; a binary built without it serves a page saying so

## Repository Structure

- `build.rs`: embeds the console app's build, `console/dist`, when there is one
- `src/files.rs`: files only the bot's user can read, written whole
- `src/main.rs`: the entry point
- `src/lib.rs`: the command line and the runtime
- `src/logging.rs`: the log, with every RPC endpoint's URL and key replaced in each line written,
  and the command line's output and errors likewise
- `src/config/mod.rs`: the config file's sections and loading
- `src/config/validation.rs`: parsing of the node URL and TLS key hash, and the config's checks
- `src/config/tests.rs`: the config tests, against `config/example.toml`
- `src/config/chain.rs`: the `[chain]`, `[funding]` and `[claims]` sections and their checks
- `src/keys.rs`: `AccountKey`: unlocking a keystore or a dev hex file, and signing
- `src/keys/tests.rs`: the key tests
- `src/node/mod.rs`: `Node`, the signed and verified REST calls, paced and over pooled connections
- `src/node/pool.rs`: the pool of kept-open HTTP/1.1 connections
- `src/node/pace.rs`: the request pacer, and the node's time counted on from its answers
- `src/node/api.rs`: the routes the bot uses and their answers
- `src/node/accounts.rs`: the account lifecycle's routes: the declaration, deposits' custody
  records, the MONKEY entitlement, withdrawals, payouts and their reissue
- `src/node/tls.rs`: the pinned or publicly rooted TLS connections
- `src/chain/mod.rs`: `Chain`, its endpoints and reads
- `src/chain/send.rs`: the transactions the bot sends, followed to their receipts
- `src/chain/contracts.rs`: the vault's and the token's bindings, and revert reasons
- `abi/`: the entries of the vault's and OpenZeppelin `ERC20`'s ABIs the bot uses, generated from
  the pinned engine commit's build; `scripts/dev-stack.sh --chain` checks they still match it
- `src/accounts/mod.rs`: `Lifecycle`, what the actions share, and an account's status
- `src/accounts/declare.rs`, `src/accounts/deposit.rs`, `src/accounts/claim.rs`,
  `src/accounts/withdraw.rs`: the actions, each for one account whose key the caller holds
- `src/accounts/cli.rs`: `trader accounts`, the actions for the config's accounts
- `src/stream/mod.rs`: `Stream`, the verified, gap-free, resuming log
- `src/stream/message.rs`: the stream's messages, read as borrowed views
- `src/bot/mod.rs`: `run`: startup, the run, the stop, and the exit status
- `src/bot/apply.rs`: applying one message of the log: prices, listings, bars, and events
- `src/bot/decide.rs`: a bar's targets turned into orders by each account's controls, the daily loss
  limit, and what is kept of each account
- `src/bot/day.rs`: an account's day loss and positions' opening times kept in a file
- `src/bot/fleet.rs`: `Fleet`: the accounts, markets and series, and applying one log message
  (no IO)
- `src/bot/params.rs`: a strategy's parameters changed while the bot runs, and its series warmed up
  again
- `src/bot/link.rs`: the accounts' side of the console: their status, built from the fleet, and
  the runner's end of the console's requests
- `src/bot/link/requests.rs`: the runner acting on the console's requests, and the flatten
- `src/bot/link/saved.rs`: the controls the console set, saved per account; `link/tests.rs` tests
  them
- `src/bot/warm.rs`: the warm-up of each series from the marks history
- `src/bot/fills.rs`: the accounts' fills and the check that they are the bot's own
- `src/bot/run.rs`: `Runner`: the fleet's decisions turned into requests, in one task
- `src/bot/orders.rs`: the signed market orders, nonces, and what an answer does to an account
- `src/bot/flatten.rs`: the daily loss limit's close of an account, one slippage group at a time
- `src/bot/reads.rs`: the reads of accounts, taken in the order they were started, and the writes of their state
- `src/bot/startup.rs`: unlocking the keys, the startup reads, and the stop signals
- `src/bot/stop.rs`: the stop: the wait for the log, then each account's signed cancel and close,
  and the check; the flatten runs the same stop for one account
- `src/config/bounds.rs`: the parameters the console may set for a strategy, each with its lowest and
  highest value, and what a change must hold to; `bounds/tests.rs` tests them
- `src/console/mod.rs`: preparing the console's data directory, and the server's accept loop
- `src/console/tls.rs`: the configured or generated certificate, and its fingerprint
- `src/console/login.rs`: the generated password, its Argon2id hash, and the login rate limit;
  `login/tests.rs` tests them
- `src/console/session.rs`: sessions and their cookie
- `src/console/guard.rs`: the request span, the security headers, the limits on a body's time and
  size, and the `Origin` and session checks
- `src/console/routes.rs`: the API's routes
- `src/console/server.rs`: the state the routes share
- `src/console/live.rs`: the live WebSocket feed
- `src/console/board.rs`: the figures, account views, totals and equity curves the API serves;
  `board/tests.rs` tests them
- `src/console/refresh.rs`: the reads of each account from the node, paced, and its fill history;
  `refresh/tests.rs` tests the history's cursors and the pace
- `src/console/assets.rs`: the embedded console app
- `tests/common/mod.rs`: a local stand-in node that signs its answers and frames, with injected
  faults, and pings after each frame, so a test can wait until the bot has applied it
- `tests/common/bot.rs`: the bot's config, run, the log and account the tests script, and the
  orders they check
- `tests/common/reads.rs`: the stand-in's account reads: views, nonces, fill history and MONKEY
  entitlement
- `tests/common/console.rs`: the bot run with its console, and an HTTPS and WebSocket client for it
- `tests/console_access.rs`: the login, its rate limit, sessions, the `Origin` check, the security
  headers, that no answer carries a key or no route moves funds, and the unbuilt page
- `tests/console_logs.rs`: that no log line carries the password
- `tests/console_reads.rs`: the figures against the node's answers, marks and close-outs, the fill
  history, the equity curve, failed reads, and the live feed
- `tests/console_controls.rs`: stopping and starting an account and its strategy, parameters within
  their bounds, the daily loss stop's status, a restart, and the flatten
- `tests/console_many.rs`: the console over several accounts, one stopped while the other trades,
  and accounts that could not start
- `tests/node_answers.rs`, `tests/stream.rs`, `tests/bot.rs`: the binary against the stand-in
- `tests/node_pacing.rs`: the pacer under the node's limit, retry-later answers, and the node's time
- `tests/many_accounts.rs`: several accounts against the stand-in: one stream connection, each
  account's nonce, orders queued under a limit, accounts that fail alone
- `tests/fleet.rs`: the fleet driven message by message: what accounts share and what is their own
- `tests/fleet_waits.rs`: a target that waits for a read is decided whatever log time passes
- `tests/bot_closes.rs`: the daily loss limit's close against the stand-in: it waits while its account is
  behind the log, and a rejected request frees the markets it named
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
- `tests/common/chain.rs`: a local stand-in chain that decodes each signed transaction and applies
  the token's and the vault's calls the bot makes, with their events
- `tests/common/lifecycle.rs`: the lifecycle's config and the node's answers the tests script
- `tests/chain.rs`: the chain client's endpoints, broadcast and refusals
- `tests/lifecycle_declare.rs`, `tests/lifecycle_deposit.rs`, `tests/lifecycle_claim.rs`,
  `tests/lifecycle_withdraw.rs`: each action against the stand-in node and chain

## Usage

```bash
cargo run -p trader -- run --config config/example.toml
cargo run -p trader -- accounts --config config/example.toml status
cargo run -p trader -- accounts --config config/example.toml declare --class professional
cargo run -p trader -- accounts --config config/example.toml deposit --amount 100
cargo run -p trader -- accounts --config config/example.toml claim
cargo run -p trader -- accounts --config config/example.toml --account 0x… withdraw --amount 50
```

`scripts/dev-stack.sh` runs a local node and prints its `[node]` section; `scripts/dev-trade-one.sh`
runs the bot against it end to end, `scripts/dev-strategies.sh` runs each strategy on its own account,
`scripts/dev-trade-many.sh` runs twenty accounts in one process, and `scripts/dev-console-api.sh` runs
two accounts with the console. `scripts/dev-stack.sh --chain` adds a local chain with the vault and
prints the `[chain]` section too; `scripts/dev-lifecycle.sh` runs the account lifecycle on it.

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
- `[node.limits]`: `per_ip` and `per_account` (`per_second` and `burst`, as the node's `[api]` sets
  them), `fresh_accounts` (`per_minute` and `burst`, as its `api.fresh_accounts.per_client`, at
  most 12 a minute with a burst of 6 on a production node), all defaulting to the engine's
  development config, and `connections` (HTTP connections
  kept open, besides the stream's; the node caps connections per address)
- `[sizing]` and `[risk]`: for every account that does not set its own `sizing` and `risk` tables
  (`[account.sizing]`, `[account.risk]`): sizing `by` (`fixed_notional` with `notional`, or
  `equity_fraction` with `bps`) and `max_leverage`, which must be below the market's own; risk
  `max_open_positions`, `max_total_notional`, `daily_loss`, `margin_buffer_bps` and `min_hold`
- `[state]`: `dir`, where each account's day loss, positions' opening times and splits are kept
- `[[account]]`: `key`, a keystore with its password from a file or an environment variable, or in
  a dev build a hex key file; `close_on_stop`; and one `[[account.strategy]]` per market, at most
  sixteen: `market` (a symbol), `bar` (whole minutes, 1m to 1d), `max_slippage_bps`, and the
  strategy by `kind` with its parameters (see `trader-core`), and `bounds`, each parameter's
  `[lowest, highest]` that the console may set (whole numbers for a whole-number parameter, strings
  for a decimal one; a parameter left out is the config's alone)
- `[[account_dir]]`: `path` of a directory whose `.json` keystores are each an account, with one
  `password` source for all, `close_on_stop`, and `[[account_dir.strategy]]` tables as above
- `[chain]` (optional): `rpc` (one or more JSON-RPC URLs), `rpc_timeout`, `receipt_timeout`,
  `settle_timeout` (how long the bot waits on the node to credit, approve or authorize),
  `poll_interval`, `max_fee_gwei` (the highest fee per gas a transaction may offer)
- `[funding]` (optional): `key`, as an `[[account]]`'s
- `[claims]` (optional, needs `[chain]`): `threshold` (MONKEY), `interval`
- `[console]`: `bind` (default `0.0.0.0:8443`), `data_dir` (default `data`), `refresh_interval`
  (default `5s`, at least a second; each account's equity curve keeps the latest read of each
  minute for the last 24 hours, in memory, served whole only in the account's own read), and
  `tls = { cert, key }`, PEM files to serve instead of the generated certificate
- `[logging]`: `level`, a tracing filter directive

`CONFIG_PATH`: the config file, when `--config` is not given. The keystore password variable is the
one the config names, and so is the funding key's.

## Testing

```bash
cargo test -p trader
cargo test -p trader --test hot_path -- --nocapture   # the benchmark's measurements
./scripts/dev-trade-one.sh    # a dev scenario, about seventeen minutes
./scripts/dev-trade-many.sh   # twenty accounts, about seventeen minutes
./scripts/dev-strategies.sh   # another, about twenty minutes
./scripts/dev-lifecycle.sh    # the account lifecycle on a local chain, about fifteen minutes, up to forty
./scripts/dev-console-api.sh  # two accounts with the console, about ten minutes
```

## Authors

The monkeytrading contributors

## License

Licensed under the Apache License, Version 2.0; see `LICENSE`.
