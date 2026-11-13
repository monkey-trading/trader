#!/usr/bin/env bash
# A local monkeytrading node for the bot: the engine commit Cargo.lock pins, fetched and built once
# into a cache outside the repository, run on that commit's config/dev.toml (mock TEE, custody and
# feeds) with every port moved to a free one, on private temporary data, and with the engine's
# synthetic signed access source, which clears every client. Prints the API URL, the enclave signer
# and the TLS key hash a bot config needs (see config/example.toml).
#
# With --chain it also starts Anvil, deploys the vault and a mock USDC from the engine's contracts/ at
# the same commit, and runs the node on that commit's config/anvil.toml (EVM custody, three local RPC
# proxies, checkpoints posted to the vault), as the engine's dev-monkey-emissions.sh does: a bot can
# deposit USDC, claim MONKEY with Vault.claimEmissions and withdraw with Vault.withdraw. Mock custody's
# dev_stack_fund does not work there; use dev_stack_wallet and a vault deposit.
#
# Run it (`dev-stack.sh [--chain]`) to keep a node up until interrupted, or source it for the
# functions a scenario uses: dev_stack_start [--chain], dev_stack_stop, dev_stack_node_config,
# dev_stack_chain_config, dev_stack_fund, dev_stack_declare, dev_stack_admin and, with --chain,
# dev_stack_wallet. With --chain the deployer funds the house with 5,000,000 USDC through the vault
# before the start returns. After a start these are set: dev_stack_api, dev_stack_signer,
# dev_stack_pin, dev_stack_work and, with --chain, dev_stack_rpc (Anvil's URL), dev_stack_vault,
# dev_stack_usdc and dev_stack_monkey (the vault's MONKEY token).
#
# Needs git, cargo, python3, curl and jq, and the public Roughtime servers the engine's dev config
# names. --chain also needs anvil and cast at run time, and forge once per engine commit (to build
# the contracts into the cache; network for `forge soldeer install` and the solc download).
# Environment:
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
dev_stack_anvil_pid=""
dev_stack_proxy_pids=()
dev_stack_proxy_ports=()
dev_stack_rpc=""
dev_stack_usdc=""
dev_stack_monkey=""
dev_stack_chain=0
# Anvil's published account 0 deploys the contracts and mints the mock USDC; account 9 relays
# checkpoints, which the engine's own scenarios leave to the relayer too.
dev_stack_deployer_key="ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
dev_stack_relayer="0xa0Ee7A142d267C1f36714E4a8F75612F20a79720"
# The mock enclave signer anvil.toml's tee.use_published_mock_key gives the node (32 bytes of 0x43).
dev_stack_mock_signer="0x5975c152fE58cDcB7E25586A3c9b994A16dbB615"
# The MONKEY period budget the vault is deployed with, which the node's emission schedule must match:
# 1,000 MONKEY.
dev_stack_period_budget="1000000000000000000000"
# The house's capital, in USDC, which the deployer deposits through the vault.
dev_stack_house_usdc=5000000

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

# Sets dev_stack_cache, the directory that keeps the pinned commit's checkout, and dev_stack_dist,
# the directory that keeps its built binaries. Sourcing this script runs it, so a shell that only
# sources the script can call dev_stack_declare and dev_stack_admin without building.
dev_stack_locate() {
  local commit
  commit="$(dev_stack_engine_commit)"
  dev_stack_commit="$commit"
  dev_stack_cache="${TRADER_ENGINE_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/monkeytrading-trader/engine}/$commit"
  dev_stack_dist="$dev_stack_cache/dist"
}

# Fetches the pinned commit and builds the node and the scenario tools once per commit, into
# $dev_stack_dist with the commit's dev config: all a dev node needs, so caching that directory alone
# by commit (as CI does) is enough to run one. With --chain it also builds the contracts into
# $dev_stack_dist/contracts, with the commit's anvil.toml and RPC proxy beside them, so a chain stack
# needs only anvil, cast and python3 at run time; a dist/ that already holds the binaries builds just
# those.
dev_stack_build() {
  local commit cache channel cargo_cmd target
  dev_stack_locate
  commit="$dev_stack_commit"
  cache="$dev_stack_cache"
  if dev_stack_built_node && { [[ "${1:-}" != --chain ]] || dev_stack_built_contracts; }; then
    echo "==> engine $commit: built"
    [[ "${1:-}" != --chain ]] || dev_stack_check_abi
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
  if ! dev_stack_built_node; then
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
    # Cargo ran in the checkout, so it read a relative CARGO_TARGET_DIR from there.
    target="${CARGO_TARGET_DIR:-target}"
    [[ "$target" == /* ]] || target="$cache/src/$target"
    rm -rf "$dev_stack_dist.tmp"
    mkdir -p "$dev_stack_dist.tmp"
    cp "$target/debug/engine" "$target/debug/examples/access_source" "$target/debug/examples/sign_request" \
      "$cache/src/config/dev.toml" "$dev_stack_dist.tmp/"
    rm -rf "$dev_stack_dist"
    mv "$dev_stack_dist.tmp" "$dev_stack_dist"
    # The binaries are kept per commit, so a private build directory is not needed again.
    [[ -n "${CARGO_TARGET_DIR:-}" ]] || rm -rf "$cache/src/target"
  fi
  if [[ "${1:-}" == --chain ]] && ! dev_stack_built_contracts; then
    echo "==> engine $commit: building the contracts"
    dev_stack_build_contracts "$cache"
  fi
  [[ "${1:-}" != --chain ]] || dev_stack_check_abi
}

# Checks that every entry of the bot's contract bindings (crates/trader/abi/) is in the ABI the
# pinned commit compiles, so the bot calls the vault and the token as they are.
dev_stack_check_abi() {
  local name missing
  for name in Vault ERC20; do
    missing="$(jq -n --slurpfile mine "$dev_stack_root/crates/trader/abi/$name.json" \
      --slurpfile built "$dev_stack_dist/contracts/$name.json" \
      '$built[0].abi as $abi | [$mine[0][] | select(. as $entry | $abi | index([$entry]) | not) | .name] | join(", ")')"
    [[ "$missing" == '""' ]] \
      || dev_stack_fail "crates/trader/abi/$name.json has entries the engine's $name does not: $missing"
  done
}

# Whether $dev_stack_dist holds the node and the scenario tools.
dev_stack_built_node() {
  [[ -x "$dev_stack_dist/engine" && -x "$dev_stack_dist/access_source" && -x "$dev_stack_dist/sign_request" \
    && -f "$dev_stack_dist/dev.toml" ]]
}

# Whether $dev_stack_dist holds the contracts and files a chain stack runs from.
dev_stack_built_contracts() {
  local name
  for name in Vault MockUSDC TimelockController ERC20; do
    [[ -s "$dev_stack_dist/contracts/$name.json" ]] || return 1
  done
  [[ -f "$dev_stack_dist/anvil.toml" && -f "$dev_stack_dist/dev-rpc-proxy.py" ]]
}

# Builds the vault, the mock USDC and the OpenZeppelin timelock in `$1`/src/contracts (the
# dependencies are fetched by `forge soldeer install` and pinned by its lock) and copies the
# artifacts, anvil.toml and the RPC proxy into $dev_stack_dist, atomically.
dev_stack_build_contracts() {
  local src="$1/src" out="$1/src/contracts/out" tmp="$dev_stack_dist.chain.tmp" name artifact
  command -v forge > /dev/null || dev_stack_fail "forge is needed once to build the engine's contracts (https://getfoundry.sh)"
  (
    cd "$src/contracts"
    forge soldeer install > "$tmp.log" 2>&1 && forge build -q >> "$tmp.log" 2>&1
  ) || { tail -n 20 "$tmp.log" >&2; dev_stack_fail "building the contracts failed"; }
  rm -rf "$tmp"
  mkdir -p "$tmp/contracts"
  for name in Vault MockUSDC TimelockController ERC20; do
    artifact="$out/$name.sol/$name.json"
    [[ -s "$artifact" ]] || dev_stack_fail "forge built no $artifact"
    # Only the ABI and the creation bytecode are kept; a library placeholder would not deploy.
    jq '{abi, bytecode: {object: .bytecode.object}}' "$artifact" > "$tmp/contracts/$name.json"
    if grep -q '__\$' "$tmp/contracts/$name.json"; then dev_stack_fail "$name needs linking"; fi
  done
  cp "$src/config/anvil.toml" "$src/scripts/dev-rpc-proxy.py" "$tmp/"
  mkdir -p "$dev_stack_dist"
  rm -rf "$dev_stack_dist/contracts"
  mv "$tmp/anvil.toml" "$tmp/dev-rpc-proxy.py" "$dev_stack_dist/"
  mv "$tmp/contracts" "$dev_stack_dist/contracts"
  rm -rf "$tmp" "$tmp.log"
}

# A free loopback port, never one a local demo uses.
dev_stack_free_port() {
  local port
  while :; do
    port="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
    case "$port" in 3000 | 8080 | 8081 | 8545 | 9464 | 9465) ;; *) echo "$port"; return ;; esac
  done
}

# Starts Anvil and the contracts (with --chain), the access source and the node on fresh ports and
# data, waits until the node sequences and prices BTC-USD, and sets dev_stack_api, dev_stack_signer,
# dev_stack_pin and dev_stack_work (and, with --chain, the chain variables dev_stack_chain_start sets).
dev_stack_start() {
  local api_port metrics_port source_port max_drift wait_s dev_toml attestation
  dev_stack_chain=0
  [[ "${1:-}" != --chain ]] || dev_stack_chain=1
  if (( dev_stack_chain )); then dev_stack_build --chain; else dev_stack_build; fi
  dev_stack_work="$(mktemp -d)"
  api_port="$(dev_stack_free_port)"
  metrics_port="$(dev_stack_free_port)"
  source_port="$(dev_stack_free_port)"
  dev_stack_api="https://127.0.0.1:$api_port"
  if (( dev_stack_chain )); then
    dev_stack_chain_start
  fi

  printf '{"mode":"clear","jurisdictions":{},"sanctioned":[]}\n' > "$dev_stack_work/access-fixture.json"
  "$dev_stack_dist/access_source" "127.0.0.1:$source_port" "$dev_stack_chain_id" "$dev_stack_vault" \
    "$dev_stack_work/access-fixture.json" > "$dev_stack_work/access-source.log" 2>&1 &
  dev_stack_source_pid=$!

  if (( dev_stack_chain )); then
    dev_toml="$dev_stack_dist/anvil.toml"
    dev_stack_chain_config_file "$dev_toml" "$api_port" "$metrics_port" "${dev_stack_proxy_ports[@]}" > "$dev_stack_work/node.toml"
  else
    dev_toml="$dev_stack_dist/dev.toml"
    sed -e "s|127.0.0.1:8080|127.0.0.1:$api_port|" \
      -e "s|127.0.0.1:9464|127.0.0.1:$metrics_port|" \
      -e "s|data/dev|$dev_stack_work|g" \
      "$dev_toml" > "$dev_stack_work/node.toml"
  fi
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

  # EVM custody needs a Critical alert route; a closed local port stands in for it, as in the
  # engine's scenarios (alerts are read from the node log).
  ENGINE_ALERTS_CRITICAL_URL="http://127.0.0.1:9/alerts" \
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
  if (( dev_stack_chain )); then
    [[ "$(tr A-F a-f <<< "$dev_stack_signer")" == "$(tr A-F a-f <<< "$dev_stack_mock_signer")" ]] \
      || dev_stack_fail "the node's enclave signer $dev_stack_signer is not the vault's checkpoint signer $dev_stack_mock_signer"
    dev_stack_house_capital
  fi
}

# Funds the house with $dev_stack_house_usdc through the vault, as the engine's deposit scenario does:
# under EVM custody genesis credits the house nothing, and without capital it takes no trade. Waits
# until the node has credited it.
dev_stack_house_capital() {
  local units=$(( dev_stack_house_usdc * 1000000 )) nav=""
  cast send --rpc-url "$dev_stack_rpc" --private-key "$dev_stack_deployer_key" "$dev_stack_usdc" \
    'mint(address,uint256)' "$(cast wallet address --private-key "$dev_stack_deployer_key")" "$units" > /dev/null \
    || dev_stack_fail "minting the house's USDC failed"
  cast send --rpc-url "$dev_stack_rpc" --private-key "$dev_stack_deployer_key" "$dev_stack_usdc" \
    'approve(address,uint256)' "$dev_stack_vault" "$units" > /dev/null || dev_stack_fail "approving the house's USDC failed"
  cast send --rpc-url "$dev_stack_rpc" --private-key "$dev_stack_deployer_key" "$dev_stack_vault" \
    'depositToHouse(uint256)' "$units" > /dev/null || dev_stack_fail "depositing the house's capital failed"
  for _ in $(seq 1 300); do
    nav="$(curl -sk --max-time 2 "$dev_stack_api/v1/house" | jq -r '.nav // "0"' 2>/dev/null || echo 0)"
    [[ "$nav" == "$dev_stack_house_usdc" ]] && return 0
    sleep 0.2
  done
  dev_stack_fail "the node did not credit the house's $dev_stack_house_usdc USDC (NAV $nav)"
}

# Starts Anvil and three local RPC proxies to it (custody.evm.providers needs three distinct URLs),
# deploys the mock USDC, an OpenZeppelin timelock and the vault from the cached artifacts, and sets
# dev_stack_rpc, dev_stack_vault, dev_stack_usdc, dev_stack_monkey and dev_stack_proxy_ports.
#
# Blocks: Anvil mines on its own every second (--block-time), and also for each transaction it is
# sent. Automine alone does not work: with one slot per epoch the `finalized` block trails the head,
# so after the deploys nothing moves it and the node's custody start (which reads at that tag) never
# sees the vault; later a deposit is credited at the `safe` tag plus confirmations, so it too needs
# blocks after its own, which checkpoint posts supply only while the node sequences enough commands.
# A steady block clock keeps credit and finality independent of that. The cost is that a transaction
# waits up to a second for its block (cast send returns after the receipt).
dev_stack_chain_start() {
  local anvil_port deployer governance port timelock_args vault_args code tool
  for tool in anvil cast; do
    command -v "$tool" > /dev/null || dev_stack_fail "$tool is needed for --chain (https://getfoundry.sh)"
  done
  anvil_port="$(dev_stack_free_port)"
  dev_stack_rpc="http://127.0.0.1:$anvil_port"
  anvil --host 127.0.0.1 --port "$anvil_port" --chain-id "$dev_stack_chain_id" --slots-in-an-epoch 1 \
    --block-time 1 --silent > "$dev_stack_work/anvil.log" 2>&1 &
  dev_stack_anvil_pid=$!
  for _ in $(seq 1 100); do
    kill -0 "$dev_stack_anvil_pid" 2>/dev/null || dev_stack_fail "anvil exited at startup: $(tail -n 5 "$dev_stack_work/anvil.log")"
    cast chain-id --rpc-url "$dev_stack_rpc" > /dev/null 2>&1 && break
    sleep 0.1
  done
  cast chain-id --rpc-url "$dev_stack_rpc" > /dev/null 2>&1 || dev_stack_fail "anvil did not come up"

  dev_stack_proxy_ports=()
  for _ in 1 2 3; do
    port="$(dev_stack_free_port)"
    python3 "$dev_stack_dist/dev-rpc-proxy.py" --port "$port" --target "$dev_stack_rpc" \
      > "$dev_stack_work/proxy-$port.log" 2>&1 &
    dev_stack_proxy_pids+=("$!")
    dev_stack_proxy_ports+=("$port")
  done
  for port in "${dev_stack_proxy_ports[@]}"; do
    for _ in $(seq 1 100); do
      cast chain-id --rpc-url "http://127.0.0.1:$port" > /dev/null 2>&1 && break
      sleep 0.1
    done
    cast chain-id --rpc-url "http://127.0.0.1:$port" > /dev/null 2>&1 || dev_stack_fail "an RPC proxy did not come up"
  done

  deployer="$(cast wallet address --private-key "$dev_stack_deployer_key")"
  dev_stack_usdc="$(dev_stack_deploy MockUSDC "")"
  # The deployer is the timelock's one proposer and the vault's guardian, with a one-second delay.
  timelock_args="$(cast abi-encode 'f(uint256,address[],address[],address)' 1 "[$deployer]" \
    '[0x0000000000000000000000000000000000000000]' 0x0000000000000000000000000000000000000000)"
  governance="$(dev_stack_deploy TimelockController "$timelock_args")"
  # The exit period, claim window and deposit staleness are thirty days each, past anything a
  # scenario waits for, as in the engine's scenarios.
  vault_args="$(cast abi-encode 'f(address,address,address,address,uint256,uint64,uint64,uint64)' \
    "$dev_stack_usdc" "$dev_stack_mock_signer" "$governance" "$deployer" "$dev_stack_period_budget" \
    2592000 2592000 2592000)"
  dev_stack_vault="$(dev_stack_deploy Vault "$vault_args")"
  dev_stack_monkey="$(cast call --rpc-url "$dev_stack_rpc" "$dev_stack_vault" 'monkey()(address)')" \
    || dev_stack_fail "the vault has no monkey()"
  # The node's custody start reads at the withdrawal finality tag, which trails the head.
  for _ in $(seq 1 100); do
    code="$(cast code --rpc-url "$dev_stack_rpc" --block finalized "$dev_stack_vault" 2>/dev/null || true)"
    [[ -z "$code" || "$code" == 0x ]] || break
    sleep 0.2
  done
  [[ -n "$code" && "$code" != 0x ]] || dev_stack_fail "the vault never reached the finalized block"
  export dev_stack_rpc dev_stack_vault dev_stack_usdc dev_stack_monkey
}

# Deploys the cached artifact `$1` with ABI-encoded constructor arguments `$2` from the deployer
# and prints its address.
dev_stack_deploy() {
  local code address
  code="$(jq -r .bytecode.object "$dev_stack_dist/contracts/$1.json")${2#0x}"
  address="$(cast send --rpc-url "$dev_stack_rpc" --private-key "$dev_stack_deployer_key" --json --create "$code" \
    | jq -er .contractAddress)" || dev_stack_fail "deploying $1 failed"
  echo "$address"
}

# Prints `$1`, the commit's anvil.toml, for this stack: ports `$2` (API) and `$3` (metrics) and the
# three RPC proxies `$4 $5 $6` in place of the engine's, the work dir for every data path, the
# deployed vault and USDC, a checkpoint every few seconds, and a [checkpoints] section that posts
# through Anvil from an unlocked account.
dev_stack_chain_config_file() {
  python3 -I - "$@" "$dev_stack_work" "$dev_stack_vault" "$dev_stack_usdc" "$dev_stack_rpc" "$dev_stack_relayer" <<'PY_CONFIG'
import re
import sys
from pathlib import Path

source, api, metrics, rpc_a, rpc_b, rpc_c, work, vault, usdc, anvil, relayer = sys.argv[1:]
replacements = {
    "data/anvil": work,
    "127.0.0.1:8545": "127.0.0.1:" + rpc_a,
    "127.0.0.1:8546": "127.0.0.1:" + rpc_b,
    "127.0.0.1:8547": "127.0.0.1:" + rpc_c,
    "127.0.0.1:8080": "127.0.0.1:" + api,
    "127.0.0.1:9464": "127.0.0.1:" + metrics,
}
text = re.sub("|".join(map(re.escape, replacements)), lambda m: replacements[m[0]], Path(source).read_text())
# The engine's values are the addresses its scenario deploys to; this stack's differ.
for key, value in (("vault", vault), ("usdc", usdc), ("checkpoint_every", "25")):
    quoted = key != "checkpoint_every"
    text, n = re.subn(rf'^{key} = .*$', f'{key} = "{value}"' if quoted else f"{key} = {value}", text, flags=re.M)
    assert n == 1, f"{key}: {n} matches in the config"
print(text, end="")
print(f"""
[checkpoints]
rpc_url = "{anvil}"
relayer = "{relayer}"
retry_interval = "500ms"
rpc_timeout = "5s"
""", end="")
PY_CONFIG
}

# Gives `$1` an address `$2` whole mock USDC (minted by the deployer) and `$3` ether, with --chain.
dev_stack_wallet() {
  (( dev_stack_chain )) || dev_stack_fail "dev_stack_wallet needs a stack started with --chain"
  cast send --rpc-url "$dev_stack_rpc" --private-key "$dev_stack_deployer_key" "$dev_stack_usdc" \
    'mint(address,uint256)' "$1" "$(( $2 * 1000000 ))" > /dev/null || dev_stack_fail "minting USDC for $1 failed"
  cast rpc --rpc-url "$dev_stack_rpc" anvil_setBalance "$1" "$(cast to-hex "$(cast to-wei "$3")")" > /dev/null \
    || dev_stack_fail "setting the balance of $1 failed"
}

# Prints the [chain] section of a bot config for this stack. Anvil quotes fees of about a gwei, above
# the bot's default cap.
dev_stack_chain_config() {
  cat <<CONFIG
[chain]
rpc = ["$dev_stack_rpc"]
max_fee_gwei = "100"
CONFIG
}

# Stops the node and the access source, keeps their logs under SCENARIO_ARTIFACTS, and removes the
# node's data.
dev_stack_stop() {
  local pid
  for pid in "$dev_stack_node_pid" "$dev_stack_source_pid" ${dev_stack_proxy_pids[@]+"${dev_stack_proxy_pids[@]}"} "$dev_stack_anvil_pid"; do
    if [[ -n "$pid" ]]; then kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; fi
  done
  dev_stack_node_pid=""
  dev_stack_source_pid=""
  dev_stack_anvil_pid=""
  dev_stack_proxy_pids=()
  if [[ -n "${dev_stack_work:-}" ]]; then
    if [[ -n "${SCENARIO_ARTIFACTS:-}" ]]; then
      mkdir -p "$SCENARIO_ARTIFACTS"
      cp "$dev_stack_work"/*.log "$dev_stack_work"/*.toml "$SCENARIO_ARTIFACTS/" 2>/dev/null || true
    fi
    rm -rf "$dev_stack_work"
  fi
}

# Prints the [node] section of a bot config for this node (its vault is the deployed one with --chain).
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

dev_stack_locate

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  [[ $# -eq 0 || ( $# -eq 1 && "$1" == --chain ) ]] || { echo "usage: $0 [--chain]" >&2; exit 2; }
  trap dev_stack_stop EXIT
  trap 'exit 130' INT TERM
  dev_stack_start "$@"
  if (( dev_stack_chain )); then
    echo "==> a dev node and chain are up; the bot config sections:"
    dev_stack_node_config
    dev_stack_chain_config
    echo "==> USDC $dev_stack_usdc, MONKEY $dev_stack_monkey; fund a wallet with dev_stack_wallet <address> <usdc> <eth>"
    echo "    after sourcing this script, deposit into the vault, and declare it with dev_stack_declare"
  else
    echo "==> a dev node is up; its [node] config:"
    dev_stack_node_config
    echo "==> fund and declare an account: source this script and call dev_stack_fund <address> <amount> and"
    echo "    dev_stack_declare <hex key> <address> with dev_stack_api=$dev_stack_api; the deposit credits once declared"
  fi
  echo "==> the node's log: $dev_stack_work/node.log; interrupt to stop"
  wait "$dev_stack_node_pid"
fi
