#!/usr/bin/env bash
# Starts several bots against a running local stack, each on its own strategy and its own console
# port, so their figures can be watched side by side. For a development stack only: the bots are
# a development build's (hex key files, a mock attestation), and the funding mints test USDC.
#
#   scripts/demo-bots.sh start   keys, configs, funding, declarations, then the bots in the background
#   scripts/demo-bots.sh stop    stops exactly the bots `start` started, by the PIDs it recorded
#
# Settings, as environment variables:
#   DEMO_WORK_DIR      required: a new directory, created here with mode 700, for the keys, configs, logs
#                      and PIDs
#   DEMO_NODE_FILE     required: a file holding the [node] section a dev stack prints
#                      (`scripts/dev-stack.sh` prints it); a [chain] section in it is ignored
#   DEMO_RPC           the chain's JSON-RPC URL; without it the stack has mock custody and each
#                      account is funded through the node's /v1/dev/deposit
#   DEMO_VAULT         the vault's address (with DEMO_RPC)
#   DEMO_USDC          the USDC token's address (with DEMO_RPC)
#   DEMO_DEPLOYER_KEY  the hex key that mints the test USDC (with DEMO_RPC; default: Anvil's account 0)
#   DEMO_DEPOSIT       USDC each account deposits (default 10000)
#   DEMO_BOTS          how many bots, 1 to 6 (default 5)
#   DEMO_BASE_PORT     the first console port; bot N uses DEMO_BASE_PORT + N - 1 (default 4101)
#   TRADER_BIN         the trader binary (default: target/debug/trader of this checkout, which
#                      `cargo build -p trader` builds)
#
# Needs curl, jq, python3 and, with DEMO_RPC, Foundry's cast. Each console's login password and
# certificate fingerprint are in its bot's log, and the password is printed here too.
set -Eeuo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
# The demo stack's own ports, which no bot may take.
forbidden=(3000 8080 8081 8082 8480 8481 8482 8545 9443 9444 9445 9464 9465 9466 18846)
# Anvil's published account 0.
anvil_deployer="ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"

die() { echo "demo-bots: $*" >&2; exit 1; }

work="${DEMO_WORK_DIR:-}"
[[ -n "$work" ]] || die "set DEMO_WORK_DIR"
[[ "$work" == /* ]] || die "DEMO_WORK_DIR must be an absolute path"

stop_bots() {
  local pids="$work/pids" pid dir command
  [[ -f "$pids" ]] || die "no $pids: nothing was started from this work directory"
  while IFS='|' read -r pid dir; do
    [[ -n "$pid" ]] || continue
    # Only a process that is still the bot started with this config is signalled: a PID that has been
    # reused since is left alone.
    command="$(ps -p "$pid" -o command= 2>/dev/null || true)"
    if [[ "$command" == *" run --config $dir/bot.toml" ]]; then
      kill "$pid" && echo "stopped $pid"
    else
      echo "skipped $pid: not the bot of $dir"
    fi
  done < "$pids"
}

if [[ "${1:-}" == stop ]]; then
  stop_bots
  exit 0
fi
[[ "${1:-}" == start ]] || die "usage: demo-bots.sh <start|stop>"

node_file="${DEMO_NODE_FILE:-}"
[[ -f "$node_file" ]] || die "set DEMO_NODE_FILE to a file holding the [node] section"
rpc="${DEMO_RPC:-}"
vault="${DEMO_VAULT:-}"
usdc="${DEMO_USDC:-}"
deployer_key="${DEMO_DEPLOYER_KEY:-$anvil_deployer}"
deposit="${DEMO_DEPOSIT:-10000}"
count="${DEMO_BOTS:-5}"
base_port="${DEMO_BASE_PORT:-4101}"
trader="${TRADER_BIN:-$root/target/debug/trader}"

[[ "$count" =~ ^[1-6]$ ]] || die "DEMO_BOTS must be 1 to 6"
[[ "$base_port" =~ ^[0-9]+$ && "$deposit" =~ ^[0-9]+$ ]] || die "DEMO_BASE_PORT and DEMO_DEPOSIT must be whole numbers"
[[ -x "$trader" ]] || die "no trader binary at $trader; run: cargo build -p trader"
command -v curl > /dev/null && command -v jq > /dev/null && command -v python3 > /dev/null \
  || die "curl, jq and python3 are needed"
if [[ -n "$rpc" ]]; then
  command -v cast > /dev/null || die "cast (Foundry) is needed with DEMO_RPC"
  [[ "$vault" =~ ^0x[0-9a-fA-F]{40}$ && "$usdc" =~ ^0x[0-9a-fA-F]{40}$ ]] \
    || die "set DEMO_VAULT and DEMO_USDC to addresses with DEMO_RPC"
fi

api="$(sed -n 's/^url = "\(.*\)"/\1/p' "$node_file" | head -n 1)"
[[ "$api" == https://* ]] || die "$node_file has no [node] url"

# The ports must be outside the demo's and free now.
for ((n = 0; n < count; n++)); do
  port=$((base_port + n))
  for taken in "${forbidden[@]}"; do
    [[ "$port" != "$taken" ]] || die "console port $port is one the local demo uses"
  done
  python3 -I -c 'import socket, sys; s = socket.socket(); s.bind(("127.0.0.1", int(sys.argv[1])))' "$port" 2> /dev/null \
    || die "console port $port is in use"
done

# The directory is the script's own: a new one, which holds keys.
[[ ! -e "$work" ]] || die "$work exists; name a directory that does not"
mkdir -m 700 -p "$work"

# Strategy n: its name and its [[account.strategy]] parameters. Every bot trades both markets on
# one-minute bars.
strategy_name=(sma_crossover ema_crossover macd rsi_reversion bollinger_reversion donchian_breakout)
strategy_params=(
  'fast = 5
slow = 20'
  'fast = 9
slow = 21'
  'fast = 12
slow = 26
signal = 9'
  'period = 14
oversold = 30
exit = 50
overbought = 70'
  'period = 20
width = "2"'
  'entry = 20
exit = 10'
)

# The node section without any [chain] table, which this script writes itself.
node_section="$(awk '/^\[chain\]/ { skip = 1; next } /^\[/ { skip = 0 } !skip' "$node_file")"

summary=""
for ((n = 0; n < count; n++)); do
  id=$((n + 1))
  port=$((base_port + n))
  dir="$work/bot-$id"
  mkdir -p "$dir"
  key="$(python3 -I -c 'import secrets; print(secrets.token_hex(32))')"
  ( umask 077; printf '%s\n' "$key" > "$dir/account.hex" )
  {
    printf '%s\n' "$node_section"
    if [[ -n "$rpc" ]]; then
      printf '\n[chain]\nrpc = ["%s"]\nmax_fee_gwei = "100"\n' "$rpc"
    fi
    cat <<CONFIG

[state]
dir = "state"

[sizing]
by = "fixed_notional"
notional = "1000"
max_leverage = "5"

[risk]
max_open_positions = 2
max_total_notional = "5000"
daily_loss = { usd = "200" }
margin_buffer_bps = 10000
min_hold = "5m"

[[account]]
key = { type = "hex_file", path = "account.hex" }
close_on_stop = true
CONFIG
    for market in BTC-USD ETH-USD; do
      cat <<CONFIG

[[account.strategy]]
market = "$market"
bar = "1m"
max_slippage_bps = 25
kind = "${strategy_name[n]}"
${strategy_params[n]}
CONFIG
    done
    cat <<CONFIG

[console]
bind = "127.0.0.1:$port"
data_dir = "data"

[logging]
level = "info"
CONFIG
  } > "$dir/bot.toml"
  chmod 600 "$dir/bot.toml"
  summary+="$id|$port|${strategy_name[n]}|$dir"$'\n'
done

# The addresses come from the bot itself, so they are the ones it will sign with.
addresses() { "$trader" addresses --config "$1/bot.toml"; }

units=$((deposit * 1000000))
for ((n = 0; n < count; n++)); do
  id=$((n + 1))
  dir="$work/bot-$id"
  key="$(<"$dir/account.hex")"
  account="$(addresses "$dir" | grep -oE '0x[0-9a-fA-F]{40}' | head -n 1)"
  [[ -n "$account" ]] || die "bot $id has no address"
  echo "==> bot $id: $account"
  if [[ -n "$rpc" ]]; then
    # Gas, then the test USDC, the vault's allowance and the deposit, one transaction each.
    cast rpc --rpc-url "$rpc" anvil_setBalance "$account" "$(cast to-hex "$(cast to-wei 1)")" > /dev/null \
      || die "giving bot $id gas failed"
    cast send --rpc-url "$rpc" --private-key "$deployer_key" "$usdc" 'mint(address,uint256)' "$account" "$units" > /dev/null \
      || die "minting USDC for bot $id failed"
    cast send --rpc-url "$rpc" --private-key "$key" "$usdc" 'approve(address,uint256)' "$vault" "$units" > /dev/null \
      || die "bot $id's approval failed"
    cast send --rpc-url "$rpc" --private-key "$key" "$vault" 'deposit(uint256)' "$units" > /dev/null \
      || die "bot $id's deposit failed"
  else
    curl -sfk --max-time 10 "$api/v1/dev/deposit" -H 'content-type: application/json' \
      -d "{\"account\":\"$account\",\"amount\":\"$deposit\"}" > /dev/null || die "the dev deposit for bot $id failed"
  fi
  # The statement is the owner's to make: running this script is the confirmation for these keys.
  "$trader" accounts --config "$dir/bot.toml" declare --class professional --confirm "$account" \
    > "$dir/declare.log" 2>&1 || { tail -n 20 "$dir/declare.log" >&2; die "bot $id's declaration failed"; }
done

# A bot starts only for an account the node has credited, which a chain deposit takes a few blocks
# to do; the node's answer is read, never the chain.
for ((n = 0; n < count; n++)); do
  account="$(addresses "$work/bot-$((n + 1))" | grep -oE '0x[0-9a-fA-F]{40}' | head -n 1)"
  for _ in $(seq 1 180); do
    curl -sfk --max-time 5 "$api/v1/accounts/$account" > /dev/null 2>&1 && continue 2
    sleep 1
  done
  die "the node has not credited $account within three minutes"
done

: > "$work/pids"
chmod 600 "$work/pids"
pid_of=()
while IFS='|' read -r id port name dir; do
  [[ -n "$id" ]] || continue
  # The config's relative paths are read from its own directory, so no cd is needed, and $! is the bot.
  nohup "$trader" run --config "$dir/bot.toml" < /dev/null > "$dir/bot.log" 2>&1 &
  pid_of[id]=$!
  echo "${pid_of[id]}|$dir" >> "$work/pids"
done <<< "$summary"

# A bot is up once its accounts have started and its console listens; one that exits first is reported.
failed=0
while IFS='|' read -r id port name dir; do
  [[ -n "$id" ]] || continue
  up=0
  for _ in $(seq 1 240); do
    if grep -q 'started accounts=[1-9]' "$dir/bot.log" && grep -q 'console listening' "$dir/bot.log"; then up=1; break; fi
    kill -0 "${pid_of[id]}" 2> /dev/null || break
    sleep 0.25
  done
  if (( up )); then
    password="$(sed -n 's/^console password.*login\.argon2: //p' "$dir/bot.log")"
    echo "bot $id: $name on BTC-USD and ETH-USD, console https://127.0.0.1:$port, password $password"
  else
    failed=1
    echo "bot $id ($name) did not come up; its log, $dir/bot.log, ends:" >&2
    tail -n 5 "$dir/bot.log" >&2
  fi
done <<< "$summary"

echo "Each console uses a self-signed certificate; the bot's log holds its fingerprint."
echo "Stop them with: DEMO_WORK_DIR=$work $0 stop"
exit "$failed"
