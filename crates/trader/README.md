# trader

## Overview

The monkeytrading bot's binary. It will run `trader-core`'s strategies against a node, sign for its
funded keys, and serve the web console; for now it parses its command line and prints its version.

## Features

- `--version` and `--help`

## Repository Structure

- `src/main.rs`: the entry point
- `src/lib.rs`: the command line and its tests

## Usage

```bash
cargo run -p trader -- --version
```

## Testing

```bash
cargo test -p trader
```

## Authors

The monkeytrading contributors

## License

Licensed under the Apache License, Version 2.0; see `LICENSE`.
