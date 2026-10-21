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
# a deliberate, reviewed step) - but the conformance fixture is VERIFIED
# against the build below, because a committed component with no refresh
# step anywhere (not here, not the publish workflow's five other packages)
# silently went stale against ABI 0.6 once already (gh #427).
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
# gh #427: force the conformance artifact to be produced by *this* run.
#
# A cached target (CI restores one through rust-cache) can hold a component
# built from a different revision, and cargo will report the package fresh
# because the cached fingerprint matches that older source - so the byte
# comparison below reads bytes this run never produced and reports a correct
# fixture as stale. It did exactly that on CI twice.
#
# Touching the crate's sources is what makes cargo recompile it: it changes
# the mtime every unit input is checked against, which covers fingerprints,
# hard-linked deps copies and any other bookkeeping the cache restored. The
# source is left byte-identical (the mtime is metadata, not content), and the
# cost is one compile+link of this one crate rather than a rebuild of the graph.
conformance_artifact="${CARGO_TARGET_DIR:-target}/$target/release/conformance.wasm"
find extensions/conformance/src -name '*.rs' -exec touch {} +
touch extensions/conformance/Cargo.toml
# shellcheck disable=SC2086
cargo build --release --target "$target" --quiet $(for p in $packages; do echo "-p $p"; done)
echo "wasm-check: every extension component builds"

# gh #427: the committed conformance fixture must be the bytes this build
# just produced. The component above is already built, so this costs one
# byte comparison. CARGO_TARGET_DIR redirects the artifact, not this path
# (gh #260's lesson), so the lookup follows it.
fixture=extensions/conformance/fixtures/tool-world.wasm
if [ -f "$fixture" ]; then
    if [ ! -f "$conformance_artifact" ]; then
        echo "wasm-check: the build produced no $conformance_artifact; the package shape changed" >&2
        exit 1
    fi
    if ! cmp -s "$conformance_artifact" "$fixture"; then
        echo "wasm-check: $fixture is stale against the conformance source (gh #427)" >&2
        echo "  committed: $(sha256sum "$fixture" | cut -d' ' -f1) ($(wc -c <"$fixture") bytes)" >&2
        echo "  built:     $(sha256sum "$conformance_artifact" | cut -d' ' -f1) ($(wc -c <"$conformance_artifact") bytes)" >&2
        echo "  toolchain: $(rustc -V), target $target" >&2
        echo "  the committed component no longer matches a fresh build - reinstall it:" >&2
        echo "  cp $conformance_artifact $fixture" >&2
        exit 1
    fi
    echo "wasm-check: committed conformance fixture matches a fresh build"
fi
# A missing fixture is a different failure and is already owned: the load
# guard in crates/lca-ext-host/tests/fixture_current.rs asserts the
# directory still lists at least one component that links.
