# trader

trader is an open-source trading bot for [monkeytrading](https://github.com/monkey-trading/engine),
the house-counterparty CFD engine. It runs simple technical strategies, such as a moving-average
crossover, across one or many funded keys, each its own account, and serves a web console on the
host's address.

> **Early development.** The bot trades one or many accounts, each in its own markets, with a simple
> moving-average crossover, and takes accounts from an empty wallet to trading and back on a local
> chain, against a local development node. It has not been released, the console is an empty page,
> and nothing here has held real funds.

## Supported node

The bot is built against the engine at commit
[`ed8f721`](https://github.com/monkey-trading/engine/commit/ed8f721b0062e48ee944ab063205f3dbccb01ae6)
(`main` on 2026-11-04, protocol version V49), the latest when it was pinned, and supports nodes
running that commit. The engine has no tagged releases yet. The bot signs its requests with that
commit's `engine-core` and checks every answer with its `engine-wire`, so a node whose request
types, signed answers, or stream messages differ refuses the bot's requests or fails its checks,
and the bot stops rather than act on them.

## How it is built

- `crates/trader-core` is a library with no IO: indicators, strategies, sizing and risk limits. Time
  and prices arrive as arguments, so strategies are tested and benchmarked without a network.
- `crates/trader` is the binary. It does the IO: it reads the node's API and the public log over
  one signed stream, checks every answer and stream frame against the node's enclave signer,
  unlocks the account key, and signs orders. It records an account's declaration, deposits USDC
  into the vault, claims MONKEY and withdraws, on the custody chain, every payout going to the
  account's own address. The console server is to come.
- `console/` is the web console, a Vite, React and TypeScript app built to static files that the
  binary will embed.
- The engine's own crates, `engine-core` and `engine-wire`, are Cargo git dependencies on
  `https://github.com/monkey-trading/engine`, pinned by one `rev`, so the bot signs exactly what the
  node checks. The repository has no git submodules.

## Building

The Rust toolchain is pinned in `rust-toolchain.toml`; `rustup` installs it on first use.

```bash
cargo build --workspace
cargo run -p trader -- --version
```

## Running against a dev node

`scripts/dev-stack.sh` fetches the pinned engine commit into a cache outside the repository
(`TRADER_ENGINE_CACHE`), builds its node once, and runs it with mock custody and price feeds on
free local ports. It prints the `[node]` section of a bot config. Fund and declare an account as the
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
from the pinned engine commit's contracts, and prints the `[chain]` section too; it needs
[Foundry](https://getfoundry.sh). `./scripts/dev-lifecycle.sh` takes three accounts through
declaration, deposits, trading, MONKEY claims through a funding key, and a withdrawal on it.

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
formatting, file length, lints, tests, dependency advisories and licences, the dev scenarios against
a node built from the pinned engine commit, the console's lint, type check, tests and build, and the
agent docs' links. Run the same checks locally before opening a pull
request. The code's conventions are documented in [`agents/AGENTS.md`](agents/AGENTS.md).

## Security

Report vulnerabilities privately, as [SECURITY.md](SECURITY.md) describes.

## License

Licensed under the Apache License, Version 2.0; see [LICENSE](LICENSE).
