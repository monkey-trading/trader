#!/usr/bin/env bash
# Checks review-delta.sh on a scratch repository: an unrelated change on main, a changed line, a
# line moved between methods, binary fixtures (also under a text conversion), a caller in a
# subdirectory under an inherited diff.relative, bad arguments, and failures. Prints one line per
# case and exits 1 if any case fails.
set -euo pipefail
script="$(cd "$(dirname "$0")" && pwd)/review-delta.sh"
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
cd "$tmp"
git init -q . && git config user.name test && git config user.email test@example.com

strategy() { # the method that sizes a position
  mkdir -p crates/core/src
  {
    echo "impl Sma {"
    echo "    pub fn target(&mut self) {"
    echo "        self.common();"
    [ "$1" != target ] || echo "        self.apply_limits();"
    echo "    }"
    echo "    pub fn flatten(&mut self) {"
    echo "        self.common();"
    [ "$1" != flatten ] || echo "        self.apply_limits();"
    echo "    }"
    echo "}"
  } > crates/core/src/sma.rs
}
other() { mkdir -p crates/trader/src && echo "fn main() { $1 }" > crates/trader/src/main.rs; }
commit() { git add -A && git commit -qm "$1" && git rev-parse HEAD; }
from() { git checkout -q --detach "$1"; }

strategy none; other base; base=$(commit base)
strategy target; reviewed=$(commit reviewed)
strategy flatten; moved=$(commit moved)
from "$reviewed"; strategy target; echo "// extra" >> crates/core/src/sma.rs; changed=$(commit changed)
from "$base"; other main; main=$(commit main)
strategy target; rebased=$(commit rebased)
from "$base"; printf '\0\1AAAA' > blob.bin; blob_a=$(commit blob-a)
from "$base"; printf '\0\1BBBB' > blob.bin; blob_b=$(commit blob-b)
from "$main"; printf '\0\1AAAA' > blob.bin; blob_a_on_main=$(commit blob-a-on-main)

failed=0
check() { # name expected-exit args...
  local name=$1 want=$2 got=0; shift 2
  "$script" "$@" > "$name.out" 2> "$name.err" || got=$?
  if [ "$got" = "$want" ]; then echo "ok   $name"; return; fi
  echo "FAIL $name: exit $got, want $want"; failed=1
}
check identical 0 "$base" "$reviewed" "$base" "$reviewed"
check rebase-onto-changed-main 0 "$base" "$reviewed" "$main" "$rebased"
# A base that is not the merge-base: main moved on after the branch forked, with no rebase.
check unrebased-against-advanced-main 0 "$base" "$reviewed" "$main" "$reviewed"
check unrebased-reviewed-base-advanced 0 "$main" "$rebased" "$main" "$rebased"
check unrebased-new-patch-changed 1 "$base" "$reviewed" "$main" "$changed"
check line-moved-between-methods 1 "$base" "$reviewed" "$base" "$moved"
check line-added 1 "$base" "$reviewed" "$base" "$changed"
grep -q '^++// extra$' line-added.out ||
  { echo "FAIL line-added: the added line is not in the residual"; failed=1; }
check binary-changed 1 "$base" "$blob_a" "$base" "$blob_b"
check binary-same 0 "$base" "$blob_a" "$main" "$blob_a_on_main"
# A text conversion that renders both blobs alike must not hide the change.
echo '*.bin diff=alike' > .git/info/attributes
git config diff.alike.textconv "sh -c 'echo alike'"
check binary-changed-under-textconv 1 "$base" "$blob_a" "$base" "$blob_b"
check bad-ref 3 "$base" no-such-ref "$base" "$reviewed"
check too-few-arguments 2 "$base" "$reviewed"
if grep -q "set -Eeuo" too-few-arguments.out; then echo "FAIL too-few-arguments: usage runs past the header"; failed=1; fi
# An inherited diff.relative must not shrink the patches to the caller's directory.
git config diff.relative true
cd crates/core
check relative-binary-changed 1 "$base" "$blob_a" "$base" "$blob_b"
check relative-rebase 0 "$base" "$reviewed" "$main" "$rebased"
cd "$tmp"
git config diff.relative false
exit "$failed"
