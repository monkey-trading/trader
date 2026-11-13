#!/usr/bin/env bash
# Dev scenario: two accounts declared, funded, claimed for and withdrawn from through the console, on the
# chain dev stack (scripts/dev-stack.sh --chain):
#   1. two accounts the node has never credited, and a funding key holding 1,000 USDC and gas: the bot
#      starts with none trading, and serves them through its console;
#   2. the operator records a MONKEY emission schedule, and the bot runs with its console;
#   3. the browser test (console/browser-test/funding.mjs, phase prepare) opens the Funding page, selects
#      the two accounts, declares both on one confirmation, and deposits 1,000 USDC into the first from
#      its own wallet and into the second from the funding wallet; each wallet, the vault, the
#      funding key and each account's balance and declaration in the node move by exactly that, and the
#      accounts sent only the transactions their own deposits need;
#   4. the bot restarts, so the two trade, and trades until each has a MONKEY entitlement; the console
#      flattens and stops them, so their balances hold still;
#   5. once each entitlement is claimable, the browser test (phase settle) claims for both and withdraws
#      100 USDC from each through the page: each account holds the MONKEY the vault records as claimed
#      and sent no transaction itself, each wallet is paid exactly 100 USDC, the vault pays out exactly 200
#      and each balance in the node falls by exactly 100;
#   6. the bot stops on SIGTERM with exit 0.
#
# Needs what `scripts/dev-stack.sh --chain` needs, Node.js with npm, and Playwright 1.64.x installed in a
# scratch directory outside the repository, named by PLAYWRIGHT_DIR (`npm install playwright@1.64.1 &&
# npx playwright install chromium` there). About fifteen minutes, up to forty: it trades until the two
# accounts have an entitlement. TRADER_BIN runs a built trader (with the console embedded) instead of
# building one; SCENARIO_ARTIFACTS keeps the node's and the bot's logs and configs.
set -Eeuo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
source "$root/scripts/dev-stack.sh"
: "${PLAYWRIGHT_DIR:?set PLAYWRIGHT_DIR to a directory with playwright installed}"
# Anvil's published accounts 2 and 3 are the accounts, and 5 the funding key.
keys=(
  5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a
  7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6
)
funding_key="8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba"
period_ms=600000
bot_pid=""

cleanup() {
  if [[ -n "$bot_pid" ]]; then kill "$bot_pid" 2>/dev/null || true; wait "$bot_pid" 2>/dev/null || true; fi
  if [[ -n "${SCENARIO_ARTIFACTS:-}" && -n "${dev_stack_work:-}" ]]; then
    mkdir -p "$SCENARIO_ARTIFACTS"
    cp "$dev_stack_work"/bot*.log "$dev_stack_work/bot.toml" "$SCENARIO_ARTIFACTS/" 2>/dev/null || true
  fi
  dev_stack_stop
}
trap cleanup EXIT
step() { echo "==> $*"; }
fail() {
  echo "FAIL: $*" >&2
  for log in "${dev_stack_work:-}"/bot*.log; do [[ -f "$log" ]] && tail -n 30 "$log" >&2; done
  exit 1
}
trap 'fail "the command at line $LINENO failed"' ERR
now_ms() { python3 -c 'import time; print(int(time.time() * 1000))'; }
node_get() { curl -sfk --max-time 5 "$dev_stack_api$1"; }
node_balance() { node_get "/v1/accounts/$1" | jq -r '.balance // "none"' 2>/dev/null || echo none; }
usdc_of() { cast call --rpc-url "$dev_stack_rpc" "$dev_stack_usdc" 'balanceOf(address)(uint256)' "$1" | cut -d' ' -f1; }
monkey_of() { cast call --rpc-url "$dev_stack_rpc" "$dev_stack_monkey" 'balanceOf(address)(uint256)' "$1" | cut -d' ' -f1; }
claimed_of() { cast call --rpc-url "$dev_stack_rpc" "$dev_stack_vault" 'emissionsClaimed(address)(uint256)' "$1" | cut -d' ' -f1; }
nonce_of() { cast nonce --rpc-url "$dev_stack_rpc" "$1"; }
same() { python3 -c 'import sys; from decimal import Decimal as D; sys.exit(0 if D(sys.argv[1]) == D(sys.argv[2]) else 1)' "$1" "$2"; }
accounts() { "$trader" accounts --config "$work/bot.toml" "$@" 2>&1 | tee -a "$work/bot-accounts.log"; }
# Starts the bot, logging to the named file, and waits for its console.
start_bot() {
  "$trader" run --config "$work/bot.toml" > "$work/$1" 2>&1 &
  bot_pid=$!
  for _ in $(seq 1 200); do
    grep -q 'console listening' "$work/$1" && return 0
    kill -0 "$bot_pid" 2>/dev/null || fail "the bot exited at startup"
    sleep 0.2
  done
  fail "the console never listened"
}
# Stops the bot with SIGTERM; it must exit with $1 (default 0): a bot that started without some accounts
# exits 1 to say so.
stop_bot() {
  local want="${1:-0}"
  local status=""
  kill -TERM "$bot_pid"
  for _ in $(seq 1 1200); do
    if ! kill -0 "$bot_pid" 2>/dev/null; then wait "$bot_pid" && status=0 || status=$?; break; fi
    sleep 0.1
  done
  bot_pid=""
  [[ "$status" == "$want" ]] || fail "the bot exited with '${status:-no exit within two minutes}', not $want"
}
# The console's status for a call; its body is left in $work/body.
console_status() {
  local method="$1" path="$2"
  shift 2
  curl -sk --max-time 130 -X "$method" -o "$work/body" -w '%{http_code}' "$@" "$console$path"
}
console_login() {
  [[ "$(console_status POST /api/login -c "$work/cookies" -H "Origin: $console" -H 'content-type: application/json' \
    -d "{\"password\":\"$password\"}")" == 204 ]] || fail "the console refused the login"
}
browser() {
  CONSOLE_URL="$console" CONSOLE_PASSWORD="$password" PLAYWRIGHT_DIR="$PLAYWRIGHT_DIR" FUNDING_PHASE="$1" \
    FUNDING_ACCOUNTS="${managed[0]},${managed[1]}" node "$root/console/browser-test/funding.mjs" \
    || fail "the funding browser test failed (phase $1)"
}

step "build the console and the bot"
if [[ -n "${TRADER_BIN:-}" ]]; then
  trader="$TRADER_BIN"
else
  (cd "$root/console" && npm ci --no-audit --no-fund > /dev/null && npm run build > /dev/null)
  read -r -a cargo_cmd <<< "${DEV_STACK_CARGO:-cargo}"
  "${cargo_cmd[@]}" build -q --locked -p trader --manifest-path "$root/Cargo.toml"
  trader="$(cargo metadata --format-version 1 --no-deps --manifest-path "$root/Cargo.toml" | jq -r .target_directory)/debug/trader"
fi
[[ -x "$trader" ]] || fail "no trader binary at $trader"

step "start the chain dev stack"
dev_stack_start --chain
work="$dev_stack_work"
addresses=()
for key in "${keys[@]}"; do addresses+=("$(cast wallet address --private-key "0x$key")"); done
funding="$(cast wallet address --private-key "0x$funding_key")"
managed=("$(tr '[:upper:]' '[:lower:]' <<< "${addresses[0]}")" "$(tr '[:upper:]' '[:lower:]' <<< "${addresses[1]}")")

step "give each account 1,500 USDC and 1 ETH, and the funding key 1,000 USDC and 1 ETH"
for account in "${addresses[@]}"; do dev_stack_wallet "$account" 1500 1; done
dev_stack_wallet "$funding" 1000 1

step "write the bot's config: two accounts trading BTC-USD, the funding key, and a console"
console_port="$(dev_stack_free_port)"
console="https://127.0.0.1:$console_port"
{
  dev_stack_node_config
  echo
  dev_stack_chain_config
  echo 'settle_timeout = "10m"'
  echo 'poll_interval = "1s"'
  for i in "${!keys[@]}"; do
    printf '%s\n' "${keys[$i]}" > "$work/account-$i.hex"
    cat <<CONFIG

[[account]]
key = { type = "hex_file", path = "account-$i.hex" }
close_on_stop = true

[[account.strategy]]
market = "BTC-USD"
bar = "1m"
kind = "sma_crossover"
fast = 1
slow = 2
max_slippage_bps = 25
CONFIG
  done
  cat <<CONFIG

[sizing]
by = "fixed_notional"
notional = "500"
max_leverage = "5"

[risk]
max_open_positions = 1
max_total_notional = "1000"
daily_loss = { usd = "1000" }
margin_buffer_bps = 10000
min_hold = "0s"
CONFIG
  printf '%s\n' "$funding_key" > "$work/funding.hex"
  cat <<CONFIG

[funding]
key = { type = "hex_file", path = "funding.hex" }

[console]
bind = "127.0.0.1:$console_port"
data_dir = "console-data"
refresh_interval = "2s"

[logging]
level = "info"
CONFIG
} > "$work/bot.toml"
chmod 600 "$work"/*.hex

step "neither account has been credited by the node"
for account in "${managed[@]}"; do
  [[ "$(node_balance "$account")" == none ]] || fail "$account was credited before the console funded it"
done

step "record a MONKEY emission schedule that starts in ten seconds"
start_ms=$(( $(now_ms) / 1000 * 1000 + 10000 ))
start_at="$(python3 -c 'import datetime, sys; print(datetime.datetime.fromtimestamp(int(sys.argv[1]) / 1000, datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"))' "$start_ms")"
dev_stack_admin init-emissions --start "$start_at" --period-budget "$dev_stack_period_budget"

step "run the bot: no account can trade yet, and the console serves both"
start_bot bot.log
password="$(sed -nE 's/^console password, shown once .*: ([a-z0-9]+)$/\1/p' "$work/bot.log")"
[[ ${#password} == 26 ]] || fail "no password was printed"

step "declare the two accounts and fund them through the console"
vault_before="$(usdc_of "$dev_stack_vault")"
wallet_before=("$(usdc_of "${managed[0]}")" "$(usdc_of "${managed[1]}")")
funding_wallet_before="$(usdc_of "$funding")"
nonce_before=("$(nonce_of "${managed[0]}")" "$(nonce_of "${managed[1]}")")
browser prepare
for account in "${managed[@]}"; do
  [[ "$(node_get "/v1/accounts/$account/eligibility" | jq -r .class)" == professional ]] || fail "$account did not declare"
  same "$(node_balance "$account")" 1000 || fail "$account's balance is $(node_balance "$account"), not 1000"
done
[[ "$(usdc_of "${managed[0]}")" == $(( wallet_before[0] - 1000000000 )) ]] || fail "the first managed wallet did not pay its own deposit"
[[ "$(usdc_of "${managed[1]}")" == "${wallet_before[1]}" ]] || fail "the second managed wallet paid a deposit the funding wallet made"
[[ "$(usdc_of "$funding")" == $(( funding_wallet_before - 1000000000 )) ]] || fail "the funding wallet did not pay the second deposit"
[[ "$(usdc_of "$dev_stack_vault")" == $(( vault_before + 2000000000 )) ]] || fail "the vault did not take exactly 2,000 USDC"
[[ "$(nonce_of "${managed[0]}")" == $(( nonce_before[0] + 2 )) ]] || fail "the first managed account sent other than an approval and a deposit"
[[ "$(nonce_of "${managed[1]}")" == "${nonce_before[1]}" ]] || fail "the second managed account sent a transaction"

step "restart the bot so both trade, until the two have a MONKEY entitlement (at most three periods)"
# The first run started without its accounts, and exits 1 to say so.
stop_bot 1
grep -q "2 accounts could not start" "$work/bot.log" || fail "the first run did not report the two accounts that could not start"
restart_ms="$(now_ms)"
start_bot bot-restarted.log
entitled() { node_get "/v1/accounts/$1/emissions" 2>/dev/null | jq -r '.newest_entitled // "0"' 2>/dev/null || echo 0; }
deadline=$(( restart_ms + 3 * period_ms + 120000 ))
while :; do
  kill -0 "$bot_pid" 2>/dev/null || fail "the bot exited while trading"
  missing=0
  for account in "${managed[@]}"; do [[ "$(entitled "$account")" != 0 ]] || missing=$((missing + 1)); done
  (( missing == 0 )) && break
  (( $(now_ms) < deadline )) || fail "$missing accounts realised no loss in three periods"
  sleep 10
done

step "flatten and stop the two through the console, so their balances hold still"
console_login
for account in "${managed[@]}"; do
  [[ "$(console_status POST "/api/accounts/$account/flatten" -b "$work/cookies" -H "Origin: $console")" == 204 ]] \
    || fail "flattening $account answered $(cat "$work/body")"
  [[ "$(node_get "/v1/accounts/$account" | jq '[.positions[] | select((.qty | tonumber) != 0)] | length')" == 0 ]] \
    || fail "$account holds a position after its flatten"
done

step "wait until each entitlement is claimable"
claimable() { node_get "/v1/accounts/$1/emissions" 2>/dev/null | jq -r 'select(.claimable) | .entitled' 2>/dev/null || true; }
entitled_before=()
for account in "${managed[@]}"; do
  entitlement=""
  for _ in $(seq 1 300); do
    entitlement="$(claimable "$account")"
    [[ -n "$entitlement" && "$entitlement" != 0 ]] && break
    sleep 1
  done
  [[ -n "$entitlement" && "$entitlement" != 0 ]] || fail "$account's entitlement never became claimable"
  entitled_before+=("$entitlement")
done

step "claim for both and withdraw 100 USDC from each through the console"
vault_before="$(usdc_of "$dev_stack_vault")"
wallet_before=("$(usdc_of "${managed[0]}")" "$(usdc_of "${managed[1]}")")
balance_before=("$(node_balance "${managed[0]}")" "$(node_balance "${managed[1]}")")
nonce_before=("$(nonce_of "${managed[0]}")" "$(nonce_of "${managed[1]}")")
browser settle
for i in 0 1; do
  account="${managed[$i]}"
  minted="$(monkey_of "$account")"
  [[ "$minted" == "$(claimed_of "$account")" ]] || fail "$account holds $minted MONKEY, the vault records $(claimed_of "$account")"
  python3 -c 'import sys; sys.exit(0 if int(sys.argv[1]) >= int(sys.argv[2]) > 0 else 1)' "$minted" "${entitled_before[$i]}" \
    || fail "$account holds $minted MONKEY, less than its entitlement ${entitled_before[$i]}"
  [[ "$(nonce_of "$account")" == "${nonce_before[$i]}" ]] || fail "$account sent a transaction to claim or withdraw"
  [[ "$(usdc_of "$account")" == $(( wallet_before[i] + 100000000 )) ]] || fail "$account's wallet was not paid exactly 100 USDC"
  expected="$(python3 -c 'import sys; from decimal import Decimal as D; print(D(sys.argv[1]) - 100)' "${balance_before[$i]}")"
  same "$(node_balance "$account")" "$expected" || fail "$account's balance is $(node_balance "$account"), not $expected"
done
[[ "$(usdc_of "$dev_stack_vault")" == $(( vault_before - 200000000 )) ]] || fail "the vault did not pay out exactly 200 USDC"
[[ "$(monkey_of "$funding")" == 0 ]] || fail "the funding key holds MONKEY"

step "stop the bot: it exits 0"
stop_bot
! grep -qE 'alert="?(node_signer_mismatch|bot_halted|console_failed)' "$work"/bot*.log || fail "the bot halted"

echo "PASS: two accounts declared, deposited into (one from its own wallet, one from the funding wallet), claimed for and withdrawn from through the console, matching the chain"
