---
name: rust-implementation
description: Use when writing, editing, or reviewing Rust in this repo (the resulting implementation). Covers edition and toolchain, safety, error handling, input-handling doctrine, the IO-free trader-core, numeric types and money, iterators, async IO under tokio, bounded queues, hot paths and allocation, keys and secrets, process-global state, configuration, serde, logging, dependencies and the pinned engine crates, workspace layout, file structure, file line cap and module splits, crate README, and Cargo profiles.
---

Load when implementing or modifying Rust code, or reviewing a Rust change.

## Vendored reference

For detailed Rust patterns, consult the vendored upstream pack:

- Path: `../vendor/rust-skills/skills/`
- Source and pinned commit: [vendor.manifest](../vendor/vendor.manifest)
- If the pack is absent, run `agents/skills/vendor/pull.sh rust-skills` from the repo root before loading vendored Rust files.
- If a routed vendored Rust file is missing, run the pull, retry the exact read, and stop with the
  missing path if it still fails; do not continue with only this wrapper's guidance.
- Start at `rust-router/SKILL.md` when the relevant sub-skill is not obvious; load only the triggered sub-skill's `SKILL.md`, not the whole tree.
- The repo conventions below and the rules win when they conflict with vendored guidance.

## General

- Use Rust 2024 edition and the latest stable toolchain; no nightly features unless absolutely necessary and approved.
- Keep dependencies current and 2024-compatible; verify the latest version on crates.io when adding one.
- Keep code simple and idiomatic — use 2024 features freely without overcomplicating.
- Avoid `unsafe`. If unavoidable, ask first and document both the function and the reasoning.
- Minimize bugs by leveraging the type system; share common types across crates through `trader-core`, and reuse the engine crates' own types for anything the node also checks. Prefer clear domain types over stringly-typed ids, states, paths, addresses, and names (`AccountId`, `MarketId`, `Side`, `Fixed`).
- Exhaust other options (generics, associated types, refactoring) before macros. Ask before writing a macro, and document it thoroughly.

## Iterators and loops

- Prefer iterator / functional style (`map`, `filter`, `fold`); consider `itertools` before complex chains.
- Use loops only when they read better, and avoid nested loops.

## Error handling

- Executables use `anyhow`; libraries use `thiserror`. Reserve panics for the truly unrecoverable.
- Do not `unwrap()` for panic cases. Prefer proper handling; if unavoidable, use `expect()` with a clear message.
- In `trader-core`, a decision that cannot be made is a typed value (`Decision::Hold`, a typed
  reason), never an `Err` or a panic: the caller logs the reason. A node's refusal of an order
  (its reason string and status) is data the binary logs and retries on, not an error to unwind.

## Input-handling doctrine

The bot consumes answers and frames from the node, market data derived from them, chain reads,
config, keystores, and console requests. Classify every failed check on such input:

- **Detected error or inconsistent/confusing state in input data → never act on it.**
  Log, raise the alert, and return without placing an order or changing state — leave everything
  as-is for the user. Do not "recover" by guessing a price, a position, or a nonce from data just
  proven untrustworthy. A signed answer that fails `verify_response`, a frame that fails
  `FrameVerifier`, or a gap in the stream means no data, not the last data. Do not trade on a mark
  the bot could not verify.
- **Internal invariants — states our own code cannot produce without a bug → hard
  `assert!` / `expect` and crash.** Add a terse comment on why the state is
  (almost) impossible. Never `debug_assert!`.
- Before softening any assert, trace the trigger: node, chain, and console content, and
  reconnect/redelivery effects, are real; content our own code cannot produce is not.
- The same check applied to multiple inputs lives in one shared helper, not copies.
- A position, balance, or nonce that contradicts what the node's signed answers say is input, not
  an internal invariant: stop that account, raise a Critical alert, and leave the others running.
  A hard panic would crash-loop across restarts and take every account down with it.
- Alert severity for rejected input: **Warning** when a retry or the next bar re-examines the
  state; **Critical** only when nothing does — the account is stopped until the user acts.
- An alert is a log line at `error` (Critical) or `warn` (Warning) carrying an
  `alert = "<snake_case_name>"` field, emitted inside the unit of work's span, once per episode:
  values that change while one problem persists (counts, retry delays, ages) go in the message, so
  they do not split it into many lines. When the emitter sees the condition clear, it logs an
  `info` line carrying `alert_resolved = "<name>"` and the same fields. An alert about one unit of
  work (one order, say) names it in an event field.
- A stopped account beats a mistaken one: anything that would let an account keep trading on a view
  of its position or margin it cannot confirm — an unverified answer, a nonce it cannot resync —
  stops that account.

## The pure core (`trader-core`)

`trader-core` holds indicators, strategies, sizing, and risk limits. It does no IO, so it is
tested and benchmarked without a network, and a recorded log of marks replays through it to the
same decisions.

- No clocks (`SystemTime`, `Instant`), randomness, files, environment, network, threads, or
  async. Time and prices arrive as arguments.
- Collections that affect a decision are `BTreeMap` / `BTreeSet` / `Vec`; never iterate a
  `HashMap` or `HashSet` to decide anything.
- Every input a decision depends on is a parameter: the bar, the position, the equity, the limits.
  The binary computes nothing a strategy decides.
- An indicator updates in constant time per bar, holding a fixed-size window, and has a
  known-answer test against values computed independently of the code.
- A new dependency of `trader-core` must not pull in IO or a runtime.

## Numeric types and money

- The engine's `Fixed` (an `i128` scaled by 10^9, from the pinned `engine-core`) is the type for
  every amount, price, quantity, and rate that reaches an order or a limit check. `mul` / `div`
  truncate toward zero; release builds keep `overflow-checks = true`, so overflow halts rather
  than wraps.
- Never use `f32` / `f64` for money, prices, or anything that reaches an order. A float is
  acceptable only for display, or inside an indicator's statistic with a stated reason, converted
  to `Fixed` at the boundary with an explicit rounding direction and a known-answer test.
- Basis points are `i64` applied with `Fixed::mul_bps` / `Fixed::from_bps`.
- Arithmetic on any value derived from external input must not be able to overflow: check the
  input where it enters the bot (`Fixed::is_valid_input`, bounded by `Fixed::MAX_INPUT`) and
  reject it as input, before any arithmetic. The node refuses an order outside that range.
- On-chain amounts stay integer base units (`U256` or `u128`) until the chain boundary, where they
  convert to `Fixed` with the token's decimals; that conversion is checked and rejects precision
  loss.
- `Fixed` serializes as a decimal string so no JSON client loses precision; keep it that way at
  every API surface, the console's included.

## Async IO under tokio

- IO is async on tokio; `trader-core` is synchronous and pure, and the binary calls it from the
  task that owns the account. Never block a runtime worker: no `std::thread::sleep`, blocking file
  or network calls, or CPU-heavy loops on async tasks. Hash a password (Argon2) and decrypt a
  keystore on `spawn_blocking`.
- Prefer channel-based communication (`tokio::sync::mpsc` or similar) over shared state unless asked otherwise or clearly justified. One task owns each account's state and nonce; others send it a message.
- Keep cancellation, polling, and timeout behavior explicit; a wait must never hang forever. Remote, process, and network calls carry explicit timeouts / deadlines, with timeout errors propagated with enough context to identify the target and operation.
- Long-running work must respect cancellation: dropping the parent future stops child work, spawned processes are killed or joined, and background tasks have an explicit shutdown path. A stop signal cancels resting orders and optionally closes positions before exit.
- Every long-running spawned task is supervised by `main`, so the process never runs on without a loop while looking healthy: the stream reader, the console server, and the other shared tasks exiting for any reason, and a panic in any task, shut the process down. An account task that stops on rejected input or a refused key is the one exception: it ends that account alone, is reported, and leaves the others running.
- Do not hold a lock or a `span.enter()` guard across an `.await`. Prefer an owned task and messages to a `Mutex` held over IO.
- A `select!` branch must be cancellation-safe: the future it drops must leave no half-done state. State what a dropped future leaves behind where it is not obvious.
- Parallelize expensive independent work when safe, preserving ordered output; keep ordered mutation paths (one account's orders and nonces) sequential.

## Bounded queues

- Every channel, buffer, and in-memory log is bounded, with a named capacity in config or a
  constant and a stated policy when it fills: back-pressure the sender, drop the oldest with a
  counter and a log line, or fail the owner. Never `unbounded_channel` and never an unbounded
  `Vec` fed by the network.
- Queues that carry orders keep order: orders queue in bar order rather than fail under a rate
  limit, and a retry-later answer backs off by the node's stated delay.
- Per-account and per-connection state has a hard cap too (open orders, history kept in memory,
  frames buffered while the account's task is busy).
- A slow consumer must not stall the stream reader for the other accounts: its queue fills and
  its policy applies.

## Hot paths and allocation

The message path (a stream frame to a decision) runs for every account on every frame.

- Allocation per message is the exception: after warm-up, decoding a frame, updating indicators,
  and deciding allocate nothing. Reuse buffers (`clear()` and refill), pre-size collections
  (`with_capacity`), and prefer fixed-size windows (ring buffers) and slices to `Vec` growth,
  `String` building, and `clone`.
- Borrow rather than copy: parse frames into borrowed structs (`&str`, `Cow`) where serde allows,
  and pass `&` to `trader-core`.
- No formatting, `to_string`, `format!`, or boxing in a branch that runs per message unless the
  branch is rare; `tracing` macros at a disabled level cost nothing, but their arguments must not
  allocate when evaluated.
- Share, do not recompute: an indicator for a market and bar length is computed once and read by
  every account that uses it.
- Claims about speed and allocation are measured: a counting-allocator test for "allocates
  nothing" and a benchmark with a budget for time and memory, not an argument.

## Keys and secrets

- Private key bytes, keystore passwords, API keys, the console password, and session secrets live
  in types that zero on drop (`zeroize`, `secrecy`) and redact in `Debug` and `Display`. Read the
  value only at the point of use (signing, unlocking), and keep it out of `Clone`, serialization,
  logs, error messages, metrics, and panic messages.
- Account keys come from an encrypted Web3 Secret Storage keystore, unlocked with a password from
  a file or an environment variable. A plain hex key is accepted only in a dev build, behind a
  cargo feature or `cfg`, and refused at config load in a production build.
- A decrypted key exists only in the task that signs for that account. No route, log line, metric
  label, or error returns a key or its password, and the console cannot move funds away from an
  account's own address.
- Password hashes use Argon2 with a per-hash salt and are compared in constant time. Never log,
  store, or print a secret except the one-time print of a generated console login password.
- Secrets are read from files or the environment once at startup and handed to the component that
  uses them; they are not stored in the config struct as plain `String`s (see Configuration).
- No home-rolled crypto: signing, hashing, key derivation, and encryption come from vetted crates,
  and the EIP-712 hashes the bot signs come from the pinned engine crate.
- Tests use obviously fake keys. Keys whose private halves the repository publishes for
  development are refused outside a dev build.

## Process-global state

No process-global mutable state. A `static` that holds runtime state (including via `LazyLock` or
`OnceLock`) is shared by every test in the binary: a test cannot get a fresh instance or reset it,
and can only assert before/after deltas that any concurrently running test can move. It also hides
a dependency that no constructor shows and limits the process to one instance.

- **Build it in `main` and pass it in.** Metrics, counters, caches, registries, config, clients,
  keys, and channels are created at startup and handed to the components that use them, as
  `main` does with the HTTP client, the stream sender, and the account channels.
- **Metrics register into a registry the process owns**, never a library's default global registry.
- **Allowed statics:** the `#[global_allocator]`; immutable values that depend on no config,
  environment, or runtime input (constant tables, compiled regexes, a monotonic clock epoch); an
  atomic counter that only mints unique ids and is never read back or asserted on; and test-only
  infrastructure behind `#[cfg(test)]` or a test feature.

## Configuration

Typed-TOML services take one root config from a TOML file, loaded and validated once at startup
(`Config::load`). The file path is the only CLI surface (`--config` / `CONFIG_PATH`);
everything else is a config key.

- **Optionality belongs to the section, not the field.** An optional process or feature is
  `Option<Section>`, and every field inside it is plain and required — presence of the section
  means it is configured. Never make each field `Option` and then hand-write a list of missing
  config paths: serde already names the missing field.
- **`#[serde(deny_unknown_fields)]` on every section struct**, so a typo fails the boot instead of
  silently keeping a default. Neither side of a `#[serde(flatten)]` can carry the attribute: make the field that turns the feature on
  **required** there, so a typo surfaces as a missing field rather than a silent default.
- **Nothing credential-bearing is a plain `String`.** Wrap secrets and credential-bearing URLs
  (an RPC endpoint with a key in its path) in a redacting, zeroing newtype whose `Debug` and
  `Display` hide the value, and read it only at the point of use. Add the type with the first
  secret the bot needs ([Keys and secrets](#keys-and-secrets)).
- **Keep validation structural.** Ranges, lengths, non-empty tables, and parseability belong in
  the type's `validate`; per-value policy checks (placeholder detection, "looks unset") do not.
  Assume an operator-supplied config is intentional. The exception worth making is a value whose
  wrong setting fails **silently** — a zero window that can never trip, a tier table out of order:
  guard those, since every other wrong value already fails loudly.
- **Classify every new constant.** Before adding one, decide whether it is a true invariant or an
  operational preference. Keep invariants in code; put adjustable values (periods, limits, queue
  capacities, rate pacing) in `Config`, with the bounds the console may change them within.
- Durations in service config are `humantime_serde` strings (`"250ms"`, `"30s"`).
- **Tests must not mutate the process environment.** `set_var`/`remove_var` are `unsafe` in Rust
  2024 and race every concurrently running test in the binary, which `#[serial]` does not prevent.
  Take the environment as an explicit source instead, and assert on a committed example file with
  a loader that ignores the ambient environment.
- Build shared clients once — RPC provider, HTTP client, the stream connection — in `main`, and pass them to the
  components that need them rather than letting each build its own.
- A flag that overrides an environment variable has both a short and a long form
  (`-c` / `--config` over `CONFIG_PATH`).
- **`.env` is optional.** Use `dotenvy`, `.env`, and `.env.example` only when a crate exposes a
  local environment-file surface; typed TOML configuration does not require them. Document the
  active configuration and credential variables in the crate's `README.md`.
- For conditional compilation, use the attribute macro rather than the declarative one unless not viable.

## Serialization

- Maximize the serde ecosystem. Implement `Serialize` / `Deserialize` for a format instead of a bespoke interface.
- Use the engine crates' own types and signing for anything the node parses or verifies, so the
  bot sends exactly what the node checks. Config keys and the console API's JSON are `snake_case`,
  internally tagged (`#[serde(tag = "type")]`) for enums; `Fixed` stays a decimal string.

## Logging

- Use `tracing` with the `env-filter` feature on `tracing-subscriber`. The level comes from
  `[logging].level` in the service's own config, installed after the config load — never from
  `RUST_LOG`, which no service reads directly. A subcommand that loads no config uses `info`.
- `trader-core` does not log: its decisions are its output. The binary logs around them.
- One log-visible correlation span per call stack, carrying the unit of work's `id`; long-running
  loops get none, and two nested log-visible spans need a reason approved in review.
- HTTP: one middleware layer, registered outermost, owns the request span; a handler records the
  entity id (`Span::current().record("id", …)`) as soon as the entity is known.
- A spawned future does not inherit the current span: spawn `fut.instrument(span)` or `fut.instrument(Span::current())`, or its lines lose the id. Never hold a `span.enter()` guard across an `.await`.
- Ids go in their own fields: `info!(id = %order_id, "…")` for the subject, role names for other entities (`account = %address`). Never re-emit the span's id as an event field, except to key an alert on its unit of work.
- Fire alerts inside the correlation span, so they carry its fields.
- Never log secrets, keys, or raw request bodies that may carry signatures.

## Dependencies

- Reuse before reinventing: don't reimplement well-known algorithms — use a permitted crate or search crates.io first.
- Prefer logic from the workspace's own crates and follow DRY: strategy and indicator logic lives in `trader-core`, and the binary reuses it.
- May add smaller utility crates freely; ask before adding a major crate, weighing maintenance and popularity.
- A new dependency of `trader-core` must not pull in IO or a runtime.
- A new crate must pass `cargo-deny` against `deny.toml` (advisories, licenses, bans, sources), which CI runs.
- Permitted by default (non-exhaustive): `alloy`, `anyhow`, `argon2`, `axum`, `base64`, `bytes`, `clap`, `futures`, `hex`, `http`, `itertools`, `rand` (key and salt generation, and tests), `reqwest`, `rustls`, `secrecy`, `serde` (and ecosystem), `sha2`, `sha3`, `smallvec`, `thiserror`, `tokio` (and ecosystem), `toml`, `tracing`, `tracing-subscriber` (with `env-filter`), `zeroize`.

### The engine crates

The bot depends on the engine's `engine-core` (command types and EIP-712 hashes), `engine-wire`
(signed answers), and, for the production build, `engine-verify`, so it signs and verifies exactly
what the node does.

- They are Cargo git dependencies on `https://github.com/monkey-trading/engine`, each with the same
  `rev = "<full commit>"`, set once in the workspace `Cargo.toml` and referenced per package with
  `{ workspace = true }`. Never `branch`, `tag`, or an unpinned git dependency; never a path
  dependency or a copy of their code.
- That `rev` is the repository's only pin of the engine; `Cargo.lock` records the commit, and
  `scripts/dev-stack.sh` (added by the first feature that trades) builds the node from it. A bump
  moves all of them together, names the node releases it supports in the README, and says what
  changed in the engine's types or hashes.
- `cargo-deny` allows the engine repository as the one git source.
- The repository has no git submodules, ever: code from another repository comes through Cargo,
  npm, or a script-managed gitignored cache.

## Workspace

- Add dependencies to the workspace `Cargo.toml`, then reference them per package with `{ workspace = true }`.
- Every executable uses both `lib.rs` (the core) and `main.rs` (the entry point).
- Strategy and indicator logic lives in the library crate `trader-core`; the binary crate `trader`
  wires IO, tasks, and configuration around it.

## File structure and layout

Each source file is laid out in this order:

1. Standard library imports
2. External crate imports
3. Local imports
4. Constants and statics
5. Type definitions
6. Trait definitions
7. Struct definitions
8. Trait method implementations
9. Struct method implementations
10. Functions
11. Unit tests

Import sections appear in this order, separated by a blank line. In `lib.rs` / `mod.rs`, place each module declaration right before its corresponding `use`.

- Import only what you need, using absolute paths (`use crate::module::function`, not `use super::function`). Same for usage: `use tracing::info;` then `info!(...)`, not `tracing::info!(...)`.
- Test-only imports belong inside the `#[cfg(test)] mod tests { … }` block, not at module top level as `#[cfg(test)] use …;`. Inside test modules and test files, `use super::*` is fine.
- Organize source logically: a small module is a single file in `src`; a larger one gets its own subdirectory with a `mod.rs`.
- A type's `impl` may span several files (one per strategy, say) when each holds one cohesive area.

## File line cap and module splits

Rust source files are capped at 500 lines, enforced per package by
`scripts/check-file-loc.sh` in CI. A package opts in with a
`check_pkg crates/<pkg> 500` line; adding that line means every `.rs` file in
the package, `tests/` included, must already be under the cap (the script takes
optional exempt paths, but prefer splitting over exempting). New packages opt in
from the start.

### How to split

Convert `foo.rs` to `foo/mod.rs` (a git rename) and extract cohesive units into
sibling submodules — never arbitrary halves. Established extractions:

- `tests.rs` — the inline `#[cfg(test)] mod tests { … }` body, always the first
  choice when tests dominate the count.
- `defaults.rs` — serde `default = "…"` functions of a config module.
- `validation.rs` — `validate` functions of a config or spec type.
- `setup.rs` — construction and wiring pulled out of a process/loop module.

Moves are verbatim: no logic, doc-comment, or naming edits ride along — only
visibility and imports change. Keep each extracted file's std / external / local
import grouping.

### Visibility and `use` changes

- Extracted items get the narrowest visibility that compiles: `pub(super)` or `pub(crate)` for
  parent-only use; the submodule itself stays private (`mod setup;`) unless its API is
  deliberately public.
- A parent may glob-import or glob-re-export its own split submodules (`pub use pricing::*;`).
  Globs are avoided everywhere else; explicit names with absolute `crate::…` paths otherwise.
- After the move, prune imports the parent no longer uses (`clippy -D warnings` fails on
  leftovers).

### Tests layout

- Declare `#[cfg(test)] mod tests;` at the bottom of the parent, where the inline module sat.
- `tests.rs` starts with `use super::*;` plus what it needs; helpers go below the tests under a
  `// ── Helpers ──` divider.
- Strategy behaviour tests drive `trader-core` through its public API in `crates/trader-core/tests/`
  with recorded bar sequences and known answers. The binary's tests run against local stand-ins for
  the node. An integration-test file over the cap splits by behaviour area, with the shared
  harness in `tests/common/mod.rs`.

### Docs and verification

- Sync [agents/AGENTS.md](../../AGENTS.md) structure bullets and any crate `README.md` to the new
  paths; describe each file exactly once.
- Before pushing: run `scripts/check-file-loc.sh` (all files under cap), and verify the split was
  pure motion — diff the removed vs re-added lines; residuals must be only rewrapping, visibility,
  and imports.

## Crate README

Every crate has a `README.md` with these sections, in order: H1 title (crate name), `## Overview`, `## Features` (bullets, no trailing period per item), `## Repository Structure`, `## Usage`, `## Configuration` (the config file's sections and keys, and environment variables as `` `ENV_VAR_NAME` ``: {description}; no other CLI flags; omit the section if none), `## Testing`, `## Authors` (single line: The monkeytrading contributors), `## License` (single line: Licensed under the Apache License, Version 2.0; see `LICENSE`.). Deviate only when asked or clearly justified — ask first.

## Cargo profiles

- Release: `opt-level = 3` and `lto = true`.
- Debug: `opt-level = 2` and `lto = "thin"` to speed up compilation.
- Release also keeps `overflow-checks = true`: order arithmetic must halt on overflow, never wrap.
  Input that could overflow is rejected at the boundary first (see Numeric types).

## Comments and rustdoc

- Use rustdoc on all functions and modules; tests need less function-level doc but more line-by-line commentary.
- Document every public and private function at the function level. For fallible functions, describe error cases in this exact form:

  ```text
  /// {brief description of function}
  /// Error cases:
  /// - foo
  /// - bar
  ```

General comment terseness applies too — see [../../rules/code-comments.md](../../rules/code-comments.md).
