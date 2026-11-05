#!/usr/bin/env bash
# Runs the trader image as though it were the binary, for a dev scenario's TRADER_BIN: on the
# host's network, as the calling user, with the directory of the config file `--config` names, an
# absolute path, mounted at the same path, so the config's relative paths resolve as they do on the
# host and the bot can write the state it keeps beside them.
# The container's exit status is this script's, and SIGTERM and SIGINT reach the bot.
#
# Usage: TRADER_IMAGE=<image> scripts/docker-trader.sh run --config </absolute/path/to/file>
set -euo pipefail

: "${TRADER_IMAGE:?the image to run}"
config=""
args=("$@")
for i in "${!args[@]}"; do
  case "${args[$i]}" in
    -c | --config) config="${args[$((i + 1))]:-}" ;;
    --config=*) config="${args[$i]#--config=}" ;;
  esac
done
[[ "$config" == /* && -f "$config" ]] || { echo "docker-trader.sh: no absolute config path in: $*" >&2; exit 2; }
dir="$(cd "$(dirname "$config")" && pwd)"
# docker run passes the signals it receives to the container's process, the bot.
exec docker run --rm --network host --user "$(id -u):$(id -g)" --volume "$dir:$dir" "$TRADER_IMAGE" "$@"
