#!/usr/bin/env bash
# The wasm components' build gate.
#
# The extension crates are ordinary workspace members, so `cargo check
# --workspace` builds them natively and says nothing about their wasm
# target. Their `wit_bindgen::generate!` blocks are target-shaped: when a
# world grows an import, every `with:` block has to map it, and the one
# that doesn't only fails at `--target wasm32-wasip2`. Nothing in CI built
# that target, so the break surfaced at release time - the publish job's
# "Build the first-party components" step, after the binaries had already
# been attached.
#
# This checks every extension crate builds for the component target on
# every push. It is a build only: no fixtures are refreshed here (that is
# a deliberate, reviewed step).
#
# Verifies: docs/testing-plan.md section 13 (an extension component must
# actually build for the target it ships on), FR-EXT-1.
set -euo pipefail

cd "$(dirname "$0")/.."

target=wasm32-wasip2
if ! rustup target list --installed | grep -qx "$target"; then
    echo "wasm-check: $target is not installed; adding it"
    rustup target add "$target" >/dev/null
fi

# Every extension crate that produces a component. Adding a new one here
# is the price of it being built at all.
packages=$(cargo metadata --no-deps --format-version 1 \
    | python3 -c '
import json, sys
meta = json.load(sys.stdin)
names = [
    p["name"] for p in meta["packages"]
    if any(manifest := [t for t in p["targets"] if "cdylib" in t["crate_types"]])
]
print(" ".join(sorted(names)))
')

if [ -z "$packages" ]; then
    echo "wasm-check: no cdylib packages found; the workspace shape changed" >&2
    exit 1
fi

echo "wasm-check: building $packages for $target"
# shellcheck disable=SC2086
cargo build --release --target "$target" --quiet $(for p in $packages; do echo "-p $p"; done)
echo "wasm-check: every extension component builds"
