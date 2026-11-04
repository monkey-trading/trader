#!/usr/bin/env bash
# Dev scenario: one account per strategy trades BTC-USD on one-minute bars against a dev node
# (scripts/dev-stack.sh), with the mock feed's random walk as the market, and one more account has a
# daily loss limit of a hundredth of a cent:
#   1. each of seven accounts is funded and declared, and a bot runs for it for TRADE_SECONDS
#      (default 900): an SMA, an EMA and a MACD crossover, an RSI reversion, a Bollinger band
#      reversion, a Donchian breakout, and the SMA crossover with the tiny limit;
#   2. each of the six strategies' accounts trades, and its fills are exactly the fills its bot logged;
#   3. no account breaches its limits: no fill leaves a position above the total-notional limit or
#      is a close-out, no position is closed before its minimum hold, and cash stays positive;
#   4. the account with the tiny limit is flattened when the limit is reached, places no order after,
#      and is flat at the end;
#   5. stopping each bot cancels its resting orders and closes its position, and every bot exits 0.
#
# Needs what scripts/dev-stack.sh needs. TRADER_BIN runs a built trader instead of building one;
# SCENARIO_ARTIFACTS keeps the node's and the bots' logs and configs.
set -Eeuo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
source "$root/scripts/dev-stack.sh"
trade_seconds="${TRADE_SECONDS:-900}"
min_hold_seconds=120
total_notional=5000
# Anvil's published accounts 1 to 7, whose private keys the engine's dev scenarios use too.
names=(sma ema macd rsi bollinger donchian tiny)
keys=(
  59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d
  5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a
  7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6
  47e179ec197488593b187f80a00eb0da91f1b9d0b13f8733639f19c30a34926a
  8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba
  92db14e403b83dfe3df233f83dfa3a0d7096f21ca9b0d6d6b8d88b2b4ec1564e
  4bbbf85ce3377467afe5d46f804f221813b2bb87f24d81f60f1fcdbf7cbf4356
)
addresses=(
  0x70997970C51812dc3A010C7d01b50e0d17dc79C8
  0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC
  0x90F79bf6EB2c4f870365E785982E1f101E93b906
  0x15d34AAf54267DB7D7c367839AAf71A00a2C6A65
  0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc
  0x976EA74026E726554dB657fA54763abd0C3a0aa9
  0x14dC79964da2C08b23698B3D3cc7Ca32193d9955
)
# Each strategy, with parameters short enough to signal within a few one-minute bars.
strategies=(
  "kind = \"sma_crossover\"
fast = 1
slow = 2"
  "kind = \"ema_crossover\"
fast = 1
slow = 3"
  "kind = \"macd\"
fast = 2
slow = 4
signal = 2"
  "kind = \"rsi_reversion\"
period = 3
oversold = 40
exit = 50
overbought = 60"
  "kind = \"bollinger_reversion\"
period = 4
width = \"0.5\""
  "kind = \"donchian_breakout\"
entry = 3
exit = 1"
  "kind = \"sma_crossover\"
fast = 1
slow = 2"
)
sizings=(
  'by = "fixed_notional"
notional = "1000"
max_leverage = "5"'
  'by = "equity_fraction"
bps = 100
max_leverage = "5"'
)
pids=()
trader=""

cleanup() {
  local pid
  for pid in "${pids[@]:-}"; do
    [[ -n "$pid" ]] && { kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; }
  done
  if [[ -n "${SCENARIO_ARTIFACTS:-}" && -n "${dev_stack_work:-}" ]]; then
    mkdir -p "$SCENARIO_ARTIFACTS"
    cp "$dev_stack_work"/*.log "$dev_stack_work"/bot-*.toml "$SCENARIO_ARTIFACTS/" 2>/dev/null || true
  fi
  dev_stack_stop
}
trap cleanup EXIT
step() { echo "==> $*"; }
fail() {
  echo "FAIL: $*" >&2
  if [[ -n "${dev_stack_work:-}" ]]; then
    local log
    for log in "$dev_stack_work"/bot-*.log; do [[ -f "$log" ]] && { echo "--- $log" >&2; tail -n 15 "$log" >&2; }; done
  fi
  exit 1
}
trap 'fail "the command at line $LINENO failed"' ERR
now_ms() { python3 -c 'import time; print(int(time.time() * 1000))'; }
account_json() { curl -sfk --max-time 5 "$api/v1/accounts/$1"; }

# The account's fills, oldest first, one JSON object a line.
history_fills() {
  local before="" page
  while :; do
    page="$(curl -sfk --max-time 5 "$api/v1/accounts/$1/history?kind=fill&limit=200${before:+&before=$before}")"
    jq -c '.events[]' <<< "$page"
    before="$(jq -r '.next // empty' <<< "$page")"
    [[ -n "$before" ]] || return 0
  done | jq -s -c 'sort_by(.ts, .order_id)[]'
}

step "build the bot"
if [[ -n "${TRADER_BIN:-}" ]]; then
  trader="$TRADER_BIN"
else
  read -r -a cargo_cmd <<< "${DEV_STACK_CARGO:-cargo}"
  "${cargo_cmd[@]}" build -q --locked -p trader --manifest-path "$root/Cargo.toml"
  trader="$(cargo metadata --format-version 1 --no-deps --manifest-path "$root/Cargo.toml" | jq -r .target_directory)/debug/trader"
fi
[[ -x "$trader" ]] || fail "no trader binary at $trader"

# The daily loss limit is per UTC day: start after midnight when the run would cross it.
to_midnight=$(( 86400 - $(date -u +%s) % 86400 ))
if (( to_midnight < trade_seconds + 180 )); then
  step "wait ${to_midnight}s for the UTC day to turn, so the run stays inside one"
  sleep $(( to_midnight + 2 ))
fi

step "start the dev stack"
dev_stack_start
api="$dev_stack_api"
work="$dev_stack_work"

step "fund and declare the seven accounts"
for i in "${!names[@]}"; do
  dev_stack_fund "${addresses[$i]}" 100000
  dev_stack_declare "${keys[$i]}" "${addresses[$i]}"
done
for i in "${!names[@]}"; do
  for _ in $(seq 1 100); do
    [[ "$(account_json "${addresses[$i]}" 2>/dev/null | jq -r '.balance // "0" | tonumber | floor' 2>/dev/null || echo 0)" -ge 100000 ]] && break
    sleep 0.2
  done
  [[ "$(account_json "${addresses[$i]}" | jq -r '.balance | tonumber | floor')" -ge 100000 ]] || fail "${names[$i]}: the deposit was not credited"
done

step "write the bots' configs and run them for ${trade_seconds}s"
for i in "${!names[@]}"; do
  name="${names[$i]}"
  printf '%s\n' "${keys[$i]}" > "$work/$name.hex"
  chmod 600 "$work/$name.hex"
  loss='{ usd = "5000" }'
  [[ "$name" == tiny ]] && loss='{ usd = "0.0001" }'
  {
    dev_stack_node_config
    cat <<CONFIG

[account]
key = { type = "hex_file", path = "$name.hex" }
close_on_stop = true
state_dir = "state-$name"

[strategy]
market = "BTC-USD"
bar = "1m"
max_slippage_bps = 25
${strategies[$i]}

[sizing]
${sizings[$(( i % 2 ))]}

[risk]
max_open_positions = 1
max_total_notional = "$total_notional"
daily_loss = $loss
margin_buffer_bps = 10000
min_hold = "${min_hold_seconds}s"

[logging]
level = "info"
CONFIG
  } > "$work/bot-$name.toml"
  "$trader" run --config "$work/bot-$name.toml" > "$work/bot-$name.log" 2>&1 &
  pids[i]=$!
done
end=$(( SECONDS + trade_seconds ))
while (( SECONDS < end )); do
  for i in "${!names[@]}"; do
    kill -0 "${pids[$i]}" 2>/dev/null || fail "the ${names[$i]} bot exited while trading"
  done
  sleep 1
done
# Orders go out as a bar closes on the minute; stop half a minute away from that.
sleep "$(python3 -c 'import time; print((30 - time.time() % 60) % 60)')"

step "stop the bots: each cancels its resting orders, closes its position, and exits 0"
stop_ms="$(now_ms)"
for i in "${!names[@]}"; do kill -TERM "${pids[$i]}"; done
for i in "${!names[@]}"; do
  status=""
  for _ in $(seq 1 900); do
    if ! kill -0 "${pids[$i]}" 2>/dev/null; then
      wait "${pids[$i]}" && status=0 || status=$?
      break
    fi
    sleep 0.1
  done
  [[ "$status" == 0 ]] || fail "the ${names[$i]} bot exited with '${status:-no exit within 90s}'"
  pids[i]=""
  log="$work/bot-${names[$i]}.log"
  grep -q 'stream connected' "$log" || fail "the ${names[$i]} bot never connected its stream"
  ! grep -qE 'alert="?(node_signer_mismatch|bot_halted)' "$log" || fail "the ${names[$i]} bot halted"
done

for i in "${!names[@]}"; do
  name="${names[$i]}"
  address="${addresses[$i]}"
  log="$work/bot-$name.log"
  step "$name: fills are the bot's, and the limits hold"
  history_fills "$address" > "$work/$name-fills.jsonl"
  jq -r '"\(.order_id) \(.side) \(.qty) \(.price)"' "$work/$name-fills.jsonl" | sort -u > "$work/$name-history.txt"
  sed -nE 's/.*: (fill|order filled) .*order_id=([0-9]+) .*side=([A-Za-z]+) qty=([^ ]+) price=([^ ]+).*/\2 \3 \4 \5/p' \
    "$log" | tr '[:upper:]' '[:lower:]' | sort -u > "$work/$name-bot.txt"
  diff "$work/$name-history.txt" "$work/$name-bot.txt" || fail "$name: the bot's logged fills are not the history's"
  fills="$(wc -l < "$work/$name-fills.jsonl" | tr -d ' ')"
  echo "$name: $fills fills"
  if [[ "$name" != tiny ]]; then
    (( fills > 0 )) || fail "$name: the strategy never traded"
  fi
  python3 -I - "$work/$name-fills.jsonl" "$min_hold_seconds" "$total_notional" "$stop_ms" <<'PY' || fail "$name: a limit was breached"
import json, sys
path, min_hold_ms, total_notional, stop_ms = sys.argv[1], int(sys.argv[2]) * 1000, float(sys.argv[3]), int(sys.argv[4])
opened = None
for line in open(path):
    fill = json.loads(line)
    after = float(fill["position"])
    notional = abs(after) * float(fill["price"])
    assert not fill.get("liquidation"), f"a close-out at {fill['ts']}"
    assert notional <= total_notional + 1, f"a position of {notional} USD, over the limit {total_notional}"
    before = after - float(fill["qty"]) * (1 if fill["side"] == "buy" else -1)
    # The stop closes whatever is held, whenever it was opened.
    if before != 0 and (after == 0 or (after > 0) != (before > 0)) and fill["ts"] < stop_ms - 60000:
        assert opened is not None and fill["ts"] - opened >= min_hold_ms, (
            f"a position opened at {opened} was closed at {fill['ts']}, before {min_hold_ms} ms")
    if before == 0 or (after != 0 and (after > 0) != (before > 0)):
        opened = fill["ts"]
PY
  [[ "$(account_json "$address" | jq -r '.balance | tonumber > 0')" == true ]] || fail "$name: cash is not positive"
  left="$(account_json "$address")"
  [[ "$(jq '.orders | length' <<< "$left")" == 0 ]] || fail "$name: orders rest after the stop"
  [[ "$(jq -r '.positions["1"].qty // "0" | tonumber' <<< "$left")" == 0 ]] || fail "$name: a position is left"
done

step "the account with the tiny daily limit is flattened, then places nothing"
log="$work/bot-tiny.log"
trip="$(grep -n 'alert="daily_loss_limit"' "$log" | head -n 1 | cut -d: -f1 || true)"
[[ -n "$trip" ]] || fail "tiny: the daily loss limit was never reached"
grep -q 'closing the position' <(tail -n +"$trip" "$log") || fail "tiny: the position was not closed when the limit was reached"
! tail -n +"$((trip + 1))" "$log" | grep -q 'placing a market order' || fail "tiny: an order was placed after the limit was reached"
echo "tiny: flattened at log line $trip, and no order after it"

echo "PASS: six strategies traded within their limits, and the account with the tiny limit was flattened and stopped"
