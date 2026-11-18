# config

The bot's example configuration. [`example.toml`](example.toml) is a complete config for
`trader run` and `trader accounts`, with a comment on every key, and the config tests load it so
that it always parses.

## Using it

Copy the example, then fill in the values marked `OWNER`: the trading server's address, the
custody vault, and the release manifest of the engine release that the server runs.

```shell
cp config/example.toml trader.toml
```

Each `[[account]]` names a keystore and the source of its password. Once the keystores are in
place, `trader addresses --config trader.toml` prints the address of every account the config
names. Against a development server, `scripts/dev-stack.sh` prints the `[node]` section to use in
place of the example's, and, with `--chain`, the `[chain]` section as well.

Relative paths in a config, such as a keystore's `path`, are read from the config file's directory.
Every section refuses a key it does not recognise, so a misspelt key stops the bot when it starts.

## Sections

| Section | What it sets |
| --- | --- |
| `[node]` | The trading server's address, the chain and vault that signatures name, and timeouts |
| `[node.attestation]` | The release manifests that the server's attestation must match |
| `[node.limits]` | The server's rate limits, under which the bot paces its requests |
| `[state]` | Where each account's daily loss and position opening times are kept across restarts |
| `[[account]]`, `[[account_dir]]` | Each account's key, the markets it trades and a strategy in each |
| `[sizing]`, `[risk]` | Position size, leverage and the risk limits, for every account that does not set its own |
| `[chain]`, `[funding]`, `[claims]` | The custody chain, a key that pays gas, and automatic MONKEY claims |
| `[console]` | The web console's address, data directory and certificate |
| `[logging]`, `[metrics]` | The log level and a loopback Prometheus listener |

[Configuration](../docs/configuration.md) documents every key, its default and its limits, and
the values that only a development build accepts.

## Testing

The config tests in the `trader` crate load `example.toml`:

```shell
cargo test -p trader --lib config
```

## License

Apache License 2.0, as the rest of the repository; see [`LICENSE`](../LICENSE).
