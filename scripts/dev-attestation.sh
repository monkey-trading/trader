#!/usr/bin/env bash
# Dev scenario for the production build and the node's attestation:
#   1. a production build refuses the dev stack's bot config at load, naming each value only a dev
#      build takes in turn: the node's mock attestation, then its pinned TLS key, then the hex key;
#   2. with the engine's verifier test fixtures (engine-verify's `test-fixtures`), a test-signed
#      SEV-SNP attestation verifies against the fixture release manifest and root, and the bot
#      trades against the signer it names, while a mock, a quote no release or trusted root admits,
#      and an attestation of another TLS key or signer stop it before it sends anything (the
#      `attestation` tests, against the local stand-in node).
#
# Needs what scripts/dev-stack.sh needs. TRADER_BIN runs a built production trader instead of
# building one; SCENARIO_ARTIFACTS keeps the node's and the bot's logs and configs.
set -Eeuo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
source "$root/scripts/dev-stack.sh"
read -r -a cargo_cmd <<< "${DEV_STACK_CARGO:-cargo}"
# Anvil's published account 1, which the dev scenarios trade.
key="59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"

trap dev_stack_stop EXIT
step() { echo "==> $*"; }
fail() {
  echo "FAIL: $*" >&2
  exit 1
}
trap 'fail "the command at line $LINENO failed"' ERR

step "build the production bot"
if [[ -n "${TRADER_BIN:-}" ]]; then
  trader="$TRADER_BIN"
else
  "${cargo_cmd[@]}" build -q --locked -p trader --features production --manifest-path "$root/Cargo.toml"
  trader="$(cargo metadata --format-version 1 --no-deps --manifest-path "$root/Cargo.toml" | jq -r .target_directory)/debug/trader"
fi

step "start the dev stack"
dev_stack_start
work="$dev_stack_work"

step "the production bot refuses the dev stack's config, one dev-only value at a time"
printf '%s\n' "$key" > "$work/account.hex"
chmod 600 "$work/account.hex"
# The dev stack's bot config, as a dev build takes it, with the `n`th edit: none; an SEV-SNP
# attestation in place of the mock (the release file is not read at load); that and no TLS pin.
dev_config() {
  local attestation='attestation = { type = "sev_snp", releases = ["release.toml"] }'
  {
    dev_stack_node_config
    cat <<CONFIG

[account]
key = { type = "hex_file", path = "account.hex" }

[strategy]
market = "BTC-USD"
bar = "1m"
fast = 1
slow = 2
qty = "0.5"
max_slippage_bps = 25
CONFIG
  } | case "$1" in
    0) cat ;;
    1) sed "s|^attestation = .*|$attestation|" ;;
    2) sed -e "s|^attestation = .*|$attestation|" -e '/^tls_key_sha256/d' ;;
  esac
}
# Each run's config drops the value the run before it was refused for.
refusals=(
  "node.attestation: a mock attestation proves nothing"
  "node.tls_key_sha256: a production build takes only a node whose certificate a public CA signs"
  "account.key: a plain hex key is accepted only in a dev build"
)
for n in "${!refusals[@]}"; do
  dev_config "$n" > "$work/bot-$n.toml"
  if "$trader" run --config "$work/bot-$n.toml" > "$work/bot-$n.log" 2>&1; then
    fail "the production bot took $work/bot-$n.toml"
  fi
  grep -qF "${refusals[$n]}" "$work/bot-$n.log" \
    || fail "the production bot did not refuse with '${refusals[$n]}': $(cat "$work/bot-$n.log")"
  echo "refused: ${refusals[$n]}"
done
dev_stack_stop

step "a test-signed attestation verifies, and the bot trades against its signer"
"${cargo_cmd[@]}" test -q --locked -p trader --manifest-path "$root/Cargo.toml" --test attestation

echo "PASS: the production bot refused the dev stack's config, and an attested signer is the one the bot trades against"
