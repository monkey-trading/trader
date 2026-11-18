# trader

The `trader` binary: it trades one or many accounts against a Monkey Trading trading server, funds
them and withdraws from them on the custody chain, and serves the web console.

## What it is for

This crate does everything that involves the outside world: the trading server's API and public
log, the account keys, the custody chain, the console's HTTPS server and the metrics listener. The
decisions themselves, from bars to target positions to orders, come from
[`trader-core`](../trader-core/README.md), which performs no input or output.

The crate guarantees two properties. It acts only on answers and log messages signed by the
enclave key that the server's verified attestation names, never on a key taken from the config.
And every payout it requests goes to the account's own address: no command and no console route
can send funds anywhere else, and no route returns a key.

[How the bot works](../../docs/how-it-works.md) describes its behaviour in full, and
[The console](../../docs/console.md) describes the console.

## Using it

```shell
cargo run -p trader -- run --config <config>
cargo run -p trader -- addresses --config <config>
cargo run -p trader -- accounts --config <config> status
cargo run -p trader -- accounts --config <config> declare --class professional
cargo run -p trader -- accounts --config <config> deposit --amount 100
cargo run -p trader -- accounts --config <config> claim
cargo run -p trader -- accounts --config <config> --account <address> withdraw --amount 50
```

`<config>` is the path of your config file, which the root [README](../../README.md#running-it)
shows how to write, and `<address>` is one of its accounts.

| Command | What it does |
| --- | --- |
| `run` | Trades the configured accounts until it receives SIGINT or SIGTERM, then cancels their working orders and, where configured, closes their positions |
| `addresses` | Prints the address of every account in the config |
| `accounts status` | Shows each account's declaration, wallet, balance, unclaimed MONKEY and withdrawals |
| `accounts declare --class <professional\|eligible-client>` | Shows the eligibility declaration, and records it for each account you confirm, in the terminal or with `--confirm <address>` |
| `accounts deposit --amount <USDC>` | Deposits USDC from each account's wallet, or with `--from-funding` from the funding key's, and waits for the credit |
| `accounts claim` | Claims each account's MONKEY to the account |
| `accounts withdraw [--amount <USDC>]` | Withdraws USDC to each account's own wallet, and pays withdrawals already approved |

`--account <address>` limits an `accounts` command to one account, and can be repeated. Every
command reads `--config`, or the file that `CONFIG_PATH` names. The `accounts` commands other than
`declare` need a `[chain]` section.

## Layout

| Module | Responsibility |
| --- | --- |
| `config` | The config file's sections, their checks, and what only a development build accepts |
| `keys` | Unlocking a keystore or a development key file, and signing |
| `node` | The signed and verified calls to the trading server, paced under its limits over a pool of connections, and its attestation |
| `stream` | The verified, gap-free public log, which resumes after a dropped connection |
| `bot` | Startup, applying each log message to every account, turning targets into orders, the daily loss limit, the stop and the halt |
| `accounts` | The declaration, deposit, claim and withdrawal actions, and the `trader accounts` command |
| `chain` | The JSON-RPC endpoints, the vault's and token's bindings, and transactions followed to their receipts |
| `console` | The console's HTTPS server, login and sessions, the API, the live feed, the funding jobs and the embedded app |
| `metrics` | The Prometheus registry and its loopback listener |
| `logging` | The log, with every RPC endpoint's URL and key removed from each line |
| `abi/` | The parts of the vault's and the ERC-20 token's ABIs that the bot uses, from the pinned engine commit |
| `build.rs` | Embeds the console app from `console/dist`, when it has been built |

Each module's own documentation describes its files.

## Configuration

The crate reads one TOML file. [Configuration](../../docs/configuration.md) documents every
section and key, and [`config/example.toml`](../../config/example.toml) is a complete example.

- `CONFIG_PATH` names the config file when `--config` is not given.
- The password variables of the account keystores and of the funding key are the ones that the
  config names.

The `production` feature refuses at startup a simulated attestation, an extra attestation root, a
pinned TLS key, a hexadecimal key file and a chain endpoint over plain HTTP.

## Testing

```shell
cargo test -p trader
cargo test -p trader --test hot_path -- --nocapture
cargo test -p trader --locked --features production --lib production_build
```

- The first command runs the unit tests, and the integration tests against local stand-ins for the
  trading server and the chain. The stand-ins sign their answers and stream messages, and can
  inject faults.
- The first command also runs `alloc`, which holds the memory allocations per log message to
  their bound, and `hot_path`, which replays an hour of a synthetic log through 1,000 accounts and
  holds the time per message and the memory per account to budgets. The second command prints
  `hot_path`'s measurements. Run it on a quiet machine.
- `production_build` checks what a production build refuses.

The scenario scripts run the bot against a development trading server. Each needs what
`scripts/dev-stack.sh` needs, and `TRADER_BIN` runs a bot that is already built:

| Script | What it runs | Time |
| --- | --- | --- |
| `scripts/dev-trade-one.sh` | One account trading BTC-USD, checked against its history | About 17 minutes |
| `scripts/dev-trade-many.sh` | Twenty accounts in one process, on one stream connection | About 17 minutes |
| `scripts/dev-strategies.sh` | Each strategy on its own account, and a tiny daily loss limit | About 20 minutes |
| `scripts/dev-lifecycle.sh` | Three accounts declared, funded, trading, claiming and withdrawing on a local chain | About 15 minutes, up to 40 |
| `scripts/dev-console-api.sh` | Two accounts with the console, its login and controls checked against the server | About 10 minutes |
| `scripts/dev-attestation.sh` | A production build refusing a development config, and a test-signed attestation | About 2 minutes |

Each script ends with a line that begins `PASS:`.

## License

Apache License 2.0; see [`LICENSE`](../../LICENSE).
