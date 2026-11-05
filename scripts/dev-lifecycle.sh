#!/usr/bin/env bash
# Dev scenario: three accounts from empty to trading and back, on the chain dev stack
# (scripts/dev-stack.sh --chain), with the bot's `trader accounts` commands:
#   1. three keys get mock USDC and ether on Anvil, and a funding key 1,000 USDC and ether for gas;
#   2. each account declares (`declare --confirm`) and receives 1,000 USDC: the first two from their
#      own wallets (`deposit`), the third from the funding key's (`deposit --from-funding`, the
#      vault's `depositFor`); each payer's wallet, the vault and each account's balance in the node
#      move by exactly that;
#   3. the operator records a MONKEY emission schedule, and one bot process trades all three in
#      BTC-USD until each has an entitlement, then stops, closing its positions;
#   4. once each entitlement is claimable, `claim` claims all three through the funding key: each
#      account holds the MONKEY the vault records as claimed, sent no transaction itself, and the
#      funding key holds none;
#   5. `withdraw --account <first> --amount 100` pays 100 USDC to that account's own wallet, through
#      the funding key: the wallet, the vault and the node's balance each move by exactly 100.
#
# Needs what `scripts/dev-stack.sh --chain` needs. TRADER_BIN runs a built trader instead of
# building one; SCENARIO_ARTIFACTS keeps the node's and the bot's logs and configs.
set -Eeuo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
source "$root/scripts/dev-stack.sh"
# Anvil's published accounts 2, 3 and 4 are the accounts, and 5 the funding key; the engine's own
# scenarios use them too.
keys=(
  5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a
  7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6
  47e179ec197488593b187f80a00eb0da91f1b9d0b13f8733639f19c30a34926a
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
  [[ -f "${dev_stack_work:-}/bot.log" ]] && tail -n 40 "$dev_stack_work/bot.log" >&2
  exit 1
}
trap 'fail "the command at line $LINENO failed"' ERR
now_ms() { python3 -c 'import time; print(int(time.time() * 1000))'; }
node_get() { curl -sfk --max-time 5 "$dev_stack_api$1"; }
# An account's balance in the node, or "none" before its first credit.
node_balance() { node_get "/v1/accounts/$1" | jq -r '.balance // "none"' 2>/dev/null || echo none; }
usdc_of() { cast call --rpc-url "$dev_stack_rpc" "$dev_stack_usdc" 'balanceOf(address)(uint256)' "$1" | cut -d' ' -f1; }
monkey_of() { cast call --rpc-url "$dev_stack_rpc" "$dev_stack_monkey" 'balanceOf(address)(uint256)' "$1" | cut -d' ' -f1; }
claimed_of() { cast call --rpc-url "$dev_stack_rpc" "$dev_stack_vault" 'emissionsClaimed(address)(uint256)' "$1" | cut -d' ' -f1; }
nonce_of() { cast nonce --rpc-url "$dev_stack_rpc" "$1"; }
# Runs `trader accounts` on the scenario's config, logging to bot-accounts.log.
accounts() { "$trader" accounts --config "$work/bot.toml" "$@" 2>&1 | tee -a "$work/bot-accounts.log"; }
# Whether a and b, decimal strings, are equal as numbers.
same() { python3 -c 'import sys; from decimal import Decimal as D; sys.exit(0 if D(sys.argv[1]) == D(sys.argv[2]) else 1)' "$1" "$2"; }

step "build the bot"
if [[ -n "${TRADER_BIN:-}" ]]; then
  trader="$TRADER_BIN"
else
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

step "give each account 1,500 USDC and 1 ETH, and the funding key 1,000 USDC and 1 ETH"
for account in "${addresses[@]}"; do dev_stack_wallet "$account" 1500 1; done
dev_stack_wallet "$funding" 1000 1

step "write the bot's config: three accounts trading BTC-USD, and the funding key"
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
fast = 1
slow = 2
qty = "0.01"
max_slippage_bps = 25
CONFIG
  done
  printf '%s\n' "$funding_key" > "$work/funding.hex"
  cat <<CONFIG

[funding]
key = { type = "hex_file", path = "funding.hex" }

[logging]
level = "info"
CONFIG
} > "$work/bot.toml"
chmod 600 "$work"/*.hex

step "declare each account; deposit 1,000 USDC into the first two from their wallets, and the third from the funding key's"
confirm=()
for account in "${addresses[@]}"; do confirm+=(--confirm "$account"); done
accounts declare --class professional "${confirm[@]}" < /dev/null
for account in "${addresses[@]}"; do
  [[ "$(node_get "/v1/accounts/$account/eligibility" | jq -r .class)" == professional ]] || fail "$account did not declare"
done
# The vault already holds the house's capital.
vault_before="$(usdc_of "$dev_stack_vault")"
accounts --account "${addresses[0]}" --account "${addresses[1]}" deposit --amount 1000 < /dev/null
accounts --account "${addresses[2]}" deposit --amount 1000 --from-funding < /dev/null
# What each wallet holds from here on: the third's own USDC stays in it.
wallets=(500000000 500000000 1500000000)
for i in "${!addresses[@]}"; do
  account="${addresses[$i]}"
  same "$(node_balance "$account")" 1000 || fail "$account's balance is $(node_balance "$account"), not 1000"
  [[ "$(usdc_of "$account")" == "${wallets[$i]}" ]] || fail "$account's wallet holds $(usdc_of "$account") USDC base units"
done
[[ "$(usdc_of "$funding")" == 0 ]] || fail "the funding key holds $(usdc_of "$funding") USDC base units after its deposit"
[[ "$(usdc_of "$dev_stack_vault")" == $(( vault_before + 3000000000 )) ]] \
  || fail "the vault holds $(usdc_of "$dev_stack_vault") USDC base units, not 3,000 USDC more"
accounts status < /dev/null

step "record a MONKEY emission schedule that starts in ten seconds"
start_ms=$(( $(now_ms) / 1000 * 1000 + 10000 ))
start_at="$(python3 -c 'import datetime, sys; print(datetime.datetime.fromtimestamp(int(sys.argv[1]) / 1000, datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"))' "$start_ms")"
dev_stack_admin init-emissions --start "$start_at" --period-budget "$dev_stack_period_budget"

step "trade all three until each has a MONKEY entitlement (at most three periods)"
"$trader" run --config "$work/bot.toml" > "$work/bot.log" 2>&1 &
bot_pid=$!
deadline=$(( start_ms + 3 * period_ms + 60000 ))
entitled() { node_get "/v1/accounts/$1/emissions" 2>/dev/null | jq -r '.newest_entitled // "0"' 2>/dev/null || echo 0; }
while :; do
  kill -0 "$bot_pid" 2>/dev/null || fail "the bot exited while trading"
  missing=0
  for account in "${addresses[@]}"; do [[ "$(entitled "$account")" != 0 ]] || missing=$((missing + 1)); done
  (( missing == 0 )) && break
  (( $(now_ms) < deadline )) || fail "$missing accounts realised no loss in three periods"
  sleep 10
done
# Orders go out as a bar closes on the minute; stop half a minute away from that.
sleep "$(python3 -c 'import time; print((30 - time.time() % 60) % 60)')"
kill -TERM "$bot_pid"
status=""
for _ in $(seq 1 1200); do
  if ! kill -0 "$bot_pid" 2>/dev/null; then wait "$bot_pid" && status=0 || status=$?; break; fi
  sleep 0.1
done
bot_pid=""
[[ "$status" == 0 ]] || fail "the bot exited with '${status:-no exit within two minutes}'"
for account in "${addresses[@]}"; do
  [[ "$(node_get "/v1/accounts/$account" | jq '[.positions[] | select((.qty | tonumber) != 0)] | length')" == 0 ]] \
    || fail "$account holds a position after the stop"
done

step "claim each account's MONKEY through the funding key once claimable"
claimable() { node_get "/v1/accounts/$1/emissions" 2>/dev/null | jq -r 'select(.claimable) | .entitled' 2>/dev/null || true; }
entitled_before=()
nonce_before=()
for account in "${addresses[@]}"; do
  entitlement=""
  for _ in $(seq 1 300); do
    entitlement="$(claimable "$account")"
    [[ -n "$entitlement" && "$entitlement" != 0 ]] && break
    sleep 1
  done
  [[ -n "$entitlement" && "$entitlement" != 0 ]] || fail "$account's entitlement never became claimable"
  entitled_before+=("$entitlement")
  nonce_before+=("$(nonce_of "$account")")
done
funding_nonce="$(nonce_of "$funding")"
accounts claim < /dev/null
for i in "${!addresses[@]}"; do
  account="${addresses[$i]}"
  minted="$(monkey_of "$account")"
  [[ "$minted" == "$(claimed_of "$account")" ]] || fail "$account holds $minted MONKEY, the vault records $(claimed_of "$account")"
  python3 -c 'import sys; sys.exit(0 if int(sys.argv[1]) >= int(sys.argv[2]) > 0 else 1)' "$minted" "${entitled_before[$i]}" \
    || fail "$account holds $minted MONKEY, less than its entitlement ${entitled_before[$i]}"
  [[ "$(nonce_of "$account")" == "${nonce_before[$i]}" ]] || fail "$account sent a transaction to claim"
  echo "$account: claimed $minted MONKEY base units"
done
[[ "$(monkey_of "$funding")" == 0 ]] || fail "the funding key holds MONKEY"
[[ "$(nonce_of "$funding")" == $(( funding_nonce + ${#addresses[@]} )) ]] || fail "the funding key did not send one claim per account"

step "withdraw 100 USDC from the first account to its own wallet"
first="${addresses[0]}"
balance_before="$(node_balance "$first")"
wallet_before="$(usdc_of "$first")"
vault_before="$(usdc_of "$dev_stack_vault")"
accounts withdraw --account "$first" --amount 100 < /dev/null
[[ "$(usdc_of "$first")" == $(( wallet_before + 100000000 )) ]] || fail "$first's wallet holds $(usdc_of "$first"), not 100 USDC more"
[[ "$(usdc_of "$dev_stack_vault")" == $(( vault_before - 100000000 )) ]] || fail "the vault paid out other than 100 USDC"
expected="$(python3 -c 'import sys; from decimal import Decimal as D; print(D(sys.argv[1]) - 100)' "$balance_before")"
same "$(node_balance "$first")" "$expected" || fail "$first's balance is $(node_balance "$first"), not $expected"
for i in 1 2; do
  [[ "$(usdc_of "${addresses[$i]}")" == "${wallets[$i]}" ]] || fail "${addresses[$i]}'s wallet changed"
done
accounts status < /dev/null
! grep -qE 'alert="?(node_signer_mismatch|bot_halted)' "$work/bot.log" || fail "the bot halted"

echo "PASS: three accounts declared, deposited (one through the funding key), traded, claimed MONKEY through the funding key, and one withdrew to its own wallet"
