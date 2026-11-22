# Configuration

`trader run` and `trader accounts` read one TOML file, named by `--config` or, without it, by the
`CONFIG_PATH` environment variable. The bot checks the whole file when it starts, and refuses to
start on any error. Every section refuses a key it does not recognise, so a misspelt key stops the
bot rather than leaving a default in place. Relative paths are read from the config file's
directory. [`config/example.toml`](../config/example.toml) is a complete config with a comment on
each key.

A production build is built with the `production` Cargo feature, as the published binaries and
image are. It refuses every value that this page marks as **development build only**.

Amounts in US dollars and decimal values are written as strings, such as `notional = "1000"` or
`width = "1.5"`. Durations are strings such as `"10s"`, `"5m"` or `"1h"`. Rates in basis points,
abbreviated to bps, are whole numbers. One basis point is one hundredth of one percent.

## `[node]`

The trading server that the bot trades against, which the engine's code calls a node.

| Key | Required | Meaning |
| --- | --- | --- |
| `url` | Yes | The server's API, as `https://host:port`, with no path. |
| `chain_id` | Yes | The id of the custody chain, which every signature names. It is the server's `[custody] chain_id`. |
| `vault` | Yes | The address of the custody vault, which every signature names. It is the server's `[custody] vault`. |
| `request_timeout` | No, `"10s"` | How long one call to the server may take, including the connection. Above zero. |
| `stream_timeout` | No, `"30s"` | The longest silence on the log stream before the bot reconnects. The server sends a heartbeat every few seconds. Above zero. |
| `tls_key_sha256` | No | **Development build only.** The SHA-256 hash of the server's TLS key, its DER-encoded SubjectPublicKeyInfo, as 64 hexadecimal characters with or without `0x`. It is for a server whose certificate no public certificate authority has signed, such as a development server. Without it, the server's certificate must chain to a public root. |
| `attestation` | Yes | What the server's attestation must prove, as described below. |

### `[node.attestation]`

Before it makes any other call, the bot reads the server's attestation from `GET /v1/attestation`
and checks it. The attestation names the enclave's address, which is the only signer the bot
accepts on any answer or stream message. It also names the server's TLS key, which is the only key
the bot's connections accept from then on. The signer is never a config value.

`type = "sev_snp"` verifies an AMD SEV-SNP report, as the engine's `engine verify-attestation`
command does. The report's certificate chain must end at a vendor root compiled into the bot. Its
launch measurement, guest policy, reported TCB version and configuration hash must match one of the
release manifests listed below. The bot fetches the certificate chain and revocation list from the
server, at `GET /v1/attestation/collateral`. Fetching them from the server adds no trust, because
the bot checks them against the compiled-in roots.

| Key | Required | Meaning |
| --- | --- | --- |
| `releases` | Yes | The paths of one or more release manifests, as each engine release publishes them. |
| `root` | No | **Development build only.** A PEM root certificate at which the report's chain may also end, for a test chain. |

`type = "mock"`, with no other key, is **development build only**. It accepts a development
server's simulated attestation, which proves nothing, and reads the signer and TLS key from it.

### `[node.limits]`

The server's rate limits on this bot. The bot paces its requests under them, so that a burst of
orders waits in the order the bars completed, rather than failing with HTTP status 429. The
defaults are the engine's development config. Set each one to the server's own value.

| Key | Required | Meaning |
| --- | --- | --- |
| `per_ip` | No, `{ per_second = 50, burst = 100 }` | The server's `api.per_ip` limit, which every request counts against. Both values above zero. |
| `per_account` | No, `{ per_second = 10, burst = 20 }` | The rate at which the server admits an account's signed requests, which orders, cancellations and closes count against. A credited account is admitted at the rate of its volume tier, which the account's `volume.requests_per_second` shows and which is 2 a second at tier 0 on the production settings; set `per_second` to it, and `burst` to the seconds of that rate the server's `api.request_bursts` allows, which is 2 seconds on the production template. A server applies its own `api.per_account` limit only to an account that has never been credited, which the bot does not trade. Both values above zero. |
| `fresh_accounts` | No, `{ per_minute = 600, burst = 20 }` | The server's `api.fresh_accounts.per_client` limit, under which `trader accounts declare` paces declarations. A production server allows at most 12 a minute, with a burst of 6. Both values above zero. |
| `connections` | No, `4` | How many HTTP connections to the server the bot keeps open, in addition to the stream, from 1 to 16. The server limits connections per address with `api.max_connections_per_ip`. |

## `[state]`

| Key | Required | Meaning |
| --- | --- | --- |
| `dir` | No, `"state"` | The directory in which the bot keeps one file per account. The file holds the account's day, which is its opening equity and whether the daily loss limit has stopped it, the opening times of its positions, and a halt caused by a share split. These survive a restart. |

## `[[account]]`

One table per account. Every account follows the same log, over the bot's one stream connection.
A config needs at least one `[[account]]` or `[[account_dir]]` table.

| Key | Required | Meaning |
| --- | --- | --- |
| `key` | Yes | The account's key. See [Keys](#keys). |
| `close_on_stop` | No, `false` | Whether stopping the bot closes the account's positions in its markets. A stop always cancels the account's working orders. |
| `sizing`, `risk` | No | Tables with the keys of [`[sizing]`](#sizing) and [`[risk]`](#risk), which apply to this account in place of those sections. |
| `strategy` | Yes | One `[[account.strategy]]` table for each market that the account trades, from 1 to 16 markets, each market once. |

### Keys

An account's key is an encrypted keystore in the Web3 Secret Storage format, the file that wallets
export:

```toml
key = { type = "keystore", path = "keys/account.json", password = { file = "keys/account.password" } }
```

The password comes from a file, `password = { file = "..." }`, or from an environment variable,
`password = { env = "VARIABLE" }`. One trailing newline in a password file is not part of the
password.

A development build also accepts `{ type = "hex_file", path = "..." }`, a file that holds the key as
64 hexadecimal characters. A production build refuses a hex key file, and also refuses the keys
whose private keys this repository publishes for development.

### `[[account.strategy]]`

| Key | Required | Meaning |
| --- | --- | --- |
| `market` | Yes | The market's symbol, such as `BTC-USD`. The market must be quoted in US dollars, priced from spot quotes and without a weekend margin. A market with trading hours is traded only while it is open. |
| `bar` | Yes | The bar length, in whole minutes from `"1m"` to `"1d"`, aligned to the Unix epoch. |
| `max_slippage_bps` | Yes | How far from the quoted price a market order may be executed, in bps, from 0 to 10,000. |
| `kind` | Yes | The strategy, with its parameters measured in bars of `bar`. A parameter of another kind is refused. Accounts that run the same strategy, with the same parameters, in the same market and bar length share one run of it. |
| `bounds` | No | For each parameter that the console may change, its lowest and highest value, such as `{ fast = [2, 20], slow = [10, 100] }`. Each range runs from low to high, names a parameter of `kind`, and includes the value that the config sets. Without bounds, the console cannot change the strategy's parameters. |

Each kind takes the following parameters. The values in brackets are the defaults.

| `kind` | Parameters | Constraints |
| --- | --- | --- |
| `sma_crossover` | `fast`, `slow`, both required | Each at least 1, `fast` below `slow`, and `slow` at most 10,000 |
| `ema_crossover` | `fast`, `slow`, both required | As for `sma_crossover` |
| `macd` | `fast` (12), `slow` (26), `signal` (9) | |
| `rsi_reversion` | `period` (14), `oversold` (`"30"`), `exit` (`"50"`), `overbought` (`"70"`) | 0 < `oversold` ≤ `exit` ≤ `overbought` < 100 |
| `bollinger_reversion` | `period` (20), `width` (`"2"`) | `period` from 2 to 1,000, and `width`, in standard deviations, at most 10 |
| `donchian_breakout` | `entry` (20), `exit` (10) | `exit` at most `entry` |

[`crates/trader-core`](../crates/trader-core/README.md#strategies) describes what each strategy
computes and when it is long, short or flat.

## `[[account_dir]]`

Every `.json` keystore in a directory is an account. All of them share one password source and the
same strategies. The accounts trade independently, and each has its own nonce, positions, orders and
saved state. A directory with no keystore is refused.

| Key | Required | Meaning |
| --- | --- | --- |
| `path` | Yes | The directory. |
| `password` | Yes | `{ file = "..." }` or `{ env = "VARIABLE" }`, as for a keystore. |
| `close_on_stop` | No, `false` | As for `[[account]]`. |
| `sizing`, `risk` | No | As for `[[account]]`. |
| `strategy` | Yes | `[[account_dir.strategy]]` tables, as for `[[account.strategy]]`. |

## `[sizing]`

How large a new position is, for every account that does not set its own `sizing`.

| Key | Required | Meaning |
| --- | --- | --- |
| `by` | Yes | `"fixed_notional"` opens a position of `notional` US dollars. `"equity_fraction"` opens a position of `bps` of the account's equity. A position already on the strategy's side keeps its size. |
| `notional` | With `fixed_notional` | The position's notional value in US dollars, above 0 and at most 1,000,000,000. |
| `bps` | With `equity_fraction` | The position's notional value in bps of equity, from 1 to 100,000. |
| `max_leverage` | Yes | The most leverage the account takes on, as notional value over equity. It must be above 0, at most 1,000, and below the market's own maximum leverage, which the market's first margin band sets. The server also holds an account to the margin floor of its volume tier, which on the production settings limits a position to 20 times leverage at tier 0 and 50 times at tier 1, and declines an order above it with `insufficient_margin`, so set it below the account's `max_leverage` in the server's account read. |

## `[risk]`

The risk limits of every account that does not set its own `risk`. Each limit applies to the
account as a whole. The bot refuses to trade an account that holds a position in a market that the
account's config does not name.

| Key | Required | Meaning |
| --- | --- | --- |
| `max_open_positions` | Yes | The most positions the account holds at once, from 1 to 1,000. |
| `max_total_notional` | Yes | The most that the notional values of the account's positions may add up to, in US dollars, above 0 and at most 1,000,000,000. |
| `daily_loss` | Yes | The loss in one UTC day at which the account's positions are closed and the account is stopped until the next day. It is `{ usd = "..." }`, above 0, or `{ equity_bps = ... }`, from 1 to 10,000 bps of the day's opening equity. The loss is the fall in equity, so a withdrawal counts as a loss, and a deposit can hide one. |
| `margin_buffer_bps` | No, `10000` | No order adds to a position unless equity stays above the margin requirement by this many bps of the requirement, from 0 to 100,000. The engine closes positions when equity falls below half the requirement. |
| `min_hold` | No, `"1h"` | How long a position is held before a strategy may close it, at most `"7d"`. |

## `[chain]`

The custody chain on which the vault runs, which is Arbitrum in production. `trader accounts
status`, `deposit`, `claim` and `withdraw`, the `[claims]` section, and the console's funding
actions all need it. Without this section, the bot trades and records declarations, and the chain
actions fail.

When the config has both `[console]` and `[chain]`, `trader run` unlocks the `[funding]` key when
it starts, as it does with `[claims]`, and exits if the key does not unlock. A password variable
that is set only where you run `trader accounts` is therefore not enough.

| Key | Required | Meaning |
| --- | --- | --- |
| `rpc` | Yes | One or more JSON-RPC endpoints, tried in order. A read moves to the next endpoint when one fails, and every transaction is sent to each endpoint. Each is an `https://` URL or, in a **development build only**, an `http://` URL on this machine, such as a local chain's. The bot never prints a URL that may carry a provider's API key. Its logs name an endpoint by its position in this list. |
| `rpc_timeout` | No, `"10s"` | How long one RPC request may take. Above zero. |
| `receipt_timeout` | No, `"5m"` | How long a sent transaction may take to be mined. Above zero. |
| `settle_timeout` | No, `"30m"` | How long the bot waits for the server to credit a deposit, approve a withdrawal and authorise it. Run the command again to continue waiting. Above zero. |
| `poll_interval` | No, `"2s"` | How often the bot polls the chain or the server while it waits. Above zero. |
| `max_fee_gwei` | No, `"1"` | The highest fee per unit of gas that a transaction may offer, in gwei. A transaction that would need a higher fee at the chain's current price is not sent, so an endpoint that misquotes the price cannot spend the payer's ether. |

## `[funding]`

A key that pays the gas for every account's MONKEY claims and withdrawals. `trader accounts deposit
--from-funding` also deposits USDC from this key's wallet into each account, through the vault's
`depositFor`. The funding key never receives a payout, because the vault pays the account. Without
this section, each account pays its own gas.

`trader run` unlocks the funding key when it starts, and exits if the key does not unlock, when the
config has `[claims]`, or has both `[console]` and `[chain]`.

| Key | Required | Meaning |
| --- | --- | --- |
| `key` | Yes | As an account's `key`. A hex key file is **development build only**. |

## `[claims]`

Claims each account's MONKEY while `trader run` runs, once the account's unclaimed amount reaches
the threshold. It needs `[chain]`. Each pass checks 64 accounts at a time. When an account is
waiting for the vault to record a checkpoint, the pass holds that batch for up to five minutes, and
then raises the alert `monkey_claim_waiting` for an account that is still waiting.

| Key | Required | Meaning |
| --- | --- | --- |
| `threshold` | Yes | The unclaimed MONKEY at which the bot claims, as a decimal string with at most 18 decimal places, such as `"100"`. |
| `interval` | Yes | How often the bot checks, such as `"10m"`. Above zero. |

## `[console]`

The console shows the bot's figures and controls in a browser, over HTTPS on the host's address,
behind a login. Without this section, the bot serves no console.

On its first run, the bot generates a self-signed certificate and a login password. It keeps the
certificate, and only an Argon2id hash of the password, in `data_dir`, and prints the password once.
Every start prints the certificate's SHA-256 fingerprint. To get a new password, delete
`login.argon2` from `data_dir` and restart the bot.

| Key | Required | Meaning |
| --- | --- | --- |
| `bind` | No, `"0.0.0.0:8443"` | The address and port the console listens on. By default it listens on every interface, so it is reachable at `https://<host>:8443`. |
| `data_dir` | No, `"data"` | Where the console keeps its generated certificate and key, the login's hash, and the controls set through it, which a restart keeps. |
| `tls` | No | `{ cert = "...", key = "..." }`, PEM files to serve in place of the generated certificate. |
| `refresh_interval` | No, `"5s"` | How often the console reads each account from the server, at least once a second. The reads use at most half of `[node.limits] per_ip`, so with many accounts a round of reads takes longer. |

## `[logging]`

| Key | Required | Meaning |
| --- | --- | --- |
| `level` | No, `"info"` | A tracing filter directive, such as `info` or `trader=debug`. |

## `[metrics]`

Without this section, the bot serves no metrics.

| Key | Required | Meaning |
| --- | --- | --- |
| `listen` | Yes | A loopback address and port, such as `"127.0.0.1:9466"`, at which the bot serves Prometheus metrics on `/metrics`. Any other address is refused, because the metrics name accounts and their equity. |

The bot serves these metrics in the OpenMetrics text format:

| Metric | Meaning |
| --- | --- |
| `trader_orders_placed_total{account, market}` | Orders and closes that the bot signed and sent to the server. A close of several positions counts once in each market it names. |
| `trader_orders_declined_total{account, market}` | Orders that the server refused in its answer, counted in each market the request named, and orders that the engine rejected, counted in the market the rejection names. The engine's rejection is read from the log, whether or not an answer arrived. A close that the engine refuses as a whole counts in its first market. |
| `trader_fills_total{account, market}` | The account's executions in the market, from the log. |
| `trader_account_equity_usd{account}` | The account's cash plus the profit or loss of its positions at their marks. `NaN` until the bot first values the account. |
| `trader_account_margin_level{account}` | Equity over the margin requirement. `NaN` while no margin is required, and until the bot first values the account. The engine closes positions below 0.5. |
| `trader_stream_lag_seconds` | This machine's clock when the bot reads an instruction from the log, less the time at which the server processed it. The figure includes any difference between the two machines' clocks. |

## Environment

- `CONFIG_PATH` names the config file when `--config` is not given.
- The keystore password variables are the ones that the config names, for the accounts and for
  the funding key.
