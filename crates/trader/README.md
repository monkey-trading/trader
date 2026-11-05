# trader

## Overview

The monkeytrading bot's binary. `trader run --config <file>` trades one or many accounts, each its
own funded key with its own markets and SMA crossover in each, against a monkeytrading node, in one
process over one stream connection. `trader addresses --config <file>` prints the accounts'
addresses, for funding and declaring them. Every answer the node signs and every frame of its
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
  follow, or someone else's fill, changes nothing: no cancel and no close, so the account is left
  as it is for its user, with a Critical alert and a non-zero exit
- Many accounts from `[[account]]` tables or directories of keystores (`[[account_dir]]`), each with
  its own strategies. One `/v1/stream` connection serves all of them: the log is read once, each
  `Prices` command moves its market's index once, and each market and bar length has one series of
  bars whose averages every strategy of that market and length shares, however many accounts
  trade on it. A bar that completes is decided for each account that reads it
- Startup reads of the markets and each account's positions and balance; from then on each
  position follows its account's fills in the log, and the balance is read again after fills. A
  fill from an order this process did not sign, once orders signed before it started have
  expired, means someone else trades the account, and stops that account (`account_shared`),
  not the others. A fill is the bot's when the command that made it carries one of the signatures
  it made for that account. An account that cannot start (a key that will not unlock, an account
  the node never credited, an empty balance, a market it cannot follow) or that the node refuses is
  reported (`account_failed`) and stops alone; the others carry on, and the run exits non-zero
- Requests share a small pool of kept-open HTTP/1.1 connections and are paced under the node's
  limits, set in `[node.limits]` from its `api.per_ip` and `api.per_account`: when a burst of
  orders exceeds them they queue in the order their bars completed rather than fail, and a
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
- Signed market orders toward each strategy's target, on the account's next nonce, quoted at the
  last mark; a declined order is logged with the node's reason and retried on the next bar, and a
  reversal the engine refuses whole is retried as a reduce-only close, then an opening order. No
  order goes out for a bar more than half a minute behind the node's time, by its frame or by the
  node's time when it is sent, or before the log has passed the expiry of the last order for that
  account and market, or of any order an earlier process may have signed, so an order's fill is
  always in the position before the next decision. An order never exceeds the engine's input bound
- On SIGINT or SIGTERM, once the log has passed every order that could still enter it, a cancel of
  each account's resting orders and, when configured, a close of its positions at the node's current
  marks, repeated on fresh nonces while a close leaves part of a position (the node closes at most
  a billion units a request), for at most a minute; a stop that leaves either behind, or cannot
  confirm it did not, exits non-zero with an alert. A stop can take the order expiry window (30 s),
  a few calls of `request_timeout` per account, paced under the limits, and a further close's, and a
  second signal does not cut it short, so a supervisor's
  grace period should be longer, the more so with many accounts
- After warm-up, reading a log message allocates nothing, for any number of accounts
  (`tests/alloc.rs`), and `tests/hot_path.rs` replays an hour of a synthetic log through 1,000
  accounts and holds the time per message and the memory per account to budgets
- `trader accounts declare --class <professional|eligible-client>` shows the class's statement
  and records it, signed by the account, only for an account the user confirms, in the terminal or
  with `--confirm <address>`; it needs the node alone
- `trader accounts deposit --amount <USDC>` approves the vault for the amount and deposits it from
  the account's wallet (`Vault.deposit`), or with `--from-funding` from the funding key's
  (`Vault.depositFor`), then waits for the node's credit; one the node holds until the account
  declares is reported as such
- `trader accounts claim` claims each account's MONKEY with the node's proof (`Vault.claimEmissions`);
  with `[claims]`, `trader run` claims once the unclaimed amount reaches a threshold
- `trader accounts withdraw [--amount <USDC>]` requests a signed withdrawal, waits for the FIFO
  queue to approve it and for each payout's authorization, asks for a new one when it expired
  unpaid (again after a while if none comes), and pays it with `Vault.withdraw`; without an amount
  it pays what is already approved
- `trader accounts status` shows each account's declaration, wallet, balance, unclaimed MONKEY and
  withdrawals
- Every payout goes to the account's own address: the vault pays a claim or withdrawal to the
  account it names, the bot names only the account whose key it holds, it checks each payout's
  authorization names that account, and its token binding has no transfer. The optional funding key
  pays gas for claims and withdrawals and funds deposits with `depositFor`, and receives nothing
- Chain access over one or more JSON-RPC endpoints, on ring TLS, connected on first use: reads move
  past an endpoint that fails, and after a transaction of the bot's lands, past one behind its
  block; every transaction is estimated first and not sent when it would revert, is sent to every
  endpoint and followed to its receipt even when no endpoint answered, is signed again only once
  another transaction has spent its nonce, and one that reverts is reported with the contract's
  reason. No fee over the configured cap is offered, so an
  endpoint that misquotes the price cannot spend the payer's ether. An endpoint's URL is never
  printed
- Declarations keep to the node's fresh-account budget for one client (`node.limits.fresh_accounts`,
  from its `api.fresh_accounts.per_client`), and a retry-later answer from the node, as those
  budgets give, and a stale nonce are followed by the request again; an answer that never came is checked against the node's record
  before anything is asked again
- The account keys from an encrypted Web3 Secret Storage keystore, or, in a dev build, a hex file;
  held only as a signing key that zeroes its secret on drop, and never printed. A release build
  refuses the keys whose private halves this repository publishes

## Repository Structure

- `src/main.rs`: the entry point
- `src/lib.rs`: the command line and the runtime
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
- `tests/support/log.rs`: messages of the log, a synthetic hour of it, and fleets to read it
- `tests/bot_halts.rs`: the bot against the stand-in when its market is relisted, the log cannot be
  read, the node changes signer, or a stop cannot leave the account as it should
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
runs the bot against it end to end, and `scripts/dev-trade-many.sh` runs twenty accounts in one
process. `scripts/dev-stack.sh --chain` adds a local chain with the vault
and prints the `[chain]` section too; `scripts/dev-lifecycle.sh` runs the account lifecycle on it.

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
- `[[account]]`: `key`, a keystore with its password from a file or an environment variable, or in
  a dev build a hex key file; `close_on_stop`; and one `[[account.strategy]]` per market, at most
  sixteen: `market` (a symbol), `bar` (whole minutes, 1m to 1d), `fast` and `slow` (periods in
  bars), `qty` (the position size), `max_slippage_bps`
- `[[account_dir]]`: `path` of a directory whose `.json` keystores are each an account, with one
  `password` source for all, `close_on_stop`, and `[[account_dir.strategy]]` tables as above
- `[chain]` (optional): `rpc` (one or more JSON-RPC URLs), `rpc_timeout`, `receipt_timeout`,
  `settle_timeout` (how long the bot waits on the node to credit, approve or authorize),
  `poll_interval`, `max_fee_gwei` (the highest fee per gas a transaction may offer)
- `[funding]` (optional): `key`, as an `[[account]]`'s
- `[claims]` (optional, needs `[chain]`): `threshold` (MONKEY), `interval`
- `[logging]`: `level`, a tracing filter directive

`CONFIG_PATH`: the config file, when `--config` is not given. The keystore password variable is the
one the config names, and so is the funding key's.

## Testing

```bash
cargo test -p trader
cargo test -p trader --test hot_path -- --nocapture   # the benchmark's measurements
./scripts/dev-trade-one.sh    # a dev scenario, about seventeen minutes
./scripts/dev-trade-many.sh   # twenty accounts, about seventeen minutes
./scripts/dev-lifecycle.sh    # the account lifecycle on a local chain, about fifteen minutes, up to forty
```

## Authors

The monkeytrading contributors

## License

Licensed under the Apache License, Version 2.0; see `LICENSE`.
