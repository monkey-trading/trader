# trader

trader is an open-source trading bot for [monkeytrading](https://github.com/monkey-trading/engine),
the house-counterparty CFD engine. It runs simple technical strategies, such as a moving-average
crossover, across one or many funded keys, each its own account, and serves a web console on the
host's address.

> **Early development.** The bot trades one or many accounts, each in its own markets, with a simple
> moving-average crossover, takes accounts from an empty wallet to trading and back on a local chain,
> and serves the console's API behind a login, against a local development node. It has not been
> released, the console app is new, and nothing here has held real funds.

## Supported node

The bot is built against the engine at commit
[`ed8f721`](https://github.com/monkey-trading/engine/commit/ed8f721b0062e48ee944ab063205f3dbccb01ae6)
(`main` on 2026-11-04, protocol version V49), the latest when it was pinned, and supports nodes
running that commit. The engine has no tagged releases yet. The bot verifies the node's attestation
with that commit's `engine-verify`, signs its requests with its `engine-core`, and checks every
answer with its `engine-wire`, so a node whose request types, signed answers, or stream messages
differ refuses the bot's requests or fails its checks, and the bot stops rather than act on them.

## How it is built

- `crates/trader-core` is a library with no IO: indicators, strategies, sizing and risk limits. Time
  and prices arrive as arguments, so strategies are tested and benchmarked without a network.
- `crates/trader` is the binary. It does the IO: it verifies the node's attestation, reads the node's
  API and the public log over one signed stream, checks every answer and stream frame against the
  enclave signer the attestation names, unlocks the account keys, and signs orders. It records an
  account's declaration, deposits USDC into the vault, claims MONKEY and withdraws, on the custody
  chain, every payout going to the account's own address. It also serves the console over HTTPS on the
  host's address: the app's files and an API for everything the console shows and controls, behind a
  login, and Prometheus metrics on a loopback address.
- `console/` is the web console, a Vite, React and TypeScript app built to static files that the
  binary will embed.
- The engine's own crates, `engine-core`, `engine-wire` and `engine-verify`, are Cargo git
  dependencies on `https://github.com/monkey-trading/engine`, pinned by one `rev`, so the bot signs
  exactly what the node checks and verifies its attestation as the engine does. The repository has
  no git submodules.
- The `Dockerfile` builds the bot as one static binary with the console embedded, on an empty base
  image, for linux/amd64 and linux/arm64, with every input pinned. A version tag publishes the
  binaries, their SHA-256 checksums and the image.

## Building

The Rust toolchain is pinned in `rust-toolchain.toml`; `rustup` installs it on first use.

```bash
cargo build --workspace
cargo run -p trader -- --version
```

A build for real funds uses the `production` feature, as the published binaries and image do. It
refuses at startup a config that only suits a development node: a mock attestation, an extra
attestation root, a pinned TLS key, a plain hex key, or a plain-HTTP chain endpoint. The image
builds the same way:

```bash
cargo build --release --locked -p trader --features production
docker buildx build --platform linux/amd64 -t trader .   # VARIANT=dev builds without the feature
```

## Quick start

A version tag publishes the bot for Linux on x86-64 and arm64: static binaries,
`trader-linux-amd64` and `trader-linux-arm64`, with a `SHA256SUMS` file on the tag's GitHub
release, and the image `ghcr.io/monkey-trading/trader:<tag>` for both. All are production builds.
Only the maintainer's tags publish: a tag ruleset limits who may push `v*` tags, and the release
workflow checks that the tag is the crate's version on a commit of `main`. No version has been
tagged yet; until one is, build the binary or the image as [Building](#building) shows.

### The config directory

The bot runs as its own user, `trader` below, which reads its config from `/etc/trader` and keeps
each account's state, such as the day's loss, in `/var/lib/trader`:

```bash
sudo useradd --system --no-create-home trader
sudo install -d -m 700 -o trader -g trader /etc/trader /var/lib/trader
```

Put in `/etc/trader`:

- `trader.toml`, from [`config/example.toml`](config/example.toml), with the node's `url`,
  `chain_id` and `vault`, `[node.limits]` set to the node's, `[state] dir = "/var/lib/trader"`, and,
  with a console, `[console] data_dir = "/var/lib/trader/console"`, since the bot writes only there.
  [`config/README.md`](config/README.md) documents every key.
- The release manifest of the engine release the node runs, as that release publishes it, named in
  `[node.attestation] releases`. The bot verifies the node's attestation against it before
  anything else, and trusts no other signer.
- The keys: encrypted keystores, the files wallets export, and files holding their passwords.

With one key, `trader.toml` names it in an `[[account]]` table with the markets it trades:

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

With many, add an `[[account]]` table for each, or put the keystores in one directory with one
password and name it in an `[[account_dir]]` table: every `.json` keystore in it is an account, each
trading on its own key, nonce and positions over the one stream connection.

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
each account before the bot trades it, with `trader accounts` as [Funding an account](#funding-an-account)
shows or from a wallet.

On SIGINT or SIGTERM the bot cancels each account's resting orders and, with `close_on_stop`,
closes its positions, which can take a minute or two, and longer with many accounts, so give it
that long to stop. It exits non-zero when it halts, such as when an answer fails its signature
check, and leaves the accounts as they are for you to look at; a supervisor that restarts it on
failure resumes trading without that look.

### Docker

```bash
docker run --name trader --network host --user "$(id -u trader):$(id -g trader)" --stop-timeout 120 \
  --volume /etc/trader:/config:ro --volume /var/lib/trader:/var/lib/trader \
  ghcr.io/monkey-trading/trader:<tag>
```

The image runs `trader run --config /config/trader.toml`. Host networking keeps the metrics
listener on the host's loopback address and serves the console, with a `[console]` section, on the
host's port 8443. Arguments after the image replace that command, so `trader accounts` runs in it
too:

```bash
docker run --rm -it --network host --user "$(id -u trader):$(id -g trader)" \
  --volume /etc/trader:/config:ro --volume /var/lib/trader:/var/lib/trader \
  ghcr.io/monkey-trading/trader:<tag> accounts --config /config/trader.toml status
```

### systemd

Check the binary against `SHA256SUMS` and install it:

```bash
sha256sum --check --ignore-missing SHA256SUMS
sudo install -m 755 trader-linux-amd64 /usr/local/bin/trader
```

Then run it with a unit such as `/etc/systemd/system/trader.service`, and
`sudo systemctl enable --now trader`:

```ini
[Unit]
Description=trader, a monkeytrading bot
Wants=network-online.target
After=network-online.target

[Service]
User=trader
StateDirectory=trader
ExecStart=/usr/local/bin/trader run --config /etc/trader/trader.toml
TimeoutStopSec=120
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true

[Install]
WantedBy=multi-user.target
```

## Running against a dev node

`scripts/dev-stack.sh` fetches the pinned engine commit into a cache outside the repository
(`TRADER_ENGINE_CACHE`), builds its node once, and runs it with mock custody and price feeds on
free local ports. It prints the `[node]` section of a bot config, which takes the node's mock
attestation, so only a development build runs with it. Fund and declare an account as the
script's `dev_stack_fund` and `dev_stack_declare` functions do, then run the bot on a config such as
[`config/example.toml`](config/example.toml):

```bash
./scripts/dev-stack.sh                      # keep a dev node up until interrupted
cargo run -p trader -- run --config bot.toml
```

The bot reads the market, the account and the recent marks, then follows the public log: each
completed bar goes to the strategy, and a target the position is not at sends a signed market
order. On SIGINT or SIGTERM it cancels the account's resting orders and, with
`close_on_stop = true`, closes its position; a stop that leaves either behind, or cannot confirm it
did not, exits non-zero. `./scripts/dev-trade-one.sh` and `./scripts/dev-trade-many.sh` run the whole path for
fifteen minutes and check each account's fills against the bot's log.

The account key is read from an encrypted keystore, the file wallets export; a development build
also takes a plain hex key file.

## Funding an account

With a `[chain]` section naming the vault's chain (Arbitrum), `trader accounts` acts for each
configured account, or the ones `--account` names:

```bash
trader accounts --config bot.toml status                      # declaration, wallet, balance, MONKEY, withdrawals
trader accounts --config bot.toml declare --class professional  # shows the declaration; asks before each
trader accounts --config bot.toml deposit --amount 1000       # USDC from the account's own wallet
trader accounts --config bot.toml claim                       # MONKEY to the account
trader accounts --config bot.toml withdraw --amount 250       # USDC to the account's own wallet
```

An optional `[funding]` key pays the gas of claims and withdrawals for every account, and
`deposit --from-funding` deposits from its wallet with the vault's `depositFor`; it never receives a
payout. Without it, each account needs ether for its own gas. With `[claims]`, `trader run` claims
each account's MONKEY once the unclaimed amount reaches a threshold.

`./scripts/dev-stack.sh --chain` adds a local Anvil chain, with the vault and a mock USDC deployed
from the pinned engine commit's contracts and the house funded with 5,000,000 mock USDC, and
prints the `[chain]` section too; it needs
[Foundry](https://getfoundry.sh). `./scripts/dev-lifecycle.sh` takes three accounts through
declaration, deposits, trading, MONKEY claims through a funding key, and a withdrawal on it.

## The console

With a `[console]` section in its config, the bot serves the console on every interface, port 8443,
over HTTPS. On first run it generates a self-signed certificate and a login password, keeps the
certificate and only an Argon2 hash of the password in the console's data directory, and prints the
password once; every start prints the certificate's SHA-256 fingerprint. Open
`https://<host>:8443`, check that the certificate the browser shows has that fingerprint, and log in.
The config may name a certificate and key of its own instead. The console shows every account and
their totals, starts, stops and flattens accounts and their strategies, and, with a `[chain]` section,
declares, deposits for, claims for and withdraws from the accounts; nothing it can do moves funds
away from an account's own key, and no route returns a key. `./scripts/dev-console-api.sh`
runs the bot with its console against a dev node and checks its API against the node's answers;
`./scripts/dev-console-funding.sh` funds, claims for and withdraws from two accounts through the
console on a local chain.

Building the app needs the Node.js version in `console/.nvmrc` and npm; build it before the bot, so
the bot embeds it:

```bash
cd console
npm ci
npm run build   # writes console/dist/, which the bot embeds when built after it
npm run dev     # Vite's dev server on 127.0.0.1, proxying /api to CONSOLE_BOT_URL
npm run stub    # a stand-in console API with three accounts, for the dev server or the browser test
```

The console signs in with the bot's password, lists every account with its equity, profit and loss,
margin level, positions and MONKEY, and opens each account's page to start and stop its strategies,
change their parameters within the bot's bounds, and flatten it. Its Funding page shows each
account's declaration, wallet, balance, unclaimed MONKEY and withdrawals, and declares, deposits,
claims and withdraws for the accounts selected, each action's state shown until it lands, with every
payout going to the account's own address. Figures update over the bot's WebSocket; a failed control
says so at the control.

## Checks

On a pull request, CI runs the checks the changed paths affect; a merge group runs all of them.
`ci-passed` is the one CI check a merge requires, and a merge also needs the maintainer's approval.

```bash
cargo fmt --all -- --check
./scripts/check-file-loc.sh                                   # Rust files stay within 500 lines
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo deny check                                              # advisories, licenses, bans, sources
cd console && npm run lint && npm run typecheck && npm test && npm run build
```

## Contributing

Work is tracked in GitHub issues, and pull requests against `main` are welcome. CI checks
formatting, file length, lints, tests, dependency advisories and licences, the dev scenarios against
a node built from the pinned engine commit, the console's lint, type check, tests and build, and the
agent docs' links. Run the same checks locally before opening a pull
request. The code's conventions are documented in [`agents/AGENTS.md`](agents/AGENTS.md).

## Security

Report vulnerabilities privately, as [SECURITY.md](SECURITY.md) describes.

## License

Licensed under the Apache License, Version 2.0; see [LICENSE](LICENSE).
