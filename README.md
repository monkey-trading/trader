# trader

trader is an open-source trading bot for [monkeytrading](https://github.com/monkey-trading/engine),
the house-counterparty CFD engine. It runs simple technical strategies, such as a moving-average
crossover, across one or many funded keys, each its own account, and serves a web console on the
host's address.

> **Early development.** The bot trades one account in one market with a simple moving-average
> crossover, against a local development node. It has not been released, the console is an empty
> page, and nothing here has held real funds.

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
- `crates/trader` is the binary. It does the IO: it verifies the node's attestation, reads the
  node's API and the public log over one signed stream, checks every answer and stream frame
  against the enclave signer the attestation names, unlocks the account key, signs orders, and
  serves Prometheus metrics on a loopback address. Chain calls and the console server are to come.
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
attestation root, a pinned TLS key, or a plain hex key. The image builds the same way:

```bash
cargo build --release --locked -p trader --features production
docker buildx build --platform linux/amd64 -t trader .   # VARIANT=dev builds without the feature
```

## Quick start

A version tag publishes the bot for Linux on x86-64 and arm64: static binaries,
`trader-linux-amd64` and `trader-linux-arm64`, with a `SHA256SUMS` file on the tag's GitHub
release, and the image `ghcr.io/monkey-trading/trader:<tag>` for both. All are production builds.
No version has been tagged yet; until one is, build the binary or the image as
[Building](#building) shows.

### The config directory

Make a directory, `/etc/trader` below, that only the user the bot runs as can read, and put in it:

- `trader.toml`, from [`config/example.toml`](config/example.toml), with the node's `url`,
  `chain_id` and `vault`. [`config/README.md`](config/README.md) documents every key.
- The release manifest of the engine release the node runs, as that release publishes it, named in
  `[node.attestation] releases`. The bot verifies the node's attestation against it before
  anything else, and trusts no other signer.
- The account's keystore, the encrypted file a wallet exports, and a file holding its password:

```toml
[account]
key = { type = "keystore", path = "keys/account.json", password = { file = "keys/account.password" } }
close_on_stop = true
```

On SIGINT or SIGTERM the bot cancels the account's resting orders and, with `close_on_stop`,
closes its position, which can take a minute or two, so give it that long to stop. It exits
non-zero when it halts, such as when an answer fails its signature check or someone else trades the
account, and leaves the account as it is for you to look at; a supervisor that restarts it on
failure resumes trading without that look.

### Docker

```bash
docker run --name trader --network host --user "$(id -u):$(id -g)" --stop-timeout 120 \
  --volume /etc/trader:/config:ro ghcr.io/monkey-trading/trader:<tag>
```

The image runs `trader run --config /config/trader.toml`. Host networking keeps the metrics
listener on the host's loopback address.

### systemd

Check the binary against `SHA256SUMS` and install it:

```bash
sha256sum --check --ignore-missing SHA256SUMS
sudo install -m 755 trader-linux-amd64 /usr/local/bin/trader
```

Then run it as its own user with a unit such as `/etc/systemd/system/trader.service`, and
`sudo systemctl enable --now trader`:

```ini
[Unit]
Description=trader, a monkeytrading bot
Wants=network-online.target
After=network-online.target

[Service]
User=trader
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
did not, exits non-zero. `./scripts/dev-trade-one.sh` runs the whole path for
fifteen minutes and checks the account's fills against the bot's log.

The account key is read from an encrypted keystore, the file wallets export; a development build
also takes a plain hex key file.

## The console

The console needs the Node.js version in `console/.nvmrc` and npm:

```bash
cd console
npm ci
npm run build   # writes console/dist/
npm run dev     # Vite's dev server on 127.0.0.1
```

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
formatting, file length, lints, tests, dependency advisories and licences, the dev scenario against
a node built from the pinned engine commit, the console's lint, type check, tests and build, and the
agent docs' links. Run the same checks locally before opening a pull
request. The code's conventions are documented in [`agents/AGENTS.md`](agents/AGENTS.md).

## Security

Report vulnerabilities privately, as [SECURITY.md](SECURITY.md) describes.

## License

Licensed under the Apache License, Version 2.0; see [LICENSE](LICENSE).
