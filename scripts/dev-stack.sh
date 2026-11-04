#!/usr/bin/env bash
# A local monkeytrading node for the bot: the engine commit Cargo.lock pins, fetched and built once
# into a cache outside the repository, run on that commit's config/dev.toml (mock TEE, custody and
# feeds) with every port moved to a free one, on private temporary data, and with the engine's
# synthetic signed access source, which clears every client. Prints the API URL, the enclave signer
# and the TLS key hash a bot config needs (see config/example.toml).
#
# Run it to keep a node up until interrupted, or source it for the functions a scenario uses:
# dev_stack_start, dev_stack_stop, dev_stack_node_config, dev_stack_fund, dev_stack_declare and
# dev_stack_admin.
#
# Needs git, cargo, python3, curl and jq, and the public Roughtime servers the engine's dev config
# names. Environment:
#   TRADER_ENGINE_CACHE  where engine commits and their built binaries are kept
#                        (default ${XDG_CACHE_HOME:-$HOME/.cache}/monkeytrading-trader/engine)
#   DEV_STACK_CARGO      the command that runs cargo for the engine build (default: cargo); a
#                        wrapper such as a build lock goes here
#   CARGO_TARGET_DIR     where cargo builds the engine (default: the cached checkout's target/)
#   SCENARIO_ARTIFACTS   a directory that keeps the node's config and logs after it stops
set -Eeuo pipefail

dev_stack_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# The custody deployment config/dev.toml names, and the access source's key: Anvil's published
# account 0, which the engine's own scenarios sign synthetic screenings with.
dev_stack_chain_id=31337
dev_stack_vault="0x0000000000000000000000000000000000000000"
dev_stack_source="0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
# The dev config's operator key, whose private key the engine publishes for local nodes.
dev_stack_operator_key="4242424242424242424242424242424242424242424242424242424242424242"
dev_stack_node_pid=""
dev_stack_source_pid=""

dev_stack_fail() {
  echo "FAIL: $*" >&2
  [[ -n "${dev_stack_work:-}" && -f "$dev_stack_work/node.log" ]] && tail -n 40 "$dev_stack_work/node.log" >&2
  exit 1
}

# The one engine commit Cargo.lock records for the engine's crates.
dev_stack_engine_commit() {
  local commits
  commits="$(grep -o 'git+https://github.com/monkey-trading/engine?rev=[0-9a-f]*#[0-9a-f]*' "$dev_stack_root/Cargo.lock" \
    | sed 's/.*#//' | sort -u)"
  [[ "$commits" =~ ^[0-9a-f]{40}$ ]] || dev_stack_fail "Cargo.lock must pin the engine to one commit, found: ${commits:-none}"
  echo "$commits"
}

# Fetches the pinned commit and builds the node and the scenario tools once per commit, into
# $dev_stack_dist with the commit's dev config: all a dev node needs, so caching that directory alone
# by commit (as CI does) is enough to run one.
dev_stack_build() {
  local commit cache channel cargo_cmd target
  commit="$(dev_stack_engine_commit)"
  cache="${TRADER_ENGINE_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/monkeytrading-trader/engine}/$commit"
  dev_stack_dist="$cache/dist"
  if [[ -x "$dev_stack_dist/engine" && -x "$dev_stack_dist/access_source" && -x "$dev_stack_dist/sign_request" \
    && -f "$dev_stack_dist/dev.toml" ]]; then
    echo "==> engine $commit: built"
    return 0
  fi
  if [[ ! -d "$cache/src/.git" ]]; then
    echo "==> engine $commit: fetching"
    rm -rf "$cache/src.tmp"
    git init -q "$cache/src.tmp"
    git -C "$cache/src.tmp" fetch -q --depth 1 https://github.com/monkey-trading/engine "$commit"
    git -C "$cache/src.tmp" checkout -q --detach FETCH_HEAD
    mv "$cache/src.tmp" "$cache/src"
  fi
  [[ "$(git -C "$cache/src" rev-parse HEAD)" == "$commit" ]] || dev_stack_fail "$cache/src is not at $commit"
  echo "==> engine $commit: building"
  # The engine has no toolchain file; it builds with this repository's, as its crates here do.
  channel="$(sed -n 's/^channel = "\(.*\)"/\1/p' "$dev_stack_root/rust-toolchain.toml")"
  read -r -a cargo_cmd <<< "${DEV_STACK_CARGO:-cargo}"
  (
    cd "$cache/src"
    export RUSTUP_TOOLCHAIN="$channel"
    "${cargo_cmd[@]}" build -q --locked -p engine-node --bin engine --example access_source
    "${cargo_cmd[@]}" build -q --locked -p engine-core --example sign_request
  )
  target="${CARGO_TARGET_DIR:-$cache/src/target}"
  rm -rf "$dev_stack_dist.tmp"
  mkdir -p "$dev_stack_dist.tmp"
  cp "$target/debug/engine" "$target/debug/examples/access_source" "$target/debug/examples/sign_request" \
    "$cache/src/config/dev.toml" "$dev_stack_dist.tmp/"
  rm -rf "$dev_stack_dist"
  mv "$dev_stack_dist.tmp" "$dev_stack_dist"
  # The binaries are kept per commit, so a private build directory is not needed again.
  [[ -n "${CARGO_TARGET_DIR:-}" ]] || rm -rf "$cache/src/target"
}

# A free loopback port, never one a local demo uses.
dev_stack_free_port() {
  local port
  while :; do
    port="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
    case "$port" in 3000 | 8080 | 8081 | 8545 | 9464 | 9465) ;; *) echo "$port"; return ;; esac
  done
}

# Starts the access source and the node on fresh ports and data, waits until the node sequences
# and prices BTC-USD, and sets dev_stack_api, dev_stack_signer, dev_stack_pin and dev_stack_work.
dev_stack_start() {
  local api_port metrics_port source_port max_drift wait_s dev_toml attestation
  dev_stack_build
  dev_stack_work="$(mktemp -d)"
  api_port="$(dev_stack_free_port)"
  metrics_port="$(dev_stack_free_port)"
  source_port="$(dev_stack_free_port)"
  dev_stack_api="https://127.0.0.1:$api_port"

  printf '{"mode":"clear","jurisdictions":{},"sanctioned":[]}\n' > "$dev_stack_work/access-fixture.json"
  "$dev_stack_dist/access_source" "127.0.0.1:$source_port" "$dev_stack_chain_id" "$dev_stack_vault" \
    "$dev_stack_work/access-fixture.json" > "$dev_stack_work/access-source.log" 2>&1 &
  dev_stack_source_pid=$!

  dev_toml="$dev_stack_dist/dev.toml"
  sed -e "s|127.0.0.1:8080|127.0.0.1:$api_port|" \
    -e "s|127.0.0.1:9464|127.0.0.1:$metrics_port|" \
    -e "s|data/dev|$dev_stack_work|g" \
    "$dev_toml" > "$dev_stack_work/node.toml"
  cat >> "$dev_stack_work/node.toml" <<CONFIG

[access]
source = "$dev_stack_source"
endpoint = "http://127.0.0.1:$source_port"
timeout = "2s"
max_age = "30s"
max_response_bytes = 4096
max_in_flight = 8
CONFIG
  for edit in "listen = \"127.0.0.1:$api_port\"" "listen = \"127.0.0.1:$metrics_port\"" "log_dir = \"$dev_stack_work/log\""; do
    grep -qF "$edit" "$dev_stack_work/node.toml" || dev_stack_fail "config edit missing: $edit"
  done
  if grep -nE '127\.0\.0\.1:(3000|8080|8081|8545|9464|9465)\b' "$dev_stack_work/node.toml"; then
    dev_stack_fail "the node config still names a port the local demo uses"
  fi

  "$dev_stack_dist/engine" run --config "$dev_stack_work/node.toml" > "$dev_stack_work/node.log" 2>&1 &
  dev_stack_node_pid=$!
  # The node stamps nothing before its first trusted time sample, which can take a Roughtime round.
  max_drift="$(awk -F'"' '/^max_drift = / { print $2 }' "$dev_toml")"
  wait_s=$(( ${max_drift%s} + 30 ))
  for _ in $(seq 1 $(( wait_s * 5 ))); do
    kill -0 "$dev_stack_node_pid" 2>/dev/null || dev_stack_fail "the node exited"
    if [[ "$(curl -sk --max-time 2 "$dev_stack_api/v1/markets" 2>/dev/null \
      | jq -r '[.[]? | select(.symbol == "BTC-USD" and .mark != null)] | length' 2>/dev/null || true)" == 1 ]]; then
      break
    fi
    sleep 0.2
  done
  grep -q 'first trusted time sample: sequencing started' "$dev_stack_work/node.log" \
    || dev_stack_fail "the node never started sequencing"
  [[ "$(curl -sk --max-time 2 "$dev_stack_api/v1/markets" | jq -r '[.[]? | select(.symbol == "BTC-USD" and .mark != null)] | length' \
    2>/dev/null || true)" == 1 ]] || dev_stack_fail "BTC-USD has no mark"
  attestation="$(curl -sfk --max-time 5 "$dev_stack_api/v1/attestation")" || dev_stack_fail "no attestation"
  dev_stack_signer="$(jq -r .enclave_address <<< "$attestation")"
  dev_stack_pin="$(jq -r '.tls_public_key_sha256 | ltrimstr("0x")' <<< "$attestation")"
  [[ "$dev_stack_signer" =~ ^0x[0-9a-fA-F]{40}$ && "$dev_stack_pin" =~ ^[0-9a-f]{64}$ ]] \
    || dev_stack_fail "the attestation names no signer or TLS key hash"
}

# Stops the node and the access source, keeps their logs under SCENARIO_ARTIFACTS, and removes the
# node's data.
dev_stack_stop() {
  local pid
  for pid in "$dev_stack_node_pid" "$dev_stack_source_pid"; do
    if [[ -n "$pid" ]]; then kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; fi
  done
  dev_stack_node_pid=""
  dev_stack_source_pid=""
  if [[ -n "${dev_stack_work:-}" ]]; then
    if [[ -n "${SCENARIO_ARTIFACTS:-}" ]]; then
      mkdir -p "$SCENARIO_ARTIFACTS"
      cp "$dev_stack_work"/*.log "$dev_stack_work"/*.toml "$SCENARIO_ARTIFACTS/" 2>/dev/null || true
    fi
    rm -rf "$dev_stack_work"
  fi
}

# Prints the [node] section of a bot config for this node.
dev_stack_node_config() {
  cat <<CONFIG
[node]
url = "$dev_stack_api"
chain_id = $dev_stack_chain_id
vault = "$dev_stack_vault"
signer = "$dev_stack_signer"
tls_key_sha256 = "$dev_stack_pin"
CONFIG
}

# Signs and submits an admin command as the dev config's operator, such as
# `init-emissions --start <time> --period-budget <units>`.
dev_stack_admin() {
  OPERATOR_KEY="$dev_stack_operator_key" "$dev_stack_dist/engine" admin --config "$dev_stack_work/node.toml" \
    --node "$dev_stack_api" --pin "$dev_stack_pin" "$@" > /dev/null
}

# Credits `$1` with `$2` USD through the mock custody's dev deposit; the credit waits for the
# account's declaration.
dev_stack_fund() {
  curl -sfk --max-time 10 "$dev_stack_api/v1/dev/deposit" -H 'content-type: application/json' \
    -d "{\"account\":\"$1\",\"amount\":\"$2\"}" > /dev/null || dev_stack_fail "the dev deposit for $1 failed"
}

# Records the professional-client declaration of the account whose hex private key is `$1`, as the
# account signs it, through the access source; `$2` is its address.
dev_stack_declare() {
  local key="$1" account="$2" nonce expires_at body events
  nonce="$(curl -sfk --max-time 5 "$dev_stack_api/v1/accounts/$account/nonce" | jq -er .next_nonce)"
  expires_at="$(python3 -c 'import time; print(int(time.time() * 1000) + 60000)')"
  body="$(SIGNER_KEY="${key#0x}" "$dev_stack_dist/sign_request" "$dev_stack_chain_id" "$dev_stack_vault" "$nonce" \
    "$expires_at" eligibility "{\"account\":\"$account\",\"class\":\"professional\"}")"
  events="$(curl -sfk --max-time 10 "$dev_stack_api/v1/eligibility" -H 'content-type: application/json' -d "$body")" \
    || dev_stack_fail "the declaration for $account was refused"
  jq -e 'any(.[]; .type == "eligibility_attested")' <<< "$events" > /dev/null \
    || dev_stack_fail "the declaration for $account was not recorded: $events"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  trap dev_stack_stop EXIT
  trap 'exit 130' INT TERM
  dev_stack_start
  echo "==> a dev node is up; its [node] config:"
  dev_stack_node_config
  echo "==> fund and declare an account: source this script and call dev_stack_fund <address> <amount> and"
  echo "    dev_stack_declare <hex key> <address> with dev_stack_api=$dev_stack_api; the deposit credits once declared"
  echo "==> the node's log: $dev_stack_work/node.log; interrupt to stop"
  wait "$dev_stack_node_pid"
fi
