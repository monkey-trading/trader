#!/usr/bin/env bash
# Tests scripts/dev-stack.sh without an engine build: a scratch copy of the script beside a
# Cargo.lock that pins a scratch git repository's one commit, a cached checkout of it, and a cargo
# stand-in that writes the binaries into the target directory it is given, from the checkout, as
# cargo does. Checks that a shell that only sources the script knows where the binaries are, and
# that the binaries are copied from the directory cargo built into, whether CARGO_TARGET_DIR is
# relative, absolute or unset. Prints one line per case and exits 1 if any case fails. Needs git.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
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

git="git -c user.name=test -c user.email=test@example.com"
mkdir -p "$scratch/engine/config" "$scratch/repo/scripts"
echo 'max_drift = "1s"' > "$scratch/engine/config/dev.toml"
$git -C "$scratch/engine" init -q
$git -C "$scratch/engine" add .
$git -C "$scratch/engine" commit -q -m engine
commit="$($git -C "$scratch/engine" rev-parse HEAD)"
echo "source = \"git+https://github.com/monkey-trading/engine?rev=$commit#$commit\"" > "$scratch/repo/Cargo.lock"
cp "$here/rust-toolchain.toml" "$scratch/repo/"
cp "$here/scripts/dev-stack.sh" "$scratch/repo/scripts/"

# in_stack <script>: runs bash `script` in a fresh shell that has sourced the scratch copy of dev-stack.sh.
in_stack() {
  TRADER_ENGINE_CACHE="$scratch/cache-default" bash -c 'set -Eeuo pipefail; source "$1/scripts/dev-stack.sh"; eval "$2"' _ \
    "$scratch/repo" "$1" 2>&1
}

cat > "$scratch/cargo" <<'CARGO'
#!/usr/bin/env bash
dir="${CARGO_TARGET_DIR:-target}/debug"
mkdir -p "$dir/examples"
touch "$dir/engine" "$dir/examples/access_source" "$dir/examples/sign_request"
chmod +x "$dir/engine" "$dir/examples/access_source" "$dir/examples/sign_request"
CARGO
chmod +x "$scratch/cargo"

# build <cache> [CARGO_TARGET_DIR]: builds in a fresh shell started away from the checkout, as a
# caller would, and prints what the build left in the cache's dist directory.
build() {
  local cache="$1"
  shift
  mkdir -p "$cache/$commit"
  $git clone -q "$scratch/engine" "$cache/$commit/src"
  (
    cd "$scratch"
    env "$@" TRADER_ENGINE_CACHE="$cache" DEV_STACK_CARGO="$scratch/cargo" \
      bash -c 'source "$1/scripts/dev-stack.sh" && dev_stack_build > /dev/null' _ "$scratch/repo"
  ) > "$scratch/build.log" 2>&1 || cat "$scratch/build.log"
  (cd "$cache/$commit/dist" 2>/dev/null && ls | tr '\n' ' ') || true
}

built="access_source dev.toml engine sign_request "
check "a relative CARGO_TARGET_DIR" "$built" "$(build "$scratch/cache-relative" CARGO_TARGET_DIR=target/shared)"
check "an absolute CARGO_TARGET_DIR" "$built" "$(build "$scratch/cache-absolute" CARGO_TARGET_DIR="$scratch/shared")"
check "no CARGO_TARGET_DIR" "$built" "$(build "$scratch/cache-default" -u CARGO_TARGET_DIR)"

# A shell that only sources the script, as the printed funding steps say, finds the binaries
# dev_stack_declare signs with.
dist="$(TRADER_ENGINE_CACHE="$scratch/cache-default" bash -c \
  'set -u; source "$1/scripts/dev-stack.sh"; echo "$dev_stack_dist"' _ "$scratch/repo")"
check "sourcing sets dev_stack_dist" "$scratch/cache-default/$commit/dist" "$dist"
check "the signing tool is in dev_stack_dist" "yes" "$([[ -x "$dist/sign_request" ]] && echo yes || echo no)"

# The port picker never hands out a port twice in a run, though each pick is a subshell, and keeps to
# 20000-29999 and off the demo's ports.
picked="$(in_stack 'for _ in $(seq 1 400); do dev_stack_free_port; done')"
check "400 picks" "400" "$(wc -l <<< "$picked" | tr -d ' ')"
check "no port picked twice" "400" "$(sort -u <<< "$picked" | wc -l | tr -d ' ')"
check "ports in range" "0" "$(awk '$1 < 20000 || $1 > 29999 || $1 == 3000 || $1 == 8080 || $1 == 8081 || $1 == 8545 { n++ } END { print n + 0 }' <<< "$picked")"
check "the record lists every pick" "400" "$(in_stack 'for _ in $(seq 1 400); do dev_stack_free_port > /dev/null; done; wc -l < "$dev_stack_ports_file" | tr -d " "')"
check "stop keeps the record on --keep-ports only" "kept gone" "$(in_stack '
  dev_stack_free_port > /dev/null
  dev_stack_stop --keep-ports; [[ -s "$dev_stack_ports_file" ]] && printf "kept "
  dev_stack_stop; [[ -e "$dev_stack_ports_file" ]] || printf "gone"')"

# What the node says when it loses its port is recognised, and nothing else is.
check "a lost port is recognised" "yes" "$(in_stack 'printf "Error: binding 127.0.0.1:38239\n    Address already in use (os error 98)\n" > "$dev_stack_ports_file.log"
  dev_stack_lost_port "$dev_stack_ports_file.log" && echo yes || echo no')"
check "another failure is not" "no" "$(in_stack 'printf "Error: reading the config\n" > "$dev_stack_ports_file.log"
  dev_stack_lost_port "$dev_stack_ports_file.log" && echo yes || echo no')"

# A start that loses its port is run again on a new one; one that loses it every time fails.
check "a lost port is retried on a new port" "3 3 distinct" "$(in_stack '
  tries=0; ports=""
  step() { tries=$((tries + 1)); p="$(dev_stack_free_port)"; ports="$ports $p"; (( tries >= 3 )) || dev_stack_port_lost=1; }
  dev_stack_attempts step 2> /dev/null
  echo "$tries $(printf "%s\n" $ports | sort -u | wc -l | tr -d " ") distinct"')"
check "a start that always loses its port fails" \
  "FAIL: a port picked for step was taken by another process on each of 5 attempts" \
  "$(in_stack 'step() { dev_stack_port_lost=1; }; dev_stack_attempts step' | grep '^FAIL' || true)"
exit "$fail"
