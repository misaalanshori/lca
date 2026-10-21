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

# gh #427: the committed conformance fixture must still match the source.
#
# Compared by **interface**, not by bytes. Byte equality was the first attempt
# and it does not survive the environment: this tree builds byte-stable
# components locally (verified across cold clones, cross-commit warm caches and
# CARGO_INCREMENTAL=0), but a hosted runner produced a component of identical
# size, identical interface and different bytes - the profile is
# `lto = "fat"`, `codegen-units = 1`, so any toolchain/linker detail below that
# can move the bytes while the component means exactly the same thing. A gate
# that reddens on codegen noise is a gate people learn to ignore, which is
# worse than no gate.
#
# The failure this guard exists for (#427) is a *link* failure: a component
# built against an older surface, missing the exports a declared world needs.
# That is an interface difference - the stale fixture had 18 exports where the
# source had 30 - and an interface comparison catches it exactly.
#
# The hole this leaves is named rather than hidden: a guest change that alters
# behaviour without altering the interface passes here. It is still caught on
# every pipeline run by the WASM-mode conformance diff, which executes the
# component in both delivery modes and diffs the results, so behaviour cannot
# drift silently - it merely loses a byte-level alarm, and gains a gate that
# actually runs everywhere.
fixture=extensions/conformance/fixtures/tool-world.wasm
built="${CARGO_TARGET_DIR:-target}/$target/release/conformance.wasm"
if [ -f "$fixture" ]; then
    if [ ! -f "$built" ]; then
        echo "wasm-check: the build produced no $built; the package shape changed" >&2
        exit 1
    fi
    if ! command -v wasm-tools >/dev/null 2>&1; then
        echo "wasm-check: installing wasm-tools for the fixture interface check"
        cargo install --quiet --locked wasm-tools >/dev/null 2>&1 || true
    fi
    if command -v wasm-tools >/dev/null 2>&1; then
        committed=$(wasm-tools component wit "$fixture" 2>/dev/null || true)
        fresh=$(wasm-tools component wit "$built" 2>/dev/null || true)
        if [ -z "$committed" ] || [ -z "$fresh" ]; then
            echo "wasm-check: could not read a component interface from $fixture or $built" >&2
            exit 1
        fi
        if [ "$committed" != "$fresh" ]; then
            echo "wasm-check: $fixture no longer matches the conformance source (gh #427)" >&2
            echo "  the committed component exports a different interface than a fresh build:" >&2
            diff <(printf '%s\n' "$committed") <(printf '%s\n' "$fresh") | head -30 >&2
            echo "  reinstall it: cp $built $fixture" >&2
            exit 1
        fi
        echo "wasm-check: committed conformance fixture matches a fresh build"
    else
        echo "wasm-check: wasm-tools unavailable; the load guard in" >&2
        echo "  crates/lca-ext-host/tests/fixture_current.rs is the check that runs" >&2
    fi
fi
# A missing fixture is a different failure and is already owned: the load
# guard in crates/lca-ext-host/tests/fixture_current.rs asserts the
# directory still lists at least one component that links.
