# Vendored skill sources

Load when reviewing provenance, pulling, or refreshing `skills/vendor/`.

## How vendoring works (pull-on-demand)

Vendored packs are **not** committed. Each pack is pinned to an upstream commit in
[`vendor.manifest`](vendor.manifest) and materialized on demand by [`pull.sh`](pull.sh),
which downloads the commit-pinned tarball and extracts it to `skills/vendor/<pack>/`.
Those pack directories are gitignored.

- Bootstrap a checkout: `agents/skills/vendor/pull.sh` pulls every missing pack; packs already
  present are left as-is.
- Pull one pack: `agents/skills/vendor/pull.sh <pack>`.
- Refresh to a newer upstream commit: bump the hash in that pack's `vendor.manifest` URL, then
  `agents/skills/vendor/pull.sh --force <pack>`.

`vendor.manifest` is the single source of truth for the commit each pack is pinned to. Because
packs are fetched from source at use time rather than copied into this repo, the repo stays small.

## What is vendored here

- `rust-skills/` — the Rust deep-dive skill pack.

The pack lands at `skills/vendor/rust-skills/` after `pull.sh`; route into it only through the
wrapper skill ([rust-implementation](../rust-implementation/SKILL.md)), never by loading the whole
tree. Once the wrapper selects a vendored file, that load is mandatory. If the pack or file is
missing, run `agents/skills/vendor/pull.sh rust-skills`, retry the exact read, and stop with the
missing path if it still fails. Do not substitute memory, adjacent skills, or wrapper-only
guidance.

The full upstream tree is retained so relative references, scripts, and templates resolve. Load
only the selected skill. Upstream tool and model names must be adapted to available capabilities;
repository execution permissions still apply.

## Upstream source

The pack's repository, pinned commit, and subtree are in [`vendor.manifest`](vendor.manifest).
Its scope is the whole repository; the wrapper routes into `skills/` starting at
`skills/rust-router/SKILL.md`.

## Refresh procedure

1. Find the new upstream commit (latest default branch) with `git ls-remote <repository> HEAD`.
2. Update the pack's tarball URL in [`vendor.manifest`](vendor.manifest) to the new commit hash.
3. Re-pull: `agents/skills/vendor/pull.sh --force rust-skills`.
4. If the upstream layout moved, update the `subtree` column in the manifest and any wrapper
   routing paths that name vendored files.
5. Validate the wrapper routing targets resolve against the freshly pulled tree (`SKILL.md`,
   routing tables, checklists, module / reference paths).
6. If workflow guidance changed, update the wrapper: `../rust-implementation/SKILL.md`.
