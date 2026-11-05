#!/usr/bin/env bash
# Tests scripts/ci-areas.sh (the path classifier) and scripts/check-ci-passed.sh (the `ci-passed`
# predicate), and checks that every job in .github/workflows/ci.yaml is in `ci-passed`'s `needs`
# and, when it is gated, in the predicate's area map. Prints one line per case and exits 1 if any
# case fails. Needs jq and yq.
set -euo pipefail
cd "$(dirname "$0")/.."
workflow=.github/workflows/ci.yaml
fail=0

# check <name> <expected> <actual>
check() {
  if [[ "$2" == "$3" ]]; then
    echo "ok   $1"
  else
    echo "FAIL $1: expected '$2', got '$3'"
    fail=1
  fi
}

# areas <path>... : the sorted, unique areas the classifier selects for these paths.
areas() {
  printf '%s\n' "$@" | ./scripts/ci-areas.sh | tr ' ' '\n' | sort -u | tr '\n' ' ' | sed 's/ $//'
}

every="agent_docs agent_scripts ci_scripts console merge_approval rust"
check "a Rust source file" "rust" "$(areas crates/trader/src/lib.rs)"
check "the manifests, lockfile, toolchain and policy" "rust" \
  "$(areas Cargo.toml Cargo.lock rust-toolchain.toml rustfmt.toml deny.toml)"
check "the example config and the dev scenarios" "rust" "$(areas config/example.toml scripts/dev-trade-one.sh scripts/dev-trade-many.sh scripts/dev-strategies.sh)"
check "the dev stack feeds the dev scenario and its test" "ci_scripts rust" "$(areas scripts/dev-stack.sh)"
check "the dev stack's test" "ci_scripts" "$(areas scripts/dev-stack-test.sh)"
check "the line cap feeds the Rust jobs and its test" "ci_scripts rust" "$(areas scripts/check-file-loc.sh)"
check "a crate README reads nothing" "" "$(areas crates/trader/README.md)"
check "a console file" "console" "$(areas console/src/App.tsx console/package-lock.json)"
check "the README, SECURITY.md and LICENSE feed the link checker" "agent_docs" \
  "$(areas README.md SECURITY.md LICENSE)"
check "the agent docs and the link checker" "agent_docs" \
  "$(areas agents/AGENTS.md agents/rules/git-workflow.md scripts/check-agent-links.sh scripts/check-agent-links-test.sh)"
check "the review-delta scripts feed their test and the link checker" "agent_docs agent_scripts" \
  "$(areas agents/skills/agent-code-review/scripts/review-delta.sh)"
check "the merge gate" "merge_approval" \
  "$(areas scripts/check-merge-approved.sh scripts/check-merge-approved-test.sh)"
check "the CI scripts and the line cap's test" "ci_scripts" \
  "$(areas scripts/ci-areas.sh scripts/check-ci-passed.sh scripts/ci-test.sh scripts/check-file-loc-test.sh)"
check "several areas" "console rust" "$(areas crates/trader/src/main.rs console/index.html)"
check "a workflow change selects every area" "$every" "$(areas .github/workflows/ci.yaml)"
check "an unlisted path selects every area" "$every" "$(areas .gitignore)"
check "an unlisted path among listed ones selects every area" "$every" "$(areas README.md .gitignore)"
check "an empty diff selects every area" "$every" "$(areas '')"
check "--all lists every area" "$every" "$(./scripts/ci-areas.sh --all | tr ' ' '\n' | sort | tr '\n' ' ' | sed 's/ $//')"

# predicate <changes-outputs-json> <jobs-json> [changes-result] : the predicate's exit status.
predicate() {
  local needs
  needs=$(jq -cn --argjson outputs "$1" --argjson jobs "$2" --arg result "${3:-success}" \
    '{changes: {result: $result, outputs: $outputs}} + ($jobs | map_values({result: .}))')
  if NEEDS=$needs ./scripts/check-ci-passed.sh >/dev/null 2>&1; then echo pass; else echo fail; fi
}
none='{"rust":"false","console":"false"}'
rust='{"rust":"true","console":"false"}'
ok='{"fmt":"success","console":"success"}'
# Each rejection case leaves one cause: every other job succeeded, and the classifier's outputs
# would admit a skip, so a predicate that ignores the cause under test passes the fixture.
check "every job succeeded" pass "$(predicate "$rust" '{"fmt":"success","console":"skipped"}')"
check "a job skipped with its area false" pass "$(predicate "$none" '{"fmt":"skipped","console":"skipped"}')"
check "a job skipped with its area true" fail "$(predicate "$rust" '{"fmt":"skipped","console":"success"}')"
check "a failed job" fail "$(predicate "$none" '{"fmt":"failure","console":"success"}')"
check "a cancelled job" fail "$(predicate "$none" '{"fmt":"cancelled","console":"success"}')"
check "a skipped job missing from the area map" fail "$(predicate "$none" '{"fmt":"success","unknown-job":"skipped"}')"
check "a skipped job when the classifier gave no outputs" fail "$(predicate '{}' '{"fmt":"skipped","console":"success"}')"
check "a classifier that succeeded with every other job" pass "$(predicate "$none" "$ok")"
check "a failed classifier" fail "$(predicate "$none" "$ok" failure)"
check "a cancelled classifier" fail "$(predicate "$none" "$ok" cancelled)"
check "a skipped classifier" fail "$(predicate "$none" "$ok" skipped)"

# Every job but the classifier, the aggregate and the cancel companions is gated by its own area:
# it is in ci-passed's needs, the predicate's map names the area its `if` reads, and it has a
# cancel-<job> companion.
needs=$(yq -r '.jobs."ci-passed".needs[]' "$workflow" | sort | tr '\n' ' ')
jobs=$(yq -r '.jobs | keys | .[]' "$workflow" | grep -v '^cancel-' | grep -vx 'ci-passed' | sort | tr '\n' ' ')
check "ci-passed needs every job" "$jobs" "$needs"
for job in $(yq -r '.jobs | keys | .[]' "$workflow" | grep -vx -e changes -e ci-passed -e '^cancel-.*'); do
  gate=$(yq -r ".jobs.\"$job\".if" "$workflow" | grep -o "outputs\.[a-z_]*" | head -1 | cut -d. -f2)
  mapped=$(NEEDS="{\"changes\":{\"result\":\"success\",\"outputs\":{\"$gate\":\"false\"}},\"$job\":{\"result\":\"skipped\"}}" \
    ./scripts/check-ci-passed.sh >/dev/null 2>&1 && echo yes || echo no)
  check "$job may skip only when its area ($gate) is false" yes "$mapped"
  has_companion=$(yq -r ".jobs.\"cancel-$job\".needs" "$workflow")
  check "$job has a cancel companion" "$job" "$has_companion"
done
exit "$fail"
