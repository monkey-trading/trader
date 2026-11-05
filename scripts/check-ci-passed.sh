#!/usr/bin/env bash
# The `ci-passed` predicate: passes only if every job in $NEEDS (the workflow's `toJSON(needs)`)
# succeeded, or was skipped while `changes` set its area false. A job missing from the map must
# succeed, and so must `changes`. Add a gated job to the map below as well as to `needs`.
set -euo pipefail

: "${NEEDS:?}"
echo "$NEEDS"
jq -e '
  .changes.outputs as $run
  | {fmt: "rust", "file-loc": "rust", clippy: "rust", test: "rust", "cargo-deny": "rust", "dev-scenarios": "rust", "dev-strategies": "rust",
     console: "console", "agent-links": "agent_docs", "review-delta": "agent_scripts",
     "merge-approval": "merge_approval", "ci-scripts": "ci_scripts"} as $area
  | all(to_entries[]; .value.result == "success"
      or .value.result == "skipped" and $run[$area[.key] // ""] == "false")' <<<"$NEEDS" >/dev/null
