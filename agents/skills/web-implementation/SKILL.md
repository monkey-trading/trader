---
name: web-implementation
description: Use when writing, editing, or reviewing the console's code or configuration in this repo — TypeScript, React, Vite, Tailwind CSS, and their config files in `console/`. Covers the framework references, TypeScript, error handling, input-handling doctrine, amounts and numbers, React 19 in a single-page app, live updates over the WebSocket, module-level state, the build embedded in the Rust binary, security headers and the session cookie, configuration, Tailwind CSS 4, fonts, accessibility, motion, performance, privacy, dependencies, project layout, testing with vitest, file structure, the file line cap and module splits, the README, build configuration, and comments and TSDoc.
---

Load when implementing or modifying the console's code or configuration, or reviewing such a change.

The console is a Vite and React single-page app in `console/`. `vite build` writes static files, and
the `trader` binary embeds them and serves them at `/` next to the console API
([AGENTS.md](../../AGENTS.md)). There is no Node.js server in production: the browser loads the
files from the bot, and everything it shows comes from the bot's console API and WebSocket.

## Framework reference

- Vite and React have no docs bundled in `node_modules`: use vite.dev and react.dev, after checking
  that each documents the major version in `console/package.json`. Search before reading, and load
  only the guide the change needs.
- Heed deprecation notices. The repo conventions below and the rules win when they conflict with
  the docs' defaults; say so when they do.
- Tailwind CSS 4 is configured in CSS (see [Tailwind CSS and styles](#tailwind-css-and-styles)).

## General

- TypeScript for all source: `.ts`, or `.tsx` when the file holds JSX; `allowJs` stays off. Root
  config files may be `.mjs` or `.ts` where the tool expects it (`eslint.config.mjs`,
  `vite.config.ts`).
- Node.js from `console/.nvmrc`; npm only, with `console/package-lock.json` committed — no other
  package manager or lockfile.
- `strict` and `noUncheckedIndexedAccess` stay on; never loosen `tsconfig.json` to make code
  compile.
- Keep code simple and idiomatic — use current TypeScript and React features without
  overcomplicating.
- Avoid escape hatches: `any`, `@ts-ignore`, `@ts-expect-error`, and `eslint-disable`. If one is
  unavoidable, ask first, prefer `@ts-expect-error` (it fails once the error is gone), and
  document the line and the reasoning in a comment.
- No non-null assertions (`!`). Like an unchecked unwrap, a wrong assumption surfaces later as an
  unrelated `TypeError`. Narrow instead; where absence is an internal invariant, throw an `Error`
  that says what was missing.
- No type assertions (`as T`, `as unknown as T`) where narrowing works; `as const` is fine. One the
  type system cannot follow (`Object.fromEntries` over a fixed key list) gets a one-line comment on
  why the type holds. Check a constant table with `satisfies` rather than a cast. Data from the
  network is never asserted into a type: parse it ([Input-handling doctrine](#input-handling-doctrine)).
- Lean on the type system: literal and discriminated unions for states and kinds
  (`status: "running" | "stopped" | "failed"`), `readonly` arrays and properties for content and
  constants, and an exhaustive `switch` that ends in a `never` check. Prefer clear domain types
  over loose strings and boolean flags.
- Named exports, except where a tool requires a default export (config files).
- `import type`, or an inline `type` specifier, for type-only imports.

## Iterators and loops

- Prefer array methods (`map`, `filter`, `reduce`, `flatMap`, `some`, `every`) where they read
  better; use `for … of` where it reads better or must stop early. Never `for … in` over an array.
- Avoid nested loops, and never mutate an array while iterating it.

## Error handling

- Throw `Error` objects, never strings or other values, with a message naming what failed; rethrow
  with context through `cause` (`new Error("…", { cause })`).
- No empty `catch`, and no `catch` that logs and carries on as if nothing happened.
- An expected outcome is a value, not an exception: a refused login is `{ status: "refused" }`, and
  an order the node declined is a result the account page shows with the node's reason.
- Every promise is awaited, returned, or given a rejection handler.
- A failure shows in place, at the control that caused it, as text the user can read and act on.
  There are no toasts, and no message when the screen already shows the result.
- No `console` output in shipped code; the console has no logging or error-reporting service
  ([Privacy](#privacy)).

## Input-handling doctrine

The console takes input from the user (forms, the URL) and from the bot's API and WebSocket.
Classify every failed check on such input:

- **Detected error or inconsistent input → never act on it.** Show the user what was refused and
  why, change nothing else, and never "recover" by guessing what they meant or what the server
  meant. A figure the console cannot parse is shown as unavailable, not as zero or the last value.
- **Server data is parsed at the boundary.** The API client turns each JSON body and each
  WebSocket message into a typed value, with a check on its shape, before any component sees it.
  A message that fails the check is dropped and counted in a visible "connection" state; it does not
  crash the page or reach state half-parsed.
- **User input is untrusted.** Parse it into a typed value at the boundary, bound its length, and
  render it only as text through React. Never pass it to `eval`, `new Function`, `innerHTML`, or
  `dangerouslySetInnerHTML`, and never build a `RegExp` or a link target from it.
- **Internal invariants — states our own code cannot produce without a bug → throw an `Error`.**
  Add a terse comment on why the state is (almost) impossible. Never `console.assert` or a silent
  fallback. A broken invariant then fails a unit test or the build, before it reaches a user.
- The same check applied to several inputs lives in one shared helper, not copies.

## Amounts and numbers

- Amounts, prices, quantities, and rates arrive from the API as decimal strings (the engine's
  `Fixed`) and stay strings, or integers in a stated unit, until display. Never `parseFloat` or
  `Number` a value into arithmetic that decides anything; sums, differences, and comparisons run on
  scaled `bigint`s or integers in a stated unit.
- Name the unit in the identifier or its TSDoc (`marginBps`, "equity, in USDC").
- Integer division truncates toward zero (`Math.trunc`); any other rounding is the server's own
  rule for that value, cited in a comment.
- Values stay within `Number.MAX_SAFE_INTEGER`; one that could exceed it is a `bigint`.
- Display formatting goes through `src/lib/format.ts`, which formats without the user's locale, so
  the same value reads the same everywhere and tests do not depend on the machine. Floats are for
  layout, charts, and animation, never for a value the console shows as an amount.
- The console computes no figure the bot already serves (equity, margin level, profit and loss);
  it shows the bot's answer. Totals across accounts come from the API, or are sums of served
  strings in `bigint`.

## React

- The app is client-rendered: there are no server components, no hydration, and no `"use client"`.
- State lives in the smallest component that needs it. Server data comes through one store per
  concern (accounts, the live feed), read with `useSyncExternalStore`, so a component re-renders
  only for what it shows.
- **Effects synchronize with systems outside React** — timers, observers, listeners, the
  WebSocket — and nothing else. Compute derived values during render, respond to a user in the
  event handler, and never copy props into state through an effect.
- **StrictMode is on** (`main.tsx` wraps the app in `<StrictMode>`), so in development every effect
  runs setup, cleanup, setup. Each cleanup undoes exactly what its setup did, and async work
  started in an effect ignores its result after cleanup (an `ignore` flag or an `AbortController`).
- Hooks follow the `react-hooks` lint rules with complete dependency lists. A callback an effect
  calls but must not re-run on is wrapped in `useEffectEvent`, never mirrored into a ref.
- Memoize (`useMemo`, `useCallback`, `memo`) only for a measured cost or an identity a dependency
  list needs.
- Keys are stable ids from the data (an address, an order id), never the index of a list that
  inserts, removes, or reorders.
- Refs are read and written in effects and handlers, never during render.
- Controls are native elements: `<button type="button">` for actions, `<a href>` for navigation,
  and a `<form>` with `onSubmit` for input that submits. No click handler on a `div` or `span`.
- Ids that tie labels, descriptions, and controls together come from `useId`, never hand-built
  strings that collide when a component renders twice.
- A control that changes server state shows its pending and failed state in place and cannot be
  submitted twice while pending.

## Live updates

- One WebSocket connection per tab, owned by the live store, opened after login and closed on
  logout. Components subscribe to the store; none opens its own socket.
- A reconnect backs off with a cap and jitter, resyncs from the REST read before it applies live
  messages again, and shows the connection state in the page frame. No data is shown as live while
  the socket is down.
- Updates are coalesced to at most one render per animation frame, and every list the feed grows
  (fills, activity, curve points) has a fixed cap, oldest dropped. Nothing grows without bound
  while the tab stays open.
- Figures update by themselves: there is no refresh button and no live switch.
- The effect that opens the socket cleans up fully under StrictMode's double run.

## Module-level state

No mutable module-level state. A `let` or a mutable collection at module scope is shared by every
component instance in the browser and by every test in a test file: a test cannot reset it, and a
second instance of a component cannot have its own.

- **Keep state in components and pass it down,** or derive it, or hold it in a store created in
  `main.tsx` and passed through context.
- **Allowed:** immutable values that depend on nothing at runtime (`readonly` tables, `as const`
  content, compiled regular expressions).

## Routing and the embedded build

- Routes are client-side. The bot serves `index.html` for any GET under `/` that is not an API
  route or a built file, so a reload on an account page works; nothing else on the server renders
  HTML.
- Asset URLs are absolute from `/` (`base: "/"`): the console is served from the bot's root, never
  from a sub-path or a CDN.
- `npm run build` writes `console/dist/`, a build output that is gitignored. The Rust build embeds
  it, and a build without it serves a page saying so. A change to the console is checked as the
  embedded bundle, not only through the dev server.
- `npm run dev` runs Vite's dev server on loopback and proxies the API paths and the WebSocket to a
  running bot. It is a development tool: never bound to a public interface, never part of an image.
- The build is reproducible from the lockfile: `npm ci`, then `npm run build`, with no network
  access beyond the registry and no step that rewrites a committed file.
- The bundle makes no request at load but to its own origin: fonts, images, and scripts are built
  files.

## Security headers and the session

The bot sets the headers on every console response; the console assumes them and never works around
them.

- `Content-Security-Policy`: `default-src 'self'`; `script-src 'self'`; `style-src 'self'`;
  `img-src 'self' data:`; `font-src 'self'`; `connect-src 'self'`; `object-src 'none'`;
  `base-uri 'self'`; `form-action 'self'`; `frame-ancestors 'none'`. Vite emits external script
  and style files, so no `'unsafe-inline'` or `'unsafe-eval'` is needed: code that needs one is
  rewritten, and a dev-only exception lives in the dev server's config, never in the bot's policy.
- `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`, `X-Frame-Options: DENY`,
  `Cross-Origin-Opener-Policy: same-origin`, and a `Permissions-Policy` that turns off `camera`,
  `microphone`, `geolocation`, `payment`, `usb`, and `browsing-topics`.
- Never widen the policy with a wildcard, a scheme source such as `https:`, or another origin
  without the owner's approval; a new kind of origin is a privacy change ([Privacy](#privacy)).
- The session is a cookie the bot sets with `HttpOnly`, `Secure`, and `SameSite=Strict`, so the
  console's code never reads it. The console stores no token, password, or key anywhere in the
  browser. State-changing requests are `POST`, `PUT`, or `DELETE` with a JSON body; the bot refuses
  any whose `Origin` is not the console.
- No route the console calls returns a key, and nothing the console can do moves funds away from
  an account's own key. A control for such an action is a defect, whatever the server allows.
- `dangerouslySetInnerHTML` is not used.
- Read the headers from the running bot with the embedded build (`curl -skI`), not only from the
  Rust config.

## Configuration and environment

- The console needs no secret and no runtime environment variable: it talks to its own origin. Only
  `VITE_*` variables reach the browser, at build time, and everything in them is public; never put
  anything secret in one. A new variable needs a safe default or a build that fails without it, and
  a line in the README.
- No `.env` file is committed (`.env*` is gitignored), and none is needed to build or run the
  console.
- **Classify every new constant.** Server-side limits and bounds come from the API, not a copy in
  the console; labels and copy in the module that renders them; colours, type sizes, and
  animations in the `@theme` tokens of the stylesheet.

## Tailwind CSS and styles

- Tailwind CSS 4, configured in CSS: `src/styles.css` imports `tailwindcss` and declares the design
  tokens in `@theme`, and Vite runs `@tailwindcss/vite`. There is no `tailwind.config.*` file.
- Class detection is limited to `src/`, so words and example classes in `agents/` or the README add
  no rules to the stylesheet.
- Colours come only from theme tokens. A raw colour in a class or a `style` is a defect: contrast is
  checked per token pair, and an off-token colour escapes that check. A new colour is a new
  `--color-*` token, checked against every background it sits on, in light and dark themes.
- Arbitrary values are for one-off adjustments. A value repeated across files is a design
  decision: it becomes a `@theme` token used by name.
- Style in the markup with utility classes. The stylesheet holds only the tokens, keyframes, base
  element styles, shared classes no utility expresses, and the reduced-motion rule. No CSS modules
  or CSS-in-JS.
- Mobile first: unprefixed classes style the narrowest screen, 320 CSS px, and `sm:`, `md:`, and
  `lg:` add to it. Nothing scrolls horizontally at 320 px; a wide table scrolls inside its own
  labelled container.
- Compose conditional classes with template literals; a class-merging package is a dependency
  decision ([Dependencies](#dependencies)).
- No `!important` outside the reduced-motion rule.

## Fonts

- Fonts are files in the repository or an npm package (an exact version), imported from the
  stylesheet so Vite builds them into the bundle and the bot serves them. Never a `<link>` or
  `@import` to a font host: `font-src 'self'` blocks it, and it would send the user's address to a
  third party.
- Load only the weights and styles the app uses, with `font-display: swap`. A system font stack is
  always acceptable.

## Accessibility

The bar is WCAG 2.2 AA; the [development checklist](../../rules/development-checklist.md)
(Console) asks the questions. Implementation conventions:

- The page is framed by `<header>`, `<main id="main">`, and `<footer>`. The first focusable element
  is a skip link to `#main`, and `main` takes `tabIndex={-1}` so the link can move focus to it. A
  route change moves focus to the new page's `h1`.
- One `h1` per page; each section has a heading, and headings nest without skipping a level.
- Focus is visible on every interactive element through a `focus-visible:` style. An outline is
  removed only where a replacement indicator shows, or on a programmatic focus target
  (`tabIndex={-1}`) that is not interactive.
- Every input has a `<label>`. A control whose visible text does not say what it does gets an
  `aria-label` that starts with that visible text (WCAG 2.5.3).
- Figures that change many times a second are `aria-hidden`, beside a static description of what
  they show. A result the user caused, such as a failed start or a flattened account, is announced
  through an `aria-live="polite"` region that exists before it changes, and the error text sits at
  the control that caused it.
- Tables use real `<table>` markup with `<th scope>`; a chart has a text alternative of its latest
  values.
- Colour is never the only carrier of meaning: profit and loss, and states, also use text or a
  sign.
- Targets are at least 24 × 24 CSS px, and the app works at 320 CSS px wide and at 200% zoom.
- Text for screen readers only uses `sr-only`; a scroller that holds one is `relative`.
- `lang="en"` on `html`.
- `eslint-plugin-jsx-a11y` runs its strict config at error. Lint is not conformance: check a change
  in a browser with the keyboard alone, the accessibility tree, 200% zoom, and a 320 px viewport.

## Animation and reduced motion

- Motion is rare: a number changing is not animated. CSS animations are `--animate-*` tokens with
  their `@keyframes` in `@theme`. Animate `transform` and `opacity`; never layout properties
  (`width`, `height`, `top`, `left`, `margin`, `padding`, `font-size`).
- The `prefers-reduced-motion: reduce` rule in the stylesheet stops every CSS animation and
  transition; never override it.
- No timer or `requestAnimationFrame` loop runs in a hidden tab unless the live store needs it.
- No animation library without the owner's approval ([Dependencies](#dependencies)).
- Nothing flashes more than three times a second.

## Performance

- The bundle is embedded in the binary and downloaded by the user: keep it small. `vite build`
  prints each file's size; record the totals before and after a change that adds a dependency or a
  page, and justify growth. A page loads its chart library only where a chart is drawn
  (`React.lazy` with `import()`).
- Import a package's specific entry point, so the bundle carries only what the app uses.
- Reserve the space of everything that renders late, so nothing shifts the layout.
- Logic lives in `src/lib` as pure functions; a component holds only what changes. Sorting,
  summing, and formatting for a long list run once per update, not once per row.
- Lists that can grow large (fills, activity) are capped or windowed.
- Measurements that need a browser (Lighthouse, the network panel) run from a scratch directory
  outside the repository, at an exact tool version, against the embedded build served by the bot,
  with the report kept in a gitignored run directory under `agents/.processes/dev-runs/`.

## Privacy

- No analytics, trackers, fingerprinting, third-party scripts, fonts, images, iframes, or embeds
  without the owner's explicit approval. Every request the console makes is same-origin: it talks
  to the bot that served it.
- Browser storage (`localStorage`, `sessionStorage`) holds per-user interface conveniences only —
  a chosen theme, a collapsed section, a table sort — read tolerantly, wrapped in `try`/`catch`,
  with the page correct when it is empty or unavailable. It never holds a key, a password, a token,
  or account data.
- Nothing the user types leaves the browser, but for what the console sends to the bot.
- Links to other sites are plain links that open without a referrer; nothing is fetched from them.

## Dependencies

- Reuse before adding: the platform, React, and Vite before a package.
- Ask before adding a runtime dependency (`dependencies`): it ships in the binary. A development
  dependency may be added for tooling with a stated reason. Both follow the rules below.
- Exact versions only (`.npmrc` sets `save-exact=true`): no `^`, `~`, tag, git, URL, or `file:`
  specifier.
- Official packages only: verify the exact name, publisher, and repository on the registry
  (`npm view <package> name repository.url maintainers`) against typosquatting.
- `npm audit --audit-level=high` passes with every change, as CI enforces.
- A bump links the release notes and confirms the behavior change is expected. A major bump of
  Vite, React, Tailwind CSS, TypeScript, ESLint, or vitest is its own PR.
- The lockfile is written only by npm, never by hand, and committed with the `package.json` change
  that caused it.
- Licences: a dependency is under an open-source licence that allows Apache-2.0 distribution (MIT,
  ISC, BSD, Apache-2.0 and the like); check it with `npm view <package> license`.
- No wallet library, key handling, or signing in the browser: the bot holds the keys and signs.
- Tools outside the build — Playwright, Lighthouse — are not dependencies of this repository: run
  them from a scratch directory outside it, at an exact version of the official package.

## Project layout

- One package in `console/`. `@/*` maps to `src/*`: import across directories with `@/`
  (`@/lib/format`), and within a directory with `./`.
- `src/main.tsx` mounts the app; `src/api/` holds the typed API client, response parsers, and the
  live store; `src/pages/` holds one component per route; `src/components/` holds shared React
  components; `src/lib/` holds pure TypeScript with no React, DOM, or browser API.
- Logic a component needs lives in `src/lib` as a pure function with tests, and the component
  calls it.

## Testing

- Vitest runs `src/**/*.test.ts` in the Node environment, and `src/**/*.test.tsx` in a DOM
  environment with Testing Library (`vitest.config.ts`). Each `src/lib` and `src/api` module with
  logic has its tests beside it as `<module>.test.ts`.
- Test the logic first: components stay thin, so their logic sits in `src/lib` and is tested there.
  A component test covers what the user does — a form submits once, a failure shows at its control,
  a keyboard path works — with queries by role and name, not by class or test id.
- Parsers get tests for valid bodies, missing fields, wrong types, and extra fields.
- Behavior only a browser shows is checked on the embedded build through the dev scenario
  ([agent-build-feature](../agent-build-feature/SKILL.md)).
- One `describe` per function or behavior area; each `it` names the behavior it pins.
- Assert whole values (`toEqual` on the full result) over fragments, and exact numbers over ranges.
- Time and randomness come in as arguments or a seed; no real timers where a fake clock will do.
- Run one file with `npm test -- src/lib/<module>.test.ts`.

## File structure and layout

Each source file is laid out in this order:

1. External imports (packages)
2. `@/` imports
3. Relative imports, then stylesheets
4. Types and interfaces
5. Constants and module-level values
6. Components and functions

Import groups appear in this order, separated by a blank line.

- Import only what you use, by name.
- `src/components/` and `src/pages/` hold one exported component per file, named like the file in
  PascalCase (`AccountPage.tsx` exports `AccountPage`). A small component used only by that file
  may sit below its export.
- `src/lib` and `src/api` modules are kebab-case (`format.ts`, `live-store.ts`), with tests beside
  them as `<module>.test.ts`.
- `.tsx` only for files that hold JSX.

## File line cap and module splits

Source files (`.ts`, `.tsx`, `.mjs`), tests included, are capped at 300 lines, enforced by ESLint's
core `max-lines` rule (`"max-lines": ["error", 300]`, which counts blank lines and comments) in
`console/eslint.config.mjs`, which `npm run lint` and CI run. Prefer splitting over exempting: an
`eslint-disable` for `max-lines` needs the owner's approval.

### How to split

Extract cohesive units — never arbitrary halves:

- A page section becomes its own component; the page keeps the composition.
- A component's logic becomes a pure function in `src/lib`, with its own tests.
- A large `src/lib` module splits by area into sibling modules.
- A test file splits by behavior area into `<module>.<area>.test.ts`; fixtures they share go in a
  sibling module the tests import.

Moves are verbatim: no logic, comment, or naming edits ride along — only imports and exports
change. Keep each new file's import grouping.

### Exports after a split

- Export only what another file uses; a helper only the moved code uses stays unexported in its
  new file.
- The original module may re-export what moved so its callers keep their imports. Re-exports are
  avoided everywhere else, with no barrel `index.ts`.
- Remove the imports the original no longer uses (`npm run lint` fails on leftovers).

### Docs and verification

- Sync the [agents/AGENTS.md](../../AGENTS.md) Structure bullets to the new paths; describe each
  file exactly once.
- Before pushing: `npm run lint` passes, so every file is under the cap, and the split was pure
  motion — diff the removed against the re-added lines; residuals are only imports, exports, and
  rewrapping.

## README

The root `README.md` is the public introduction, in prose; it names how to build the console
(`npm ci`, `npm run build` in `console/`, with Node.js from `console/.nvmrc`) and run it against a
bot (`npm run dev`). Keep those commands in step with `console/package.json`.

## Build configuration

- `vite.config.ts` sets `base: "/"`, the `@` alias, the dev server's proxy to a local bot, and
  `build.outDir` (`dist`). It enables no plugin beyond React and Tailwind without a stated reason.
- `tsconfig.json` keeps `strict`, `noUncheckedIndexedAccess`, `isolatedModules`, `noEmit`, and the
  `@/*` path. `npm run typecheck` runs `tsc --noEmit`.
- `package.json` scripts: `dev`, `build`, `lint`, `typecheck`, `test`. CI runs the last four.

## Comments and TSDoc

- Use TSDoc (`/** … */`) on every top-level function, component, and hook, exported or not, and a
  file comment at the top of every module saying what it holds; closures inside them need none.
  Tests need less function-level doc but more line-by-line commentary.
- For a function that throws, describe the error cases in this exact form:

  ```text
  /**
   * {brief description of function}
   * Error cases:
   * - foo
   * - bar
   */
  ```

- Document a component's props on its props type, one line per prop whose name does not say
  enough.

General comment terseness applies too — see [../../rules/code-comments.md](../../rules/code-comments.md).
