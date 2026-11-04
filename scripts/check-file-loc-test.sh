#!/usr/bin/env bash
# Tests scripts/check-file-loc.sh against a scratch copy of the repository layout: for each opted-in
# crate, a file of exactly its cap (500) is accepted and one line more is rejected with a non-zero
# status and its path named, with the success line absent; a nested file counts, a file under
# `target/` and a file that is not Rust do not, and a rejection in one crate names only that
# crate's file. Prints one line per case and exits 1 if
# any case fails. Reads each crate and its cap from the script's `check_pkg` lines, and fails on a
# line that carries exemptions until a case for them is added. Needs only bash and coreutils.
set -euo pipefail
script="$(cd "$(dirname "$0")" && pwd)/check-file-loc.sh"
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
crates=$(sed -n 's/^check_pkg \([^ ]*\) [0-9]*$/\1/p' "$script")
fail=0
[[ -n "$crates" ]] || { echo "FAIL no opted-in crates found in check-file-loc.sh"; exit 1; }
if grep -q '^check_pkg [^ ]* [0-9]* ' "$script"; then
  echo "FAIL check-file-loc.sh exempts a path; add a case for it to this test"; fail=1
fi
# cap <crate> : the crate's line cap.
cap() { sed -n "s|^check_pkg $1 \([0-9]*\)\$|\1|p" "$script"; }

# scratch <name> : a fresh repository holding a copy of the script and a file at its cap in every crate.
scratch() {
  root="$tmp/$1"
  mkdir -p "$root/scripts"
  cp "$script" "$root/scripts/check-file-loc.sh"
  local c
  for c in $crates; do lines "$root/$c/src/within.rs" "$(cap "$c")"; done
}
# lines <path> <count> : a Rust file of exactly <count> lines.
lines() {
  mkdir -p "$(dirname "$1")"
  seq "$2" | sed 's/.*/\/\/ line/' > "$1"
}
# run : the script's output and status in the current scratch repository, as "status|output".
run() {
  local out status=0
  out=$("$root/scripts/check-file-loc.sh" 2>&1) || status=$?
  echo "$status|$out"
}
# expect <name> <status> <result> [want-substring...] [-- absent-substring...]
expect() {
  local name=$1 want=$2 result=$3 ok=1 mode=want
  shift 3
  [[ "${result%%|*}" == "$want" ]] || ok=0
  for needle; do
    if [[ $needle == -- ]]; then mode=absent; continue; fi
    if [[ $mode == want ]]; then [[ "$result" == *"$needle"* ]] || ok=0; else [[ "$result" != *"$needle"* ]] || ok=0; fi
  done
  if ((ok)); then
    echo "ok   $name"
  else
    echo "FAIL $name: wanted status $want and the listed output, got:"; echo "${result//$'\n'/$'\n'     }"; fail=1
  fi
}

done_line="All checked Rust files are within their line caps."
scratch boundary
expect "files of exactly their cap pass in every crate" 0 "$(run)" "$done_line"

for crate in $crates; do
  scratch "over-$crate"
  lines "$root/$crate/src/big.rs" $(($(cap "$crate") + 1))
  result=$(run)
  expect "a file one line over $crate's cap is rejected and named" 1 "$result" "$crate/src/big.rs" "over $(cap "$crate") lines" -- "$done_line"
  for other in $crates; do
    [[ $other == "$crate" ]] || expect "…and $other's file at its cap is not named" 1 "$result" -- "$other/src/within.rs"
  done
done

crate=${crates%%$'\n'*} over=$(($(cap "${crates%%$'\n'*}") + 1))
scratch nested
lines "$root/$crate/src/a/b/deep.rs" "$over"
expect "a nested file over the cap is rejected" 1 "$(run)" "$crate/src/a/b/deep.rs" -- "$done_line"

scratch target
lines "$root/$crate/target/debug/generated.rs" "$over"
expect "a file over the cap under target/ is ignored" 0 "$(run)" "$done_line"

scratch non-rust
lines "$root/$crate/src/notes.txt" "$over"
expect "a file over the cap that is not Rust is ignored" 0 "$(run)" "$done_line"
exit "$fail"
