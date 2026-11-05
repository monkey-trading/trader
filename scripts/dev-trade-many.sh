#!/usr/bin/env bash
# Dev scenario: twenty accounts in one bot process trade BTC-USD and ETH-USD on two SMA strategies
# against a dev node (scripts/dev-stack.sh), with the mock feed's random walk as the markets:
#   1. each account is funded with a dev deposit and declared through the synthetic access source;
#   2. one bot runs all twenty on one-minute bars for TRADE_SECONDS (default 900): accounts 0-4 trade
#      BTC-USD on a 1/2 crossover, 5-9 BTC-USD on 2/3, 10-14 ETH-USD on 1/2, 15-19 ETH-USD on 2/3, and
#      account 0 also trades ETH-USD on 2/3; it is stopped with SIGTERM half a minute into a bar and
#      exits 0;
#   3. the bot opened one stream connection and few others, and the node refused no request for rate;
#   4. every account's fills in its history are exactly the fills the bot logged for it, and the
#      accounts as a whole filled on both sides in both markets;
#   5. stopping cancelled every resting order and closed every position.
#
# Needs what scripts/dev-stack.sh needs, and lsof. TRADER_BIN runs a built trader instead of
# building one; SCENARIO_ARTIFACTS keeps the node's and the bot's logs and configs.
set -Eeuo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
source "$root/scripts/dev-stack.sh"
trade_seconds="${TRADE_SECONDS:-900}"
accounts=20
bot_pid=""

cleanup() {
  if [[ -n "$bot_pid" ]]; then kill "$bot_pid" 2>/dev/null || true; wait "$bot_pid" 2>/dev/null || true; fi
  if [[ -n "${SCENARIO_ARTIFACTS:-}" && -n "${dev_stack_work:-}" ]]; then
    mkdir -p "$SCENARIO_ARTIFACTS"
    cp "$dev_stack_work/bot.log" "$dev_stack_work/bot.toml" "$SCENARIO_ARTIFACTS/" 2>/dev/null || true
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
account_json() { curl -sfk --max-time 5 "$api/v1/accounts/$1"; }

step "build the bot"
if [[ -n "${TRADER_BIN:-}" ]]; then
  trader="$TRADER_BIN"
else
  read -r -a cargo_cmd <<< "${DEV_STACK_CARGO:-cargo}"
  "${cargo_cmd[@]}" build -q --locked -p trader --manifest-path "$root/Cargo.toml"
  trader="$(cargo metadata --format-version 1 --no-deps --manifest-path "$root/Cargo.toml" | jq -r .target_directory)/debug/trader"
fi
[[ -x "$trader" ]] || fail "no trader binary at $trader"

step "start the dev stack"
dev_stack_start
api="$dev_stack_api"
work="$dev_stack_work"
port="${api##*:}"

step "write the bot's config: $accounts accounts, keys 0x1001 to 0x$(printf '%x' $((0x1000 + accounts)))"
{
  dev_stack_node_config
  for i in $(seq 0 $((accounts - 1))); do
    printf '%064x\n' $((0x1001 + i)) > "$work/account$i.hex"
    chmod 600 "$work/account$i.hex"
    market="BTC-USD"; (( i >= 10 )) && market="ETH-USD"
    periods="fast = 1
slow = 2"
    (( i % 10 >= 5 )) && periods="fast = 2
slow = 3"
    cat <<CONFIG

[[account]]
key = { type = "hex_file", path = "account$i.hex" }
close_on_stop = true

[[account.strategy]]
market = "$market"
bar = "1m"
$periods
qty = "0.5"
max_slippage_bps = 25
CONFIG
    if (( i == 0 )); then
      cat <<CONFIG

[[account.strategy]]
market = "ETH-USD"
bar = "1m"
fast = 2
slow = 3
qty = "1"
max_slippage_bps = 25
CONFIG
    fi
  done
  printf '\n[logging]\nlevel = "info"\n'
} > "$work/bot.toml"
addresses=()
while IFS= read -r line; do addresses+=("$line"); done < <("$trader" addresses --config "$work/bot.toml")
(( ${#addresses[@]} == accounts )) || fail "expected $accounts addresses, got ${#addresses[@]}"

step "fund and declare the accounts"
for i in $(seq 0 $((accounts - 1))); do
  dev_stack_fund "${addresses[$i]}" 100000
  dev_stack_declare "$(cat "$work/account$i.hex")" "${addresses[$i]}"
done
for address in "${addresses[@]}"; do
  for _ in $(seq 1 100); do
    [[ "$(account_json "$address" 2>/dev/null | jq -r '.balance // "0" | tonumber | floor' 2>/dev/null || echo 0)" -ge 100000 ]] && break
    sleep 0.2
  done
  [[ "$(account_json "$address" | jq -r '.balance | tonumber | floor')" -ge 100000 ]] || fail "the deposit for $address was not credited"
done

step "run the bot for ${trade_seconds}s"
"$trader" run --config "$work/bot.toml" > "$work/bot.log" 2>&1 &
bot_pid=$!
connections=0
end=$(( SECONDS + trade_seconds ))
while (( SECONDS < end )); do
  kill -0 "$bot_pid" 2>/dev/null || fail "the bot exited while trading"
  open="$(lsof -a -p "$bot_pid" -iTCP:"$port" -sTCP:ESTABLISHED -Fn 2>/dev/null | grep -c '^n' || true)"
  (( open > connections )) && connections=$open
  sleep 1
done
# Orders go out as a bar closes on the minute; stop half a minute away from that.
sleep "$(python3 -c 'import time; print((30 - time.time() % 60) % 60)')"

step "stop the bot: it cancels resting orders, closes positions, and exits 0"
kill -TERM "$bot_pid"
status=""
for _ in $(seq 1 1800); do
  if ! kill -0 "$bot_pid" 2>/dev/null; then
    wait "$bot_pid" && status=0 || status=$?
    break
  fi
  sleep 0.1
done
bot_pid_done="$bot_pid"
bot_pid=""
[[ "$status" == 0 ]] || fail "the bot (pid $bot_pid_done) exited with '${status:-no exit within three minutes}'"
! grep -qE 'alert="?(node_signer_mismatch|bot_halted|account_failed|account_shared|stop_incomplete)' "$work/bot.log" \
  || fail "an account failed or the bot halted"

step "one stream connection, few others, and no request refused for rate"
streams="$(grep -c 'stream connected' "$work/bot.log" || true)"
[[ "$streams" == 1 ]] || fail "the bot connected its stream $streams times"
echo "open connections to the node at most $connections (the stream and the bot's pool of 4)"
(( connections <= 5 )) || fail "$connections connections to the node for $accounts accounts"
requests_line="$(grep 'requests to the node' "$work/bot.log" | tail -n 1)"
echo "$requests_line"
grep -qE 'retry_later=0( |$)' <<< "$requests_line" || fail "the node asked the bot to retry later: $requests_line"
! grep -q 'asks to try again' "$work/bot.log" || fail "a request was answered with a retry-later"

step "every account's history fills are the bot's, and both sides filled in both markets"
history_fills() {
  local before="" page
  while :; do
    page="$(curl -sfk --max-time 5 "$api/v1/accounts/$1/history?kind=fill&limit=200${before:+&before=$before}")"
    jq -r '.events[] | "\(.order_id) \(.market) \(.side) \(.qty) \(.price)"' <<< "$page"
    before="$(jq -r '.next // empty' <<< "$page")"
    [[ -n "$before" ]] || return 0
  done
}
: > "$work/all-fills.txt"
for address in "${addresses[@]}"; do
  lower="$(tr '[:upper:]' '[:lower:]' <<< "$address")"
  history_fills "$address" | sort -u > "$work/history-fills.txt"
  sed -nE 's/.*: (fill|order filled) .*account=(0x[0-9a-fA-F]+) order_id=([0-9]+) market=([0-9]+) side=([A-Za-z]+) qty=([^ ]+) price=([^ ]+).*/\2 \3 \4 \5 \6 \7/p' \
    "$work/bot.log" | tr '[:upper:]' '[:lower:]' | awk -v a="$lower" '$1 == a { print $2, $3, $4, $5, $6 }' | sort -u > "$work/bot-fills.txt"
  diff "$work/history-fills.txt" "$work/bot-fills.txt" > /dev/null || {
    diff "$work/history-fills.txt" "$work/bot-fills.txt" >&2 || true
    fail "the bot's logged fills for $address are not its history's"
  }
  cat "$work/history-fills.txt" >> "$work/all-fills.txt"
done
for market in 1 2; do
  for side in buy sell; do
    n="$(awk -v m="$market" -v s="$side" '$2 == m && $3 == s' "$work/all-fills.txt" | wc -l | tr -d ' ')"
    echo "market $market $side fills: $n"
    (( n > 0 )) || fail "no $side fills in market $market"
  done
done

step "no order rests and no position is left"
for address in "${addresses[@]}"; do
  left="$(account_json "$address")"
  [[ "$(jq '.orders | length' <<< "$left")" == 0 ]] || fail "orders rest for $address: $(jq -c .orders <<< "$left")"
  [[ "$(jq '[.positions[]?.qty | tonumber | select(. != 0)] | length' <<< "$left")" == 0 ]] \
    || fail "a position is left for $address: $(jq -c .positions <<< "$left")"
done

echo "PASS: $accounts accounts traded both ways in both markets over one stream, each fill logged as the history holds it, and stopped clean"
