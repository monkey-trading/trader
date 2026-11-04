# trader

trader is an open-source trading bot for [monkeytrading](https://github.com/monkey-trading/engine),
the house-counterparty CFD engine. It runs simple technical strategies, such as a moving-average
crossover, across one or many funded keys, each its own account, and serves a web console on the
host's address.

> **Early development.** The repository holds the build foundation only: a Cargo workspace whose
> binary prints its version, an empty console page, and the checks that guard them. The bot does
> not trade yet, and nothing here holds real funds.

## How it is built

- `crates/trader-core` is a library with no IO: indicators, strategies, sizing and risk limits. Time
  and prices arrive as arguments, so strategies are tested and benchmarked without a network.
- `crates/trader` is the binary. It will do the IO: the node's API and stream, signing, keys, chain
  calls, and the console server.
- `console/` is the web console, a Vite, React and TypeScript app built to static files that the
  binary will embed.
- The engine's own crates will be Cargo git dependencies on
  `https://github.com/monkey-trading/engine`, pinned by one `rev`, so the bot signs exactly what the
  node checks. The repository has no git submodules.

## Building

The Rust toolchain is pinned in `rust-toolchain.toml`; `rustup` installs it on first use.

```bash
cargo build --workspace
cargo run -p trader -- --version
```

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
formatting, file length, lints, tests, dependency advisories and licences, the console's lint, type
check, tests and build, and the agent docs' links. Run the same checks locally before opening a pull
request. The code's conventions are documented in [`agents/AGENTS.md`](agents/AGENTS.md).

## Security

Report vulnerabilities privately, as [SECURITY.md](SECURITY.md) describes.

## License

Licensed under the Apache License, Version 2.0; see [LICENSE](LICENSE).
