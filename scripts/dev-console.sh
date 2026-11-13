#!/usr/bin/env bash
# Dev scenario: the console app against the bot's real console server and a dev node (scripts/dev-stack.sh):
#   1. the console app is built and embedded in the bot, which runs three accounts with its console on
#      a free loopback port, all trading BTC-USD on one-minute bars with parameter bounds;
#   2. after TRADE_SECONDS (default 120) of trading, the browser test (console/browser-test/run.mjs)
#      signs in with the printed password, checks each account's row against the API, opens each
#      account's page, stops and starts a strategy, flattens an account, and checks phone width and the
#      keyboard (its refusal-in-place check needs an account whose strategies refuse to start, which the
#      stand-in API has: `npm run stub`);
#   3. the Vite dev server, proxying to the bot, takes the same login and a session check, which the
#      bot's Origin check would refuse without the proxy's rewrite of the Origin;
#   4. the bot stops on SIGTERM with exit 0.
#
# Needs what scripts/dev-stack.sh needs, Node.js with npm, and Playwright 1.64.0 installed in a scratch
# directory outside the repository, named by PLAYWRIGHT_DIR (Playwright is no dependency of the repository;
# `npm install playwright@1.64.0 && npx playwright install chromium` there). TRADER_BIN runs a built trader
# instead (built with the console); SCENARIO_ARTIFACTS keeps the logs.
set -Eeuo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
source "$root/scripts/dev-stack.sh"
trade_seconds="${TRADE_SECONDS:-120}"
: "${PLAYWRIGHT_DIR:?set PLAYWRIGHT_DIR to a directory with playwright 1.64.0 installed}"
# Three of the dev keys scripts/dev-trade-many.sh trades, whose private keys the repository publishes.
keys=("$(printf '%064x' $((0x1001)))" "$(printf '%064x' $((0x1002)))" "$(printf '%064x' $((0x1003)))")
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
(( ${#accounts[@]} == 3 )) || fail "expected three addresses, got ${#accounts[@]}"

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

step "write the bot's config: all three accounts, with a console on a free loopback port"
console_port="$(dev_stack_free_port)"
console="https://127.0.0.1:$console_port"
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

[console]
bind = "127.0.0.1:$console_port"
data_dir = "console-data"
refresh_interval = "2s"

[logging]
level = "info"
CONFIG
} > "$work/bot.toml"

step "run the bot for ${trade_seconds}s"
"$trader" run --config "$work/bot.toml" > "$work/bot.log" 2>&1 &
bot_pid=$!
for _ in $(seq 1 100); do
  grep -q 'console listening' "$work/bot.log" && break
  kill -0 "$bot_pid" 2>/dev/null || fail "the bot exited at startup"
  sleep 0.2
done
grep -q 'console listening' "$work/bot.log" || fail "the console never listened"

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


step "trade for ${trade_seconds}s"
end=$(( SECONDS + trade_seconds ))
while (( SECONDS < end )); do
  kill -0 "$bot_pid" 2>/dev/null || fail "the bot exited while trading"
  sleep 1
done

step "run the browser test against the console"
CONSOLE_URL="$console" CONSOLE_PASSWORD="$password" PLAYWRIGHT_DIR="$PLAYWRIGHT_DIR" EXPECT_ACCOUNTS="${#accounts[@]}" \
  node "$root/console/browser-test/run.mjs" || fail "the browser test failed"

step "the dev server's proxy reaches the bot's console: login and session through it"
dev_port="$(dev_stack_free_port)"
(cd "$root/console" && CONSOLE_BOT_URL="$console" exec npx vite --port "$dev_port" --strictPort --host 127.0.0.1 > "$work/vite.log" 2>&1) &
vite_pid=$!
for _ in $(seq 1 100); do
  curl -s -o /dev/null "http://127.0.0.1:$dev_port/" && break
  kill -0 "$vite_pid" 2>/dev/null || fail "the dev server exited: $(tail -n 5 "$work/vite.log")"
  sleep 0.2
done
dev="http://127.0.0.1:$dev_port"
status="$(curl -s -o /dev/null -w '%{http_code}' -c "$work/dev-cookies" -H "Origin: $dev" -H 'content-type: application/json' \
  -d "{\"password\":\"$password\"}" "$dev/api/login")"
[[ "$status" == 204 ]] || fail "the login through the dev server answered $status"
status="$(curl -s -o /dev/null -w '%{http_code}' -b "$work/dev-cookies" "$dev/api/session")"
[[ "$status" == 204 ]] || fail "the session check through the dev server answered $status"
kill "$vite_pid" 2>/dev/null || true
wait "$vite_pid" 2>/dev/null || true

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

echo "PASS: the console app signed in to the bot's console server, matched each account to the API, stopped and started a strategy, flattened an account, and the bot stopped with exit 0"
