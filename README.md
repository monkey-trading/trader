# trader

An open-source trading bot for [Monkey Trading](https://github.com/monkey-trading/engine). It runs
technical trading strategies on one or many accounts, verifies every answer that the trading server
signs before it acts on it, and serves a web console for watching and controlling the accounts.

Monkey Trading is a platform for trading contracts for difference against a house, whose rules are
the public code of its engine. The bot trades through the same signed API as any other client. Each
account is a funded key of its own, and each account trades its own markets, with one strategy in
each market. One process trades every account in its config over a single connection to the
server's public log.

## Status

The bot has not been released, and has never held real funds.

| Part | State |
| --- | --- |
| Trading | Runs end to end against a development trading server, with one account or many |
| Deposits, MONKEY claims and withdrawals | Run against a local chain with the custody vault |
| Web console | Runs against a development trading server |
| Releases | No version has been tagged yet, so the bot is built from source |

## Supported trading server

The bot is built against the engine at commit
[`ed8f721`](https://github.com/monkey-trading/engine/commit/ed8f721b0062e48ee944ab063205f3dbccb01ae6),
which runs protocol version V49, and it supports trading servers built from that commit. The
engine has no tagged releases yet.

The bot uses that commit's own code to verify the server's attestation, to sign its requests, and to
check every answer and stream message. A server whose request formats, signed answers or stream
messages differ from that commit's refuses the bot's requests or fails the bot's checks, and the
bot then stops rather than act on them.

## How it works

The trading server runs inside an enclave, a virtual machine whose memory the host machine can
neither read nor alter, and publishes a log of every instruction it processes. Before the bot makes
any other call, it verifies the enclave's attestation, a report signed by the processor that
identifies the exact code the enclave runs and the key it signs with. From then on, the bot accepts
an answer or a log message only if that key signed it. An answer signed by any other key stops the
bot.

The bot builds bars of each market's reference price from the public log, and gives each completed
bar to the strategies that trade the market. Each strategy computes a target position: long, short
or flat. The account's controls then decide whether the target becomes an order, and how large:

- position sizing, as a fixed notional value or a share of equity, held to a leverage cap;
- risk limits on the number of open positions and their total notional value;
- a daily loss limit, which closes the account's positions and stops the account until the next
  UTC day;
- a margin buffer above the engine's requirement, and a minimum holding time;
- each market's trading hours.

An order is a signed market order, sent at the last reference price with the strategy's maximum
slippage. The bot paces its requests under the server's rate limits, so that a burst of orders
waits its turn rather than failing.

The bot ships six strategies: simple and exponential moving-average crossovers, a MACD signal-line
crossover, RSI reversion, Bollinger band reversion and a Donchian channel breakout.
[`crates/trader-core`](crates/trader-core/README.md#strategies) describes what each one computes
and when it is long, short or flat. [How the bot works](docs/how-it-works.md) describes the bot in
full, including how it stops, when it halts, and how it funds accounts.

## Getting it

You need Rust 1.99.1, which `rustup` installs from [`rust-toolchain.toml`](rust-toolchain.toml)
on first use. The web console needs Node.js 24, the version in [`console/.nvmrc`](console/.nvmrc),
and npm. The development server needs `git`, `python3`, `curl` and `jq`, and its local chain also
needs Foundry's `anvil`, `cast` and `forge`.

```shell
git clone https://github.com/monkey-trading/trader.git
cd trader
(cd console && npm ci && npm run build)
cargo build --workspace
cargo run -p trader -- --version
```

The last command prints `trader 0.1.0`. The bot embeds the web console's files from
`console/dist` when it is built, so build the console first. A bot built without them serves a page
that says so.

A build for real funds uses the `production` feature, as the published binaries and image do. A
production build refuses at startup a config that suits only a development server: a simulated
attestation, an extra attestation root, a pinned TLS key, a plain hexadecimal key file, or a chain
endpoint over plain HTTP.

```shell
cargo build --release --locked -p trader --features production
docker buildx build --platform linux/amd64 -t trader .
```

The image is one static binary, with the console embedded, on an empty base image, for linux/amd64
and linux/arm64, with every input pinned. `--build-arg VARIANT=dev` builds the image without the
`production` feature.

When a version is tagged, its GitHub release carries static binaries for Linux,
`trader-linux-amd64` and `trader-linux-arm64`, with a `SHA256SUMS` file, and the image is published
as `ghcr.io/monkey-trading/trader:<tag>` for both platforms. All of them are production builds.

## What you get

| Command | Purpose |
| --- | --- |
| `trader run` | Trades the configured accounts until stopped, and serves the console. Example: `trader run --config /etc/trader/trader.toml` |
| `trader addresses` | Prints the address of every account in the config, for funding and declaring them. Example: `trader addresses --config trader.toml` |
| `trader accounts` | Shows each account's status, records its eligibility declaration, deposits USDC, claims MONKEY and withdraws. Example: `trader accounts --config trader.toml status` |

| Part | Purpose |
| --- | --- |
| [`crates/trader-core`](crates/trader-core/README.md) | The strategies, indicators, sizing and risk limits, with no input or output of their own |
| [`crates/trader`](crates/trader/README.md) | The `trader` binary |
| [`console`](docs/console.md) | The web console, a Vite, React and TypeScript app that the binary embeds |
| [`config`](config/README.md) | The documented example configuration |
| `Dockerfile` | The reproducible image |

## Running it

### Against a development trading server

To try the bot on your own machine, start a development trading server:

```shell
./scripts/dev-stack.sh
```

The script fetches the pinned engine commit into a cache outside the repository, builds its server
once, and runs it with simulated custody and price feeds on free local ports. It prints the
`[node]` section of a bot config, which accepts the server's simulated attestation, so only a
development build of the bot runs with it. The server runs until you interrupt the script.

In a second terminal, write a config `bot.toml` from [`config/example.toml`](config/example.toml),
with the printed `[node]` section and a key of your own. A development build accepts a file of 64
hexadecimal characters as a key: `key = { type = "hex_file", path = "account.hex" }`. Then fund the
account through the development server's deposit route, record its eligibility declaration, and
start the bot:

```shell
curl -sk <url>/v1/dev/deposit -H 'content-type: application/json' \
  -d '{"account":"<address>","amount":"10000"}'
cargo run -p trader -- accounts --config bot.toml declare --class professional --confirm <address>
cargo run -p trader -- run --config bot.toml
```

`<url>` is the `url` that the script printed, and `<address>` is the account's address, which
`trader addresses --config bot.toml` prints. The server holds the deposit until the declaration is
recorded, and credits it a few seconds later. A bot started before the credit exits with
`no account can start`, and can be started again once the account is credited. The bot logs
`started accounts=1 failed=0`, warms up its bars from the server's history, and then sends an order
each time its target changes. Press Ctrl-C to stop it, which cancels the account's working orders
and closes its position.

`./scripts/dev-trade-one.sh` runs this whole path for fifteen minutes and checks the account's
executions against the bot's log, and `./scripts/dev-trade-many.sh` does the same for twenty
accounts in one process.

### On a server for real funds

The bot runs as its own user, `trader` below. It reads its config from `/etc/trader`, and keeps
each account's state, such as the day's loss, in `/var/lib/trader`:

```shell
sudo useradd --system --no-create-home trader
sudo install -d -m 700 -o trader -g trader /etc/trader /var/lib/trader
```

Put these files in `/etc/trader`:

- `trader.toml`, written from [`config/example.toml`](config/example.toml). It names the
  server's `url`, `chain_id` and `vault`, and sets `[node.limits]` to the server's own limits. It
  sets `[state] dir = "/var/lib/trader"` and, with a console,
  `[console] data_dir = "/var/lib/trader/console"`, because the bot writes only under
  `/var/lib/trader`. [Configuration](docs/configuration.md) documents every key.
- The release manifest of the engine release that the server runs, as that release publishes it,
  named in `[node.attestation] releases`. The bot verifies the server's attestation against it
  before anything else, and trusts no other signer.
- The account keys, as encrypted keystores, the files that wallets export, and the files that
  hold their passwords.

With one key, `trader.toml` names it in an `[[account]]` table, with the markets it trades:

```toml
[[account]]
key = { type = "keystore", path = "keys/account.json", password = { file = "keys/account.password" } }
close_on_stop = true

[[account.strategy]]
market = "BTC-USD"
bar = "5m"
max_slippage_bps = 25
kind = "sma_crossover"
fast = 5
slow = 20
```

With many keys, add an `[[account]]` table for each, or put the keystores in one directory with one
password, and name the directory in an `[[account_dir]]` table. Every `.json` keystore in it is an
account, trading on its own key, nonce and positions over the one stream connection:

```toml
[[account_dir]]
path = "keys"
password = { file = "keys.password" }
close_on_stop = true

[[account_dir.strategy]]
market = "ETH-USD"
bar = "15m"
max_slippage_bps = 25
kind = "ema_crossover"
fast = 5
slow = 20
```

`trader addresses --config /etc/trader/trader.toml` prints each account's address. Declare and fund
each account before the bot trades it, with `trader accounts` as
[Funding an account](#funding-an-account) shows, or from a wallet.

#### Docker

```shell
docker run --name trader --network host --user "$(id -u trader):$(id -g trader)" \
  --stop-timeout 300 --volume /etc/trader:/config:ro --volume /var/lib/trader:/var/lib/trader \
  ghcr.io/monkey-trading/trader:<tag>
```

`<tag>` is a released version. The image runs `trader run --config /config/trader.toml`. Host
networking keeps the metrics listener on the host's loopback address, and serves the console, with
a `[console]` section, on the host's port 8443. Arguments after the image name replace that
command, so `trader accounts` runs in the image too:

```shell
docker run --rm -it --network host --user "$(id -u trader):$(id -g trader)" \
  --volume /etc/trader:/config:ro --volume /var/lib/trader:/var/lib/trader \
  ghcr.io/monkey-trading/trader:<tag> accounts --config /config/trader.toml status
```

#### systemd

Check the binary against `SHA256SUMS`, and install it:

```shell
sha256sum --check --ignore-missing SHA256SUMS
sudo install -m 755 trader-linux-amd64 /usr/local/bin/trader
```

Then run it with a unit such as `/etc/systemd/system/trader.service`, and
`sudo systemctl enable --now trader`:

```ini
[Unit]
Description=trader, a Monkey Trading bot
Wants=network-online.target
After=network-online.target

[Service]
User=trader
StateDirectory=trader
ExecStart=/usr/local/bin/trader run --config /etc/trader/trader.toml
TimeoutStopSec=300
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true

[Install]
WantedBy=multi-user.target
```

### Stopping the bot

On SIGINT or SIGTERM, the bot cancels each account's working orders and, for an account with
`close_on_stop`, closes its positions. It first waits for the log to pass every order that could
still enter it, which takes at most 35 seconds. It then sends about six requests for each account,
for the reads, the nonce, the cancellation and the close, paced under the server's `per_ip` limit,
which is 50 requests a second by default.

Give the bot 60 seconds to stop, plus six requests for each account at the `per_ip` rate. For 1,000
accounts at the default rate, that is 300 seconds, which the Docker and systemd examples above
allow. A bot with one account stops at once, whatever the timeout.

**A kill in the middle of the stop leaves the accounts that the stop had not reached with their
working orders and positions, and nothing reports which accounts those are. Before the next start,
read them with `trader accounts status`.**

The bot exits with a non-zero status when it halts, for example when an answer fails its signature
check, and leaves the accounts as they are for you to inspect. A supervisor that restarts the bot
on failure resumes trading without that inspection.

### Funding an account

With a `[chain]` section that names the vault's chain, Arbitrum in production, `trader accounts`
acts for each configured account, or for each one that `--account` names:

```shell
trader accounts --config bot.toml status
trader accounts --config bot.toml declare --class professional
trader accounts --config bot.toml deposit --amount 1000
trader accounts --config bot.toml claim
trader accounts --config bot.toml withdraw --amount 250
```

- `status` shows each account's declaration, wallet, balance, unclaimed MONKEY and withdrawals.
- `declare` shows the eligibility declaration, and asks you to confirm it for each account before
  it records it.
- `deposit` deposits USDC from the account's own wallet, and waits for the server's credit.
- `claim` claims the account's MONKEY to the account.
- `withdraw` withdraws USDC to the account's own wallet.

An optional `[funding]` key pays the gas for every account's claims and withdrawals, and
`deposit --from-funding` deposits from its wallet through the vault's `depositFor`. The funding key
never receives a payout. Without it, each account needs ether for its own gas. With a `[claims]`
section, `trader run` claims each account's MONKEY once the unclaimed amount reaches a threshold.

`trader run` unlocks the funding key when it starts, if the config has `[claims]`, or has both
`[console]` and `[chain]`. It exits if the key does not unlock, so the key's password must be in
the bot's own environment or file, not only where you run `trader accounts`.

`./scripts/dev-stack.sh --chain` adds a local Anvil chain, with the vault and a mock USDC deployed
from the pinned engine commit's contracts, and the house funded with 5,000,000 mock USDC. It prints
the `[chain]` section as well. `./scripts/dev-lifecycle.sh` takes three accounts through their
declarations, deposits, trading, MONKEY claims through a funding key, and a withdrawal on that
chain.

### The console

With a `[console]` section, the bot serves the web console over HTTPS on every interface, at port
8443 by default. On its first run, the bot generates a self-signed certificate and a login
password, and prints the password once. Every start prints the certificate's SHA-256 fingerprint.
Open `https://<host>:8443`, check that the certificate your browser shows has that fingerprint, and
sign in.

The console shows every account and the totals across them. It starts and stops accounts and
their strategies, changes strategy parameters within the bounds that the config sets, and closes
every position of an account. With a `[chain]` section, it also declares, deposits, claims and
withdraws for the accounts. Nothing it can do moves funds away from an account's own key, and no
route returns a key. [The console](docs/console.md) describes its pages, controls, security and
API.

## Testing

On a pull request, CI runs the checks that the changed paths affect, and a merge group runs all of
them. To run the main checks locally:

```shell
cargo fmt --all -- --check
./scripts/check-file-loc.sh
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo deny check
(cd console && npm run lint && npm run typecheck && npm test && npm run build)
```

- `./scripts/check-file-loc.sh` keeps each Rust file within 500 lines.
- `cargo deny check` checks advisories, licences, banned crates and sources. It needs `cargo-deny`.
- The scenario scripts in `scripts/` run the bot against a development server for ten to twenty
  minutes each. [`crates/trader`](crates/trader/README.md#testing) lists them.

## Contributing

Pull requests against `main` are welcome. For a large change, please open an issue first.

- Run the checks above before you open a pull request.
- `trader-core` performs no input or output. Time and prices reach it as arguments.
- The engine's crates are pinned to one commit in `Cargo.toml`. A change of that commit moves all
  three crates together, and updates [Supported trading server](#supported-trading-server).
- Commit and pull request titles take the form `[<scopes>] Subject`, for example
  `[trader,config] Pace declarations under the server's budget for new accounts`.

A pull request merges once CI passes and a maintainer has approved it.

## Security

Please report vulnerabilities privately, as [SECURITY.md](SECURITY.md) describes, and never in a
public issue.

## License

trader is licensed under the Apache License 2.0. The licence text is in [`LICENSE`](LICENSE).
