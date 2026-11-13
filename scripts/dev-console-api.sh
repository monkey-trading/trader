#!/usr/bin/env bash
# Dev scenario: the bot's console API against a dev node (scripts/dev-stack.sh):
#   1. the console app is built and embedded in the bot, which runs two accounts with its console on
#      a free loopback port, both trading BTC-USD on one-minute bars for TRADE_SECONDS (default 180);
#   2. the bot printed its certificate's SHA-256 fingerprint, which is the served certificate's, and
#      a password, which logs in to a session cookie that is HttpOnly, Secure and SameSite=Strict;
#   3. without the cookie, or from a foreign Origin, the API refuses, and the live feed too;
#   4. stopping the second account through the console stops its strategy: no fill for two more bars,
#      while the first runs on;
#   5. with both stopped and settled, each account's figures, fills and MONKEY entitlement through the console match the node's
#      answers, read when the account has settled;
#   6. `/` serves the embedded console, and the bot stops on SIGTERM with exit 0.
#
# Needs what scripts/dev-stack.sh needs, and Node.js with npm for the console's build. TRADER_BIN
# runs a built trader instead (built with the console); SCENARIO_ARTIFACTS keeps the logs.
set -Eeuo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
source "$root/scripts/dev-stack.sh"
trade_seconds="${TRADE_SECONDS:-180}"
# Two of the dev keys scripts/dev-trade-many.sh trades, whose private keys the repository publishes.
keys=("$(printf '%064x' $((0x1001)))" "$(printf '%064x' $((0x1002)))")
accounts=()
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
node_get() { curl -sfk --max-time 5 "$api$1"; }
# Every fill in an account's history, newest first, one JSON object a line.
history_fills() {
  local before="" page
  while :; do
    page="$(node_get "/v1/accounts/$1/history?kind=fill&limit=200${before:+&before=$before}")"
    jq -c '.events[]' <<< "$page"
    before="$(jq -r '.next // empty' <<< "$page")"
    [[ -n "$before" ]] || return 0
  done
}
# console_status <method> <path> [curl options...]: the status of a call to the console, whose body
# is left in $work/body.
console_status() {
  local method="$1" path="$2"
  shift 2
  curl -sk --max-time 130 -X "$method" -o "$work/body" -w '%{http_code}' "$@" "$console$path"
}
# The body of a GET of the console in the session, which must answer 200.
console_get() {
  local status
  status="$(console_status GET "$1" -b "$work/cookies")"
  [[ "$status" == 200 ]] || fail "GET $1 answered $status: $(cat "$work/body")"
  cat "$work/body"
}
# The status of a POST to the console in the session, from its own origin.
console_post() { console_status POST "$1" -b "$work/cookies" -H "Origin: $console"; }

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

step "start the dev stack"
dev_stack_start
api="$dev_stack_api"
work="$dev_stack_work"

# The sections every account shares: where the bot keeps its state, how it sizes, and its risk limits.
trading_sections() {
  cat <<CONFIG

[state]
dir = "state"

[sizing]
by = "fixed_notional"
notional = "20000"
max_leverage = "5"

[risk]
max_open_positions = 1
max_total_notional = "100000"
daily_loss = { usd = "1000000" }
margin_buffer_bps = 10000
min_hold = "0s"
CONFIG
}

step "read the accounts' addresses"
for i in "${!keys[@]}"; do
  printf '%s\n' "${keys[$i]}" > "$work/account$i.hex"
  chmod 600 "$work/account$i.hex"
done
cat > "$work/addresses.toml" <<CONFIG
$(dev_stack_node_config)
$(for i in "${!keys[@]}"; do printf '\n[[account]]\nkey = { type = "hex_file", path = "account%s.hex" }\n\n[[account.strategy]]\nmarket = "BTC-USD"\nbar = "1m"\nkind = "sma_crossover"\nfast = 1\nslow = 2\nmax_slippage_bps = 25\n' "$i"; done)
$(trading_sections)
CONFIG
while IFS= read -r line; do accounts+=("$(tr '[:upper:]' '[:lower:]' <<< "$line")"); done \
  < <("$trader" addresses --config "$work/addresses.toml")
(( ${#accounts[@]} == 2 )) || fail "expected two addresses, got ${#accounts[@]}"

step "fund and declare ${#accounts[@]} account(s)"
for i in "${!accounts[@]}"; do
  dev_stack_fund "${accounts[$i]}" 100000
  dev_stack_declare "${keys[$i]}" "${accounts[$i]}"
done
for account in "${accounts[@]}"; do
  for _ in $(seq 1 100); do
    [[ "$(node_get "/v1/accounts/$account" 2>/dev/null | jq -r '.balance // "0" | tonumber | floor' 2>/dev/null || echo 0)" -ge 100000 ]] && break
    sleep 0.2
  done
  [[ "$(node_get "/v1/accounts/$account" | jq -r '.balance | tonumber | floor')" -ge 100000 ]] || fail "$account was not credited"
done

step "write the bot's config: both accounts; its console's port is picked at each start"
# Writes the bot's config: the base file and its console on `$console_port`.
write_bot_config() {
  {
    cat "$work/bot-base.toml"
    cat <<CONFIG

[console]
bind = "127.0.0.1:$console_port"
data_dir = "console-data"
refresh_interval = "2s"
CONFIG
  } > "$work/bot.toml"
}
{
  dev_stack_node_config
  for i in "${!keys[@]}"; do
    cat <<CONFIG

[[account]]
key = { type = "hex_file", path = "account$i.hex" }
close_on_stop = true

[[account.strategy]]
market = "BTC-USD"
bar = "1m"
kind = "sma_crossover"
fast = 1
slow = 2
max_slippage_bps = 25
bounds = { fast = [1, 5], slow = [2, 20] }
CONFIG
  done
  trading_sections
  cat <<CONFIG

[logging]
level = "info"
CONFIG
} > "$work/bot-base.toml"

step "run the bot for ${trade_seconds}s"
# Starts the bot on a newly picked console port and waits for its console; a start that loses the port
# to another process is run again on a new one.
start_bot() {
  console_port="$(dev_stack_free_port)"
  console="https://127.0.0.1:$console_port"
  write_bot_config
  "$trader" run --config "$work/bot.toml" > "$work/bot.log" 2>&1 &
  bot_pid=$!
  for _ in $(seq 1 100); do
    grep -q 'console listening' "$work/bot.log" && return 0
    if ! kill -0 "$bot_pid" 2>/dev/null; then
      wait "$bot_pid" 2>/dev/null || true
      bot_pid=""
      if dev_stack_lost_port "$work/bot.log"; then dev_stack_port_lost=1; return 0; fi
      fail "the bot exited at startup"
    fi
    sleep 0.2
  done
  fail "the console never listened"
}
dev_stack_attempts start_bot

step "the printed fingerprint is the served certificate's, and the password logs in"
printed_fingerprint="$(sed -nE 's/^console certificate .*SHA-256 fingerprint ([0-9A-F:]+)$/\1/p' "$work/bot.log")"
served_fingerprint="$(python3 -c '
import hashlib, socket, ssl, sys
context = ssl.create_default_context()
context.check_hostname = False
context.verify_mode = ssl.CERT_NONE
with socket.create_connection(("127.0.0.1", int(sys.argv[1]))) as raw:
    with context.wrap_socket(raw) as tls:
        digest = hashlib.sha256(tls.getpeercert(binary_form=True)).hexdigest().upper()
print(":".join(digest[i:i + 2] for i in range(0, 64, 2)))' "$console_port")"
[[ -n "$printed_fingerprint" && "$printed_fingerprint" == "$served_fingerprint" ]] \
  || fail "printed fingerprint '$printed_fingerprint' is not the served '$served_fingerprint'"
password="$(sed -nE 's/^console password, shown once .*: ([a-z0-9]+)$/\1/p' "$work/bot.log")"
[[ ${#password} == 26 ]] || fail "no password was printed"
! grep -v '^console password' "$work/bot.log" | grep -qF "$password" || fail "the password appears in the log"
login="$(curl -sk --max-time 10 -o /dev/null -D - -c "$work/cookies" -H "Origin: $console" \
  -H 'content-type: application/json' -d "{\"password\":\"$password\"}" "$console/api/login")"
grep -q '^HTTP/1.1 204' <<< "$login" || fail "the login was refused: $login"
cookie_line="$(grep -i '^set-cookie:' <<< "$login")"
for flag in "__Host-session=" "HttpOnly" "Secure" "SameSite=Strict" "Path=/"; do
  grep -qF "$flag" <<< "$cookie_line" || fail "the session cookie lacks $flag: $cookie_line"
done
[[ "$(console_status POST /api/login -H "Origin: $console" -H 'content-type: application/json' \
  -d '{"password":"wrong"}')" == 401 ]] || fail "a wrong password logged in"

step "requests without the session, or from a foreign origin, are refused"
first="${accounts[0]}"
second="${accounts[1]}"
[[ "$(console_status GET /api/accounts)" == 401 ]] || fail "a read without the session was answered"
[[ "$(console_status POST "/api/accounts/$first/stop" -H "Origin: $console")" == 401 ]] \
  || fail "a stop without the session was taken"
for origin in "https://evil.example" "null"; do
  [[ "$(console_status POST "/api/accounts/$first/stop" -b "$work/cookies" -H "Origin: $origin")" == 403 ]] \
    || fail "a stop from $origin was taken"
done
[[ "$(console_status POST "/api/accounts/$first/stop" -b "$work/cookies")" == 403 ]] \
  || fail "a stop without an Origin was taken"
live_status() {
  curl -sk --max-time 5 -o /dev/null -w '%{http_code}' -b "$work/cookies" -H "Origin: $1" -H 'Connection: Upgrade' \
    -H 'Upgrade: websocket' -H 'Sec-WebSocket-Version: 13' -H 'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==' \
    "$console/api/live" || true
}
[[ "$(live_status https://evil.example)" == 403 ]] || fail "the live feed opened for a foreign origin"
[[ "$(live_status "$console")" == 101 ]] || fail "the live feed did not open for the console"
for account in "${accounts[@]}"; do
  [[ "$(console_get "/api/accounts/$account" | jq -r .state)" == running ]] || fail "$account is not running"
done

step "trade for ${trade_seconds}s"
end=$(( SECONDS + trade_seconds ))
while (( SECONDS < end )); do
  kill -0 "$bot_pid" 2>/dev/null || fail "the bot exited while trading"
  sleep 1
done

step "stop $second through the console; its strategy places nothing for two bars, and $first runs on"
[[ "$(console_post "/api/accounts/$second/stop")" == 204 ]] || fail "the stop was refused: $(cat "$work/body")"
[[ "$(console_get "/api/accounts/$second" | jq -r .state)" == stopped ]] || fail "$second is not stopped"
[[ "$(console_get "/api/accounts/$first" | jq -r .state)" == running ]] || fail "$first stopped too"
# An order signed just before the stop may still enter the log until it expires, 30 s on.
sleep 35
fills_before="$(history_fills "$second" | wc -l | tr -d ' ')"
sleep 125
fills_after="$(history_fills "$second" | wc -l | tr -d ' ')"
[[ "$fills_before" == "$fills_after" ]] || fail "$second filled while stopped: $fills_before then $fills_after fills"
[[ "$(console_get "/api/accounts/$second" | jq -r '.strategies[0].running and .state == "stopped"')" == true ]] \
  || fail "$second's status does not show it stopped"
echo "$second: $fills_after fills, none after the stop; $first: $(history_fills "$first" | wc -l | tr -d ' ') fills"

step "stop $first too, so no account's figures move while they are compared"
[[ "$(console_post "/api/accounts/$first/stop")" == 204 ]] || fail "the stop was refused: $(cat "$work/body")"
# An order signed just before the stop may still enter the log until it expires, 30 s on; the console
# then reads the node again within its refresh interval.
sleep 40

step "each account's figures through the console match the node's answers"
for account in "${accounts[@]}"; do
  view="$(console_get "/api/accounts/$account")"
  node_view="$(node_get "/v1/accounts/$account")"
  [[ "$(jq -r .error <<< "$view")" == null ]] || fail "$account: $(jq -r .error <<< "$view")"
  [[ "$(jq -r .balance <<< "$node_view")" == "$(jq -r .figures.balance <<< "$view")" ]] \
    || fail "$account: the balance differs: $(jq -c .figures <<< "$view") against $node_view"
  [[ "$(jq -c '[.positions | to_entries[] | select(.value.qty != "0") | {market: (.key | tonumber), qty: .value.qty, entry: .value.entry}]' <<< "$node_view")" \
    == "$(jq -c '[.figures.positions[] | {market, qty, entry}]' <<< "$view")" ]] || fail "$account: the positions differ"
  [[ "$(jq -c '[.orders[] | .order_id]' <<< "$node_view")" == "$(jq -c '[.figures.orders[] | .order_id]' <<< "$view")" ]] \
    || fail "$account: the orders differ"
  history_fills "$account" > "$work/fills.jsonl"
  realised="$(python3 -c '
import json, sys
from decimal import Decimal
fills = [json.loads(line) for line in open(sys.argv[1])]
print(sum((Decimal(f["realised_pnl"]) - Decimal(f["profit_fee"]) for f in fills), Decimal(0)))' "$work/fills.jsonl")"
  python3 -c 'import sys; from decimal import Decimal as D; sys.exit(0 if D(sys.argv[1]) == D(sys.argv[2]) else 1)' \
    "$(jq -r .figures.realised_pnl <<< "$view")" "$realised" || fail "$account: realised $(jq -r .figures.realised_pnl <<< "$view") is not $realised"
  [[ "$(head -n 50 "$work/fills.jsonl" | jq -sc '[.[] | .cursor]')" == "$(console_get "/api/accounts/$account/fills" | jq -c '[.fills[] | .cursor]')" ]] \
    || fail "$account: the fills differ"
  entitled="$(curl -sk --max-time 5 "$api/v1/accounts/$account/emissions" | jq -r '.entitled // "0"')"
  [[ "$entitled" == "$(jq -r .figures.monkey.entitled <<< "$view")" ]] || fail "$account: the entitlement differs"
  python3 -c 'import sys; from decimal import Decimal as D; sys.exit(0 if D(sys.argv[1]) - D(sys.argv[2]) == D(sys.argv[3]) else 1)' \
    "$(jq -r .figures.equity <<< "$view")" "$(jq -r .figures.balance <<< "$view")" "$(jq -r .figures.unrealised_pnl <<< "$view")" \
    || fail "$account: unrealised is not equity less balance"
  echo "$account: balance $(jq -r .figures.balance <<< "$view"), realised $realised over $(wc -l < "$work/fills.jsonl" | tr -d ' ') fills, entitled $entitled"
done

step "/ serves the embedded console"
[[ "$(console_status GET /)" == 200 ]] || fail "/ was not served"
if [[ -f "$root/console/dist/index.html" && -z "${TRADER_BIN:-}" ]]; then
  cmp -s "$work/body" "$root/console/dist/index.html" || fail "/ is not the console's built index.html"
fi

step "stop the bot: it exits 0"
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
! grep -qE 'alert="?(node_signer_mismatch|bot_halted|console_failed)' "$work/bot.log" || fail "the bot halted"

echo "PASS: the console logged in with the printed password only, refused requests without the session or from a foreign origin, stopped the account's strategy, and matched the node's answers"
