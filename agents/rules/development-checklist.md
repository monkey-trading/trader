# Development checklist

Load at the start of every coding task, before the first edit, and keep it applied until the work
is done; load it for every code review.

**Rule**: It lists what a change must not miss, and it governs every stage — not a final gate.

## How to use it

- **Before the first edit:** read the applicable sections; they shape the design and the plan.
  Design comes first.
- **While writing code:** apply each applicable item as you write. Concurrency, crash recovery,
  error handling, observability, and tests are built in from the start, never bolted on at the end.
- **Before calling the work done or opening the PR:** self-review the final diff against every
  applicable section and fix what it finds.
- **Reviewing someone else's change:** walk the same sections over their diff.
- A section applies when the change touches what its **When** line names; sections without one
  always apply.
- Each item is a question. An unmet item the code, config, or tests can settle is a defect: fix it
  while developing, report it while reviewing. An item they cannot settle is stated as unverified,
  never assumed.
- Weigh each item against what the code is for and where it runs. A dev-only mock, a test harness,
  or a bot that runs as one process by design does not owe production hardening: note an item
  that does not apply, with the reason, rather than adding machinery for it. Every guard you add
  names the concrete scenario in this deployment that needs it.

## 1. Intent and scope

- Does the change do what its issue or task asks, and is anything it requires missing?
- For a feature: is it complete — every acceptance criterion met, the feature built to run end to
  end on the dev stack, and its dev scenario's check passing — and does any part of its design
  still wait on an open decision
  ([agent-plan-feature](../skills/agent-plan-feature/SKILL.md))?
- Does the change carry edits unrelated to that intent?
- Is the solution sized to the problem — no speculative abstraction, configurability, or
  abstraction with one caller ([minimal-change](minimal-change.md))?

## 2. Design and data flow

When: calculations, persistence, module or crate boundaries, new or updated dependencies.

- Which service or component owns the calculation and the authoritative data? Is there one
  source of truth, or can two derivations drift?
- Trace the data flow end to end: where it enters, where it is persisted, what is cached, and how
  the cache is invalidated.
- What new dependencies — services, crates, infrastructure — does it introduce, and what happens
  if each fails at the worst time?
- Is each new library needed — nothing in the workspace or standard library already does it — and
  within the dependency rules of the language's `*-implementation` skill?
- Is each new dependency from its official registry or upstream repository, with its exact name and
  publisher verified against typosquatting?
- Does each dependency bump link its changelog or release notes and confirm that the resulting
  behavior is expected?
- How do scheduling, retries, persistence, and recovery interact?
- Does the responsibility sit in the right service or module, with the smallest public surface?
- In the code the change touches, does each layer do only its own job? Indicators, strategies,
  sizing, and risk limits live in `trader-core` and are testable without IO; the `trader` binary
  streams, signs, sends, and persists, and makes no trading decision; API handlers and the console
  translate requests into commands and delegate.
- Is it consistent with similar implementations in the repository, and is any departure
  justified?

## 3. Concurrency

When: async tasks, threads, multiple replicas, shared state, writes from more than one place,
retries, queues.

- Is in-process state owned by one task and changed only through messages sent to it? That is the
  repo's convention — each account's state has one owner, and reads go through it as queries;
  shared state behind a lock needs a stated reason the single-owner design does not fit.
- Is every queue bounded, with a stated policy when it fills (back-pressure, drop the oldest, or
  fail the account), never unbounded growth?
- Can two tasks, threads, or replicas act on the same state at once? What serializes them — a
  single owner, a unique constraint, a transaction, a row lock, or an advisory lock?
- Is every read-modify-write atomic (compare-and-set, conditional `UPDATE`, `SELECT … FOR
  UPDATE`), and is a failed compare-and-set handled, never swallowed?
- Is every retried or redelivered operation idempotent — an idempotency key, a unique constraint,
  or an upsert?
- Does ordering matter, and is it guaranteed or enforced?
- Are locks held for the shortest scope — no await while holding one, no lock-order deadlock?
- Is async code cancellation-safe: can a dropped future (timeout, `select!`, shutdown) leave
  state half-updated?
- Does it still hold with more accounts in the process, and with two bot processes holding the
  same key? A nonce is one sequence per account: who else may spend it?

## 4. Crash recovery and restart

When: stateful services, background jobs, multi-step operations, in-memory state, external side
effects, anything the bot keeps between restarts.

- If the process dies at any line — between two writes, or after an external call but before
  recording it — what state is left, and does the next start repair it?
- Is a crash logged, and does it stop the service? A shared background worker, task, or thread
  that dies must stop or restart the whole service — something awaits or supervises it — so the
  service never runs on without that loop while looking healthy. The exception is one account's
  task stopping on rejected input: it ends that account alone and is reported.
- Does anything that must survive a restart live only in memory: counters, nonces, queues, or a
  cache that is the source of truth?
- On startup, does the service reconcile in-flight work from persisted state (and from the node,
  the log offset, or the chain) instead of starting blank or repeating it? Position, balance,
  nonce, and resting orders are read back from the node, never assumed.
- Is each side effect recorded before, or atomically with, its trigger, so a restart neither loses
  nor duplicates it?
- Can work stuck in an intermediate state be detected and resumed or failed, with a timeout?
- Does shutdown drain or checkpoint in-flight work on `SIGTERM`, and does the service come back
  from a kill or restart without an operator when the state it finds is consistent?
- Does a restart or a dropped connection resume the stream from the last verified offset, with no
  gap and no replayed fill counted twice, and does a node that signs with a different key than the
  signer its attestation named stop the bot rather than trade?
- When the state it finds is inconsistent — persisted data that contradicts itself, the node, or
  the chain — does it raise a Critical alert and stop that account without changing anything, or
  fail closed where continuing could trade on a wrong position, for the user to repair? It never
  guesses a repair.

## 5. Persistence, rollout, and compatibility

When: stored formats, config, keystores, the console API, the engine crates' `rev`, deploy.

- Does the bot sign exactly what the node checks? The engine crates are pinned by one Cargo `rev`,
  `Cargo.lock` records the commit, and a bump names the node releases it supports.
- Is a deploy ordering required — bot against node release, config against code — and is it
  stated in the PR?
- Is rollback safe, including for the data directory, keystores, and stored hashes the new version
  already wrote?
- Do config, console API, and stored-format changes stay compatible for existing users, or is the
  break stated and migrated?
- Are new config values and secrets set for every environment, with safe defaults?

## 6. A pure core

When: `trader-core`, indicators, strategies, sizing, or risk limits.

- Does `trader-core` do no IO: no clock, network, file, environment, or randomness? Time and
  prices arrive as arguments, so a recorded log replays through it to the same decisions.
- Is every input a decision depends on passed in — bars, marks, equity, positions, limits — and
  is the order of anything that affects a decision fixed (`BTreeMap`, sorted keys, arrival order)?
- Does an indicator update in constant time per bar, with a known-answer test against values
  computed independently?
- Is valid input handled without a panic? Only internal invariants may assert, and a market-data
  value is input.

## 7. Money and precision

When: amounts, prices, rates, fees, balances, share calculations.

- Are values decimal or integer units, never binary floats?
- Is every rounding mode and direction explicit and safe where it matters? `Fixed` multiplication
  and division truncate toward zero; is that the right direction for this order size, notional, or
  limit check?
- Are overflow, underflow, and division by zero impossible or handled?
- Are units and decimal places converted consistently at every boundary?

## 8. Chain data

When: RPC reads, events, receipts, transaction submission (the vault calls).

- Are reorgs and confirmation depth handled?
- Does a read across backends compare the same block, and does an absence carry the answering
  node's height before it counts as "not there"?
- Are nonces, gas, and resubmission safe under retries, concurrent senders, and restarts?

## 9. Errors

- Is every error handled or propagated with context, never swallowed?
- Does every external call have a timeout, and a bounded retry?
- Is each new failure visible: a log at the right level, a metric, and an alert where the user
  must act?

## 10. Observability

When: new or changed logging, units of work, task or service boundaries, alerts.

- Does each unit of work — a request, a message, a job run, an operation — get one correlation
  scope carrying its id, so every line beneath it carries that id? Long-running loops get none.
- Does the id follow the work across every boundary — into spawned tasks, through queues and
  messages, and into calls to other services? A unit of work started from another records the
  originating id in a role field, so the chain stays joinable.
- Is every id in its own field — the subject's under `id`, other entities under role names
  (`order_id`, `account`, `seq`) — and never only inside a message string?
- Are levels right: `info` for a state transition of an identifiable thing, `debug` for per-step
  detail and "why nothing happened", `trace` for mechanical echoes with no decision?
- Can a failure be diagnosed from the logs alone? Following one id shows every step the unit took —
  its inputs, each decision and its reason, the external calls and their results.
- Is each alert fired inside the unit's scope, so it carries the same ids as the logs? Is its
  severity right — Critical only when the system is blocked and an operator must act now, Warning
  for anything that can wait — and does it fire once per episode rather than on every check?
- Are logs free of secrets and personal data?

## 11. Security and trust boundaries

When: user, API, chain, or config input; auth; secrets; crypto; external calls.

- Is authorization checked where the action happens, not only upstream?
- Is input validated at the boundary it crosses, and when a check fails, does the code refuse to
  act — alert, log, change nothing — rather than "recover" on data it just proved wrong?
- Are secrets kept out of code, logs, state, and committed files? Key material lives in a
  redacting, zeroing type and is read only where it signs; no console route returns a key, and
  nothing the console can do moves funds away from an account's own key.
- Is all crypto from vetted libraries, never home-rolled?
- Is every answer from the node verified (`verify_response`, `FrameVerifier`) against the
  configured or attested signer before the bot acts on it, and a failed check treated as no data?
- Does everything the bot signs carry the EIP-712 domain (chain id and vault), a nonce, and an
  expiry, built by the pinned engine crate rather than a copy of its hashing?
- Is the console's login, session cookie, `Origin` check, and rate limit intact for the routes the
  change adds?

## 12. Performance and resources

When: hot paths, loops over external data, queries, caches, queues.

- Any unbounded loop, query, memory growth, or queue?
- Any N+1 query or missing index for a new query shape?
- Any blocking call on an async path, or CPU-heavy work on the tokio runtime's worker threads?
- Is the work per stream message bounded? A scan over every account on the message path needs a
  stated limit or a plan to remove it, and a hot path allocates nothing per message once warm.

## 13. Tests

- **They test what they claim.** A test's name matches its assertions, and each assertion would
  fail if the behavior broke. No tautologies, no asserting that a mock returned what it was told,
  no bare `is_ok()` or no-panic check where the value matters.
- **Mutation-checked when the tests run anyway.** For a new or changed test, make one temporary
  edit to the code under test that breaks exactly what the test's name claims, and run it: that
  test must fail and nothing unrelated should. Undo the edit by hand, never with `git checkout` or
  `git restore`, which also discard uncommitted work. A test that stays green is a defect. Without
  a test run, ask: if this line were inverted, which assertion would fail?
- **They cover the changed branches.** The success path, every new or changed error path,
  boundaries (zero, empty, maximum, off-by-one), missing or `None` inputs, retries and timeouts,
  and every failure mode sections 2–12 found — including concurrent callers and a restart from
  persisted state where the behavior is testable.
- **Right level.** Unit tests for logic; integration tests where the network, file, or chain
  boundary is the behavior. End-to-end coverage is the dev scenario's job.
- **Deterministic.** No sleeps for synchronization; time, randomness, and network are controlled;
  no dependence on test order or on shared global state.
- **Regression.** A bug fix carries a test that fails without the fix.
- **Not weakened.** A deleted, skipped, or ignored test, a loosened assertion, or a widened
  tolerance is a defect unless the PR justifies it.
- **Realistic data.** Amounts use production precision; fixtures match real shapes.
- **Actually runs.** The tests sit in a CI lane that runs for these paths.

## 14. Console

When: anything under `console/`, the console API, the headers or session the bot serves it with.
The accessibility bar is WCAG 2.2 AA.

- **TypeScript.** Do `strict` and `noUncheckedIndexedAccess` stay on, with no `any`, `@ts-ignore`,
  non-null assertion, or `eslint-disable` the user did not approve, every type assertion
  justified, and every `switch` over a union ending in a `never` check? Do `npm run typecheck` and
  `npm run lint` pass?
- **React.** Does every effect survive StrictMode's double run with no leaked timer, listener, or
  socket, and ignore async results after cleanup? Is each effect synchronizing with something
  outside React, with complete dependency lists and stable list keys?
- **Server data.** Is each API body and WebSocket message parsed into a typed value at the
  boundary, and does a message that fails the check change nothing? Are amounts decimal strings or
  scaled integers, never floats, and is nothing the bot serves recomputed in the browser?
- **Live updates.** Does one connection serve the tab, with capped lists, a backed-off reconnect
  that resyncs before applying messages, and a visible connection state? Are there no refresh
  buttons or live switches?
- **Failures.** Does a failure show in place, at the control that caused it, with no toast and no
  message where the screen already shows the result? Can a pending control not be submitted twice?
- **Accessibility.** Is everything reachable and operable by keyboard alone, with visible focus and
  focus moved deliberately on a route change? Does every control and input have an accessible
  name, do text and components reach their contrast ratios on the tokens actually paired, and are
  targets at least 24 × 24 CSS px? Does the page work at 320 CSS px and 200% zoom with no
  horizontal page scroll? Are fast-changing figures `aria-hidden` behind a static description, and
  results of the user's actions announced through a polite live region? Was the change checked in
  a browser, or stated unverified?
- **Security.** Are the headers and the cookie flags (`HttpOnly`, `Secure`, `SameSite=Strict`) the
  bot serves unchanged or tightened, with the CSP free of `'unsafe-inline'` and `'unsafe-eval'`?
  Does the console hold no key, password, or token in the bundle, storage, or the DOM, and does no
  route it calls return a key or move funds away from an account's own key? Is user input parsed,
  bounded, and rendered only as text?
- **Privacy.** Does the change add analytics, trackers, third-party requests, fonts, scripts, or
  embeds, or put account data in browser storage? Each needs the owner's approval first.
- **Supply chain.** Is every dependency an exact version under a licence that allows Apache-2.0
  distribution, from its official package, with the lockfile written by npm and `npm audit
  --audit-level=high` passing? Is every GitHub Action pinned to a full commit SHA?
- **Build.** Does the production build (`npm run build`) embed in the binary, with its size
  measured when the change adds a dependency or page?

## 15. Code quality and domain skills

- Does the change follow every `*-implementation` skill for the files it touches — these, and any
  other the [AGENTS.md](../AGENTS.md) index lists?
  - [rust-implementation](../skills/rust-implementation/SKILL.md) — the Rust workspace.
  - [web-implementation](../skills/web-implementation/SKILL.md) — the TypeScript console.
- Does it add process-global mutable state — statics, singletons, a library's default registry?
  That is a hidden dependency: it breaks test isolation and allows only one instance per process.
  The language's `*-implementation` skill lists the exceptions.
- Clean and idiomatic, with no dead code, unused dependencies, or leftover debugging the change
  created.
- Names come from the design or issue vocabulary, spelled out (`client_qty`, not `figures` or
  `data`). A constant or variable whose name no longer describes its use gets a new name, not
  a new use.
- Comments are terse and describe current behavior ([code-comments](code-comments.md)).

## 16. Docs and indexes

- Are the nearest `AGENTS.md`, README, and runbooks updated for changed commands, paths, env vars,
  or behavior ([keep-agents-index-in-sync](keep-agents-index-in-sync.md))?

## 17. Workspace rules

- Does the change follow every rule and skill [AGENTS.md](../AGENTS.md) selects for it?
