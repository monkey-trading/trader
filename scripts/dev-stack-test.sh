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
exit "$fail"
