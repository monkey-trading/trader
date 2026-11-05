#!/usr/bin/env bash
# Checks the trader image <image>: its /trader is a static executable for <arch>, as `file` names
# the architecture (x86-64 or aarch64), and the image runs it alone, printing the crate's version.
# CI runs it on the images it builds.
#
# Usage: scripts/check-image-binary.sh <image> <arch>
set -euo pipefail

image="$1"
arch="$2"
version="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$(dirname "$0")/../Cargo.toml" | head -n 1)"
work="$(mktemp -d)"
container=""
cleanup() {
  [[ -z "$container" ]] || docker rm "$container" > /dev/null
  rm -rf "$work"
}
trap cleanup EXIT

container="$(docker create "$image")"
docker cp "$container:/trader" "$work/trader" > /dev/null
described="$(file "$work/trader")"
echo "$described"
[[ "$described" == *"$arch"* ]] || { echo "FAIL: /trader is not built for $arch" >&2; exit 1; }
[[ "$described" == *"statically linked"* || "$described" == *"static-pie linked"* ]] \
  || { echo "FAIL: /trader is not statically linked" >&2; exit 1; }
printed="$(docker run --rm "$image" --version)"
[[ "$printed" == "trader $version" ]] || { echo "FAIL: --version printed '$printed', not 'trader $version'" >&2; exit 1; }
echo "PASS: $image holds a static $arch trader $version that runs alone"
