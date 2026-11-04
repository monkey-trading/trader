# trader-core

## Overview

The monkeytrading bot's decision logic: indicators, strategies, sizing and risk limits. It does no
IO, so it is tested and benchmarked without a network.

## Features

- No clocks, randomness, files, environment, or network: time and prices arrive as arguments

## Repository Structure

- `src/lib.rs`: the crate's documentation; the logic arrives with the first strategy

## Usage

A library crate, to be used by `trader`.

## Testing

```bash
cargo test -p trader-core
```

## Authors

The monkeytrading contributors

## License

Licensed under the Apache License, Version 2.0; see `LICENSE`.
