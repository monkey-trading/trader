#!/usr/bin/env bash
# Dev scenario: one account trades BTC-USD with an SMA crossover against a dev node
# (scripts/dev-stack.sh), with the mock feed's random walk as the market:
#   1. the account is funded with a dev deposit and declared through the synthetic access source,
#      and the operator records a MONKEY emission schedule that starts at once;
#   2. the bot runs on one-minute bars, a one-bar average over a two-bar one, for TRADE_SECONDS
#      (default 900), and is stopped with SIGTERM half a minute into a bar; it exits 0;
#   3. the account's history holds fills on both sides, and they are exactly the fills the bot logged;
#   4. stopping cancelled every resting order and closed the position;
#   5. if the first ten-minute MONKEY period has closed and the account's fills in it realised a
#      loss, the period's allocation is in the account's history and its entitlement in
#      GET /v1/accounts/{address}/emissions.
#
# Needs what scripts/dev-stack.sh needs. TRADER_BIN runs a built trader instead of building one;
# SCENARIO_ARTIFACTS keeps the node's and the bot's logs and configs.
set -Eeuo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
source "$root/scripts/dev-stack.sh"
trade_seconds="${TRADE_SECONDS:-900}"
# Anvil's published account 1, whose private key the engine's dev scenarios use too.
key="59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"
account="0x70997970C51812dc3A010C7d01b50e0d17dc79C8"
period_ms=600000
bot_pid=""

cleanup() {
  if [[ -n "$bot_pid" ]]; then kill "$bot_pid" 2>/dev/null || true; wait "$bot_pid" 2>/dev/null || true; fi
  if [[ -n "${SCENARIO_ARTIFACTS:-}" && -n "${dev_stack_work:-}" ]]; then
    mkdir -p "$SCENARIO_ARTIFACTS"
    cp "$dev_stack_work/bot.log" "$SCENARIO_ARTIFACTS/" 2>/dev/null || true
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
account_json() { curl -sfk --max-time 5 "$api/v1/accounts/$account"; }

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

step "fund and declare the account"
dev_stack_fund "$account" 100000
dev_stack_declare "$key" "$account"
for _ in $(seq 1 100); do
  [[ "$(account_json 2>/dev/null | jq -r '.balance // "0" | tonumber | floor' 2>/dev/null || echo 0)" -ge 100000 ]] && break
  sleep 0.2
done
[[ "$(account_json | jq -r '.balance | tonumber | floor')" -ge 100000 ]] || fail "the deposit was not credited"

step "record a MONKEY emission schedule that starts in ten seconds"
start_ms=$(( $(now_ms) / 1000 * 1000 + 10000 ))
start_at="$(python3 -c 'import datetime, sys; print(datetime.datetime.fromtimestamp(int(sys.argv[1]) / 1000, datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"))' "$start_ms")"
dev_stack_admin init-emissions --start "$start_at" --period-budget 1000000000000000000000

step "write the bot's config: BTC-USD, 1m bars, fast 1 over slow 2, 50,000 USD a side, close on stop"
printf '%s\n' "$key" > "$work/account.hex"
chmod 600 "$work/account.hex"
{
  dev_stack_node_config
  cat <<CONFIG

[[account]]
key = { type = "hex_file", path = "account.hex" }
close_on_stop = true

[[account.strategy]]
market = "BTC-USD"
bar = "1m"
kind = "sma_crossover"
fast = 1
slow = 2
max_slippage_bps = 25

[sizing]
by = "fixed_notional"
notional = "50000"
max_leverage = "5"

[risk]
max_open_positions = 1
max_total_notional = "200000"
daily_loss = { usd = "100000" }
margin_buffer_bps = 10000
min_hold = "0s"

[logging]
level = "info"
CONFIG
} > "$work/bot.toml"

step "run the bot for ${trade_seconds}s"
"$trader" run --config "$work/bot.toml" > "$work/bot.log" 2>&1 &
bot_pid=$!
end=$(( SECONDS + trade_seconds ))
while (( SECONDS < end )); do
  kill -0 "$bot_pid" 2>/dev/null || fail "the bot exited while trading"
  sleep 1
done
# Orders go out as a bar closes on the minute; stop half a minute away from that.
sleep "$(python3 -c 'import time; print((30 - time.time() % 60) % 60)')"

step "stop the bot: it cancels resting orders, closes the position, and exits 0"
kill -TERM "$bot_pid"
status=""
for _ in $(seq 1 600); do
  if ! kill -0 "$bot_pid" 2>/dev/null; then
    wait "$bot_pid" && status=0 || status=$?
    break
  fi
  sleep 0.1
done
bot_pid_done="$bot_pid"
bot_pid=""
[[ "$status" == 0 ]] || fail "the bot (pid $bot_pid_done) exited with '${status:-no exit within a minute}'"
grep -q 'stream connected' "$work/bot.log" || fail "the bot never connected its stream"
! grep -qE 'alert="?(node_signer_mismatch|bot_halted)' "$work/bot.log" || fail "the bot halted"

step "the history's fills are the bot's, on both sides"
history_fills() {
  local before="" page
  while :; do
    page="$(curl -sfk --max-time 5 "$api/v1/accounts/$account/history?kind=fill&limit=200${before:+&before=$before}")"
    jq -r '.events[] | "\(.order_id) \(.side) \(.qty) \(.price)"' <<< "$page"
    before="$(jq -r '.next // empty' <<< "$page")"
    [[ -n "$before" ]] || return 0
  done
}
history_fills | sort -u > "$work/history-fills.txt"
sed -nE 's/.*: (fill|order filled) .*order_id=([0-9]+) .*side="?([A-Za-z]+)"? qty="?([^ "]+)"? price="?([^ "]+)"?.*/\2 \3 \4 \5/p' \
  "$work/bot.log" | tr '[:upper:]' '[:lower:]' | sort -u > "$work/bot-fills.txt"
diff "$work/history-fills.txt" "$work/bot-fills.txt" || fail "the bot's logged fills are not the history's"
buys="$(grep -c ' buy ' "$work/history-fills.txt" || true)"
sells="$(grep -c ' sell ' "$work/history-fills.txt" || true)"
echo "fills: $buys buys and $sells sells, each in the bot's log"
(( buys > 0 && sells > 0 )) || fail "expected fills on both sides"

step "no order rests and no position is left"
left="$(account_json)"
[[ "$(jq '.orders | length' <<< "$left")" == 0 ]] || fail "orders rest after the stop: $(jq -c .orders <<< "$left")"
[[ "$(jq -r '.positions["1"].qty // "0" | tonumber' <<< "$left")" == 0 ]] || fail "a position is left: $(jq -c .positions <<< "$left")"

step "the first MONKEY period's entitlement"
period_end=$(( start_ms + period_ms ))
if (( $(now_ms) < period_end + 5000 )); then
  echo "the first period ends at $period_end, after the run: nothing to check"
else
  losses="$(curl -sfk --max-time 5 "$api/v1/accounts/$account/history?kind=fill&limit=200" | jq --argjson from "$start_ms" \
    --argjson to "$period_end" '[.events[] | select(.ts >= $from and .ts < $to and (.realised_pnl | tonumber) < 0)] | length')"
  allocated="$(curl -sfk --max-time 5 "$api/v1/accounts/$account/history?kind=emission_allocated&limit=200" \
    | jq -r '[.events[] | select(.period == 0) | .amount] | first // "none"')"
  if (( losses == 0 )); then
    [[ "$allocated" == none ]] || fail "period 0 allocated $allocated to an account with no realised loss"
    echo "the account realised no loss in the first period, so it earned no entitlement"
  else
    [[ "$allocated" != none && "$allocated" != 0 ]] || fail "$losses losing fills in period 0, but no allocation"
    entitled=""
    for _ in $(seq 1 180); do
      entitled="$(curl -sk --max-time 5 "$api/v1/accounts/$account/emissions" | jq -r '.newest_entitled // empty' 2>/dev/null || true)"
      [[ -n "$entitled" ]] && python3 -c 'import sys; sys.exit(0 if int(sys.argv[1]) >= int(sys.argv[2]) else 1)' \
        "$entitled" "$allocated" && break
      sleep 1
    done
    python3 -c 'import sys; sys.exit(0 if int(sys.argv[1] or 0) >= int(sys.argv[2]) > 0 else 1)' "$entitled" "$allocated" \
      || fail "the entitlement '$entitled' does not hold period 0's allocation $allocated"
    echo "$losses losing fills in the first period: allocated $allocated, entitled $entitled"
  fi
fi

echo "PASS: the bot traded both ways, logged every fill the account's history holds, and stopped clean"
