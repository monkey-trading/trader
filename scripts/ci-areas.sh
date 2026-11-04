#!/usr/bin/env bash
# Sorts changed files into the CI areas whose jobs read them. Reads one path per line on stdin and
# prints the areas to run, space separated. A path the list does not name selects every area, and so
# does an empty line, which is what an empty diff gives. `--all` prints every area.
# `.github/workflows/ci.yaml` runs it in its `changes` job; `scripts/ci-test.sh` tests it.
set -euo pipefail

all="rust console agent_docs agent_scripts merge_approval ci_scripts"
if [[ ${1:-} == --all ]]; then
  echo "$all"
  exit 0
fi

areas=
while IFS= read -r file; do
  case "$file" in
    # The review-delta helper and its test; Markdown links to them are checked too.
    agents/skills/agent-code-review/scripts/*) areas+=" agent_scripts agent_docs" ;;
    # The merge queue's approval gate and its test.
    scripts/check-merge-approved.sh | scripts/check-merge-approved-test.sh) areas+=" merge_approval" ;;
    # This classifier, the ci-passed predicate, and their test; the line cap's test.
    scripts/ci-areas.sh | scripts/check-ci-passed.sh | scripts/ci-test.sh | scripts/check-file-loc-test.sh \
      | scripts/dev-stack-test.sh)
      areas+=" ci_scripts" ;;
    # The line cap is read by the Rust jobs, and its test by the CI scripts job.
    scripts/check-file-loc.sh) areas+=" rust ci_scripts" ;;
    # The link checker, its test, the Markdown it reads, and the LICENSE the README links.
    agents/* | AGENTS.md | CLAUDE.md | README.md | SECURITY.md | LICENSE | scripts/check-agent-links.sh \
      | scripts/check-agent-links-test.sh)
      areas+=" agent_docs" ;;
    # Read by no build, test, or script.
    crates/*/README.md) ;;
    # The dev stack is read by the dev scenarios and tested by the CI scripts job.
    scripts/dev-stack.sh) areas+=" rust ci_scripts" ;;
    # The Rust jobs, and the dev scenarios and the example config the tests load.
    crates/* | Cargo.toml | Cargo.lock | rust-toolchain.toml | rustfmt.toml | deny.toml | config/* \
      | scripts/dev-trade-one.sh | scripts/dev-console-api.sh)
      areas+=" rust" ;;
    console/*) areas+=" console" ;;
    *)
      echo "$all"
      exit 0
      ;;
  esac
done
echo "${areas# }"
