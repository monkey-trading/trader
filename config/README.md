# Configuration

`trader run --config <file>` and `trader accounts --config <file>` (or `CONFIG_PATH`) read one
TOML file, check it once at startup, and refuse to start on any error. Every section refuses a key
it does not know, so a misspelt key fails the load instead of leaving a default in place. Relative
paths are read from the config file's directory. [`example.toml`](example.toml) is a complete config
with a comment on each key.

A production build, built with the `production` cargo feature as the published binaries and image
are, refuses at load every value marked **dev build only** below.

## `[node]`

The node the bot trades against.

| Key | Required | Meaning |
| --- | --- | --- |
| `url` | yes | The node's API, `https://host:port`, with no path. |
| `chain_id` | yes | The custody chain's id, which every signature names: the node's `[custody] chain_id`. |
| `vault` | yes | The custody vault's address, which every signature names: the node's `[custody] vault`. |
| `request_timeout` | no, `"10s"` | How long one call to the node may take, connection included. Above zero. |
| `stream_timeout` | no, `"30s"` | The longest silence on the log stream before the bot reconnects. The node sends a heartbeat every few seconds. Above zero. |
| `tls_key_sha256` | no | **Dev build only.** The SHA-256 of the node's TLS key (its DER SubjectPublicKeyInfo), 64 hex characters with or without `0x`, for a node whose certificate no public CA signs, as a dev node's. Without it the node's certificate must chain to a public root. |
| `attestation` | yes | What the node's attestation must prove; see below. |

### `[node.attestation]`

Before any other call, the bot reads the node's attestation (`GET /v1/attestation`) and checks it.
The enclave address it names is the signer every answer and stream frame must carry, and the TLS
key it names is the only one the bot's connections to the node accept from then on. The signer is
never a config value.

`type = "sev_snp"` verifies an AMD SEV-SNP quote, as the engine's `engine verify-attestation`
does: the quote's certificate chain must end at a vendor root compiled into the bot, and its
launch measurement, guest policy, reported TCB and configuration hash must match one of the
release manifests below. The chain and revocation list come from the node
(`GET /v1/attestation/collateral`); that adds no trust, since they are checked against the
compiled-in roots.

| Key | Required | Meaning |
| --- | --- | --- |
| `releases` | yes | Paths to release manifests, as each engine release publishes one. At least one. |
| `root` | no | **Dev build only.** A PEM root certificate the quote's chain may also end at, for a test chain. |

`type = "mock"`, with no other key, is **dev build only**: it takes a dev node's mock attestation,
which proves nothing, and reads the signer and TLS key from it.

### `[node.limits]`

Optional. The node's limits on this bot, which it paces its requests under, so that a burst of
orders queues in bar order instead of failing with 429.

| Key | Required | Meaning |
| --- | --- | --- |
| `per_ip` | no, `{ per_second = 50, burst = 100 }` | The node's `api.per_ip` bucket: every request counts against it. Both above zero. |
| `per_account` | no, `{ per_second = 10, burst = 20 }` | The node's `api.per_account` bucket: an account's orders and cancels count against it. Both above zero. |
| `fresh_accounts` | no, `{ per_minute = 600, burst = 20 }` | The node's `api.fresh_accounts.per_client` bucket, which `trader accounts declare` paces declarations under: a production node allows at most 12 a minute with a burst of 6. Both above zero. |
| `connections` | no, `4` | How many HTTP connections to the node the bot keeps open, besides the one stream, from 1 to 16. The node caps connections per address (`api.max_connections_per_ip`). |

The defaults are the engine's development config; set them to the node's.

## `[state]`

| Key | Required | Meaning |
| --- | --- | --- |
| `dir` | no, `"state"` | The directory where the bot keeps each account's day (its opening equity and whether the daily loss limit stopped it), its positions' opening times and a split's halt across restarts, one file per account. |

## `[[account]]`

One table per account; every account follows the same log over the one stream connection. At
least one `[[account]]` or `[[account_dir]]`.

| Key | Required | Meaning |
| --- | --- | --- |
| `key` | yes | The account's key: `{ type = "keystore", path = "...", password = { file = "..." } }` or `password = { env = "VARIABLE" }` for an encrypted Web3 Secret Storage keystore, as wallets export it (a password file's one trailing newline is not part of the password); or, **dev build only**, `{ type = "hex_file", path = "..." }`, a file holding the key as 64 hex characters. A production build also refuses the keys this repository publishes for development. |
| `close_on_stop` | no, `false` | Whether stopping the bot closes the account's positions in its markets. A stop always cancels resting orders. |
| `sizing`, `risk` | no | Tables with the keys of `[sizing]` and `[risk]` below, for this account in place of those sections. |
| `strategy` | yes | One `[[account.strategy]]` table per market the account trades, from 1 to 16, each market once. |

### `[[account.strategy]]`

| Key | Required | Meaning |
| --- | --- | --- |
| `market` | yes | The market's symbol, such as `BTC-USD`: a USD market priced from spot quotes. A market with trading hours is traded only while it is open. |
| `bar` | yes | The bar length: whole minutes from `"1m"` to `"1d"`, aligned to the Unix epoch. |
| `max_slippage_bps` | yes | How far from the quoted mark a market order may fill, in bps, from 0 to 10,000. |
| `kind` | yes | The strategy, with its parameters in bars of `bar`; a parameter another kind takes is refused. Accounts that run the same strategy with the same parameters in a market and bar length share one run of it. |
| `bounds` | no | For each parameter the console may change, its lowest and highest value, such as `{ fast = [2, 20], slow = [10, 100] }`. Each range is low to high, names a parameter of `kind`, and holds the value the config sets. Without bounds the console cannot change the strategy's parameters. |

Each `kind`'s parameters, with their defaults:

- `sma_crossover`: `fast`, `slow` (required). Long while the fast simple moving average of closes is
  above the slow one, short while below. At least 1, `fast` below `slow`, `slow` at most 10,000.
- `ema_crossover`: `fast`, `slow` (required). The same with exponential moving averages.
- `macd`: `fast` (12), `slow` (26), `signal` (9). Long while the MACD line is above its signal line.
- `rsi_reversion`: `period` (14), `oversold` (`"30"`), `exit` (`"50"`), `overbought` (`"70"`). Long
  when the index falls to `oversold`, short when it rises to `overbought`, flat at `exit`; the levels
  satisfy 0 < `oversold` ≤ `exit` ≤ `overbought` < 100.
- `bollinger_reversion`: `period` (20, from 2 to 1,000), `width` (`"2"`, standard deviations, at
  most 10). Long on a close below the lower band, short above the upper, flat at the middle band.
- `donchian_breakout`: `entry` (20), `exit` (10, at most `entry`). Long on a close above the highest
  of the last `entry` closes, short below their lowest, flat on a close beyond the last `exit`
  closes.

Decimal values are strings, such as `width = "1.5"`.

## `[[account_dir]]`

Every `.json` keystore in a directory is an account, all with one password source and the same
strategies. The accounts trade independently: each has its own nonce, positions, orders and saved
state. A directory with no keystore is refused.

| Key | Required | Meaning |
| --- | --- | --- |
| `path` | yes | The directory. |
| `password` | yes | `{ file = "..." }` or `{ env = "VARIABLE" }`, as for a keystore above. |
| `close_on_stop` | no, `false` | As for `[[account]]`. |
| `sizing`, `risk` | no | As for `[[account]]`. |
| `strategy` | yes | `[[account_dir.strategy]]` tables, as `[[account.strategy]]`. |

## `[sizing]`

How large a new position is, for every account that does not set its own `sizing`.

| Key | Required | Meaning |
| --- | --- | --- |
| `by` | yes | `"fixed_notional"`, a position of `notional` USD, or `"equity_fraction"`, a position of `bps` of the account's equity. A position on the strategy's side keeps its size. |
| `notional` | with `fixed_notional` | The position's notional in USD: above 0, at most 1,000,000,000. |
| `bps` | with `equity_fraction` | The position's notional in bps of equity, from 1 to 100,000. |
| `max_leverage` | yes | The most leverage the account takes on, as notional over equity: above 0, at most 1,000, and below the market's own, which its first margin tier sets. |

## `[risk]`

The risk limits of every account that does not set its own `risk`. They hold for each account as a
whole, and the bot refuses an account that holds a position in a market it does not trade.

| Key | Required | Meaning |
| --- | --- | --- |
| `max_open_positions` | yes | The most positions the account holds at once, from 1 to 1,000. |
| `max_total_notional` | yes | The most the positions' notional may add up to, in USD: above 0, at most 1,000,000,000. |
| `daily_loss` | yes | At this loss in a UTC day the account is flattened and stopped until the next day: `{ usd = "..." }`, above 0, or `{ equity_bps = ... }`, from 1 to 10,000 bps of the day's opening equity. The loss is the fall in equity, so a withdrawal counts as a loss and a deposit hides one. |
| `margin_buffer_bps` | no, `10000` | No order adds to a position unless equity stays this many bps of the margin requirement above it, from 0 to 100,000. The engine closes positions when equity falls below half the requirement. |
| `min_hold` | no, `"1h"` | How long a position is held before a strategy closes it, at most `"7d"`. |

## `[chain]`

Optional. The custody chain the vault is on, Arbitrum in production, for `trader accounts`
(`status`, `deposit`, `claim` and `withdraw`) and `[claims]`. Without it the bot trades and
declares, and the chain actions fail.

| Key | Required | Meaning |
| --- | --- | --- |
| `rpc` | yes | JSON-RPC endpoints, tried in order: a read moves to the next when one fails, and every transaction is sent to each. `https://`, or, **dev build only**, `http://` on this machine, as a local dev chain's. At least one. A provider's API key in a URL is never printed: logs name an endpoint by its place in this list. |
| `rpc_timeout` | no, `"10s"` | How long one RPC request may take. Above zero. |
| `receipt_timeout` | no, `"5m"` | How long a sent transaction may take to be mined. Above zero. |
| `settle_timeout` | no, `"30m"` | How long the bot waits on the node for a deposit's credit, a withdrawal's approval and its authorization; run the command again to carry on. Above zero. |
| `poll_interval` | no, `"2s"` | How often the bot polls the chain or the node while it waits. Above zero. |
| `max_fee_gwei` | no, `"1"` | The highest fee per gas a transaction may offer, in gwei, as a decimal string; a transaction the chain's price would need higher is not sent. |

## `[funding]`

Optional. A key that pays the gas of every account's MONKEY claims and withdrawals, and that
`trader accounts deposit --from-funding` deposits USDC from into each account (`Vault.depositFor`).
It never receives a payout: the vault pays the account. Without it, each account pays its own gas.

| Key | Required | Meaning |
| --- | --- | --- |
| `key` | yes | As an account's `key`; a hex file is **dev build only**. |

## `[claims]`

Optional. Claims each account's MONKEY while `trader run` runs, once its unclaimed amount reaches
the threshold. Needs `[chain]`.

| Key | Required | Meaning |
| --- | --- | --- |
| `threshold` | yes | The unclaimed MONKEY at which the bot claims, as a decimal string with at most 18 decimals, such as `"100"`. |
| `interval` | yes | How often the bot checks, such as `"10m"`. Above zero. |

## `[console]`

Optional. The console: the bot's figures and controls in a browser, over HTTPS on the host's
address, behind a login. Without the section the bot serves no console. On its first run the bot
generates a self-signed certificate and a login password, keeps the certificate and only an Argon2id
hash of the password in `data_dir`, and prints the password once; every start prints the
certificate's SHA-256 fingerprint. Delete `data_dir`'s `login.argon2` and restart for a new password.

| Key | Required | Meaning |
| --- | --- | --- |
| `bind` | no, `"0.0.0.0:8443"` | The address and port the console listens on: every interface by default, so it is reachable at `https://<host>:8443`. |
| `data_dir` | no, `"data"` | Where the console keeps its generated certificate and key, the login's hash, and the controls it set, which a restart keeps. |
| `tls` | no | `{ cert = "...", key = "..." }`, PEM files to serve instead of the generated certificate. |
| `refresh_interval` | no, `"5s"` | How often the console reads each account from the node, at least a second. |

## `[logging]`

| Key | Required | Meaning |
| --- | --- | --- |
| `level` | no, `"info"` | A tracing filter directive, such as `info` or `trader=debug`. |

## `[metrics]`

Optional: without the section the bot serves no metrics.

| Key | Required | Meaning |
| --- | --- | --- |
| `listen` | yes | A loopback address and port, such as `"127.0.0.1:9466"`, where the bot serves Prometheus metrics at `/metrics`. Any other address is refused: the metrics name accounts and their equity. |

The bot serves, in OpenMetrics text:

- `trader_orders_placed_total{account, market}`: orders and closes the bot signed and sent to the
  node; a close of several positions counts in each market it names
- `trader_orders_declined_total{account, market}`: those the node refused in its answer, in each
  market the request named, or the engine rejected, as the log gives it whether the answer came or
  not, in the market the rejection names (a close the engine refuses whole, in its first)
- `trader_fills_total{account, market}`: the account's fills in the market, from the log
- `trader_account_equity_usd{account}`: the account's cash and its positions' profit or loss at
  their marks; `NaN` until the bot first values the account
- `trader_account_margin_level{account}`: equity over the margin requirement, `NaN` while nothing
  is required and until the bot first values the account; the engine closes positions below 0.5
- `trader_stream_lag_seconds`: this machine's clock when the bot reads a command from the log, less
  the time the node sequenced it, so it includes any difference between the two clocks

## Environment

- `CONFIG_PATH`: the config file, when `--config` is not given
- The keystore password variable the config names, if it names one
