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
# a deliberate, reviewed step) - but **every committed component** is
# VERIFIED against the build below, by interface, because a committed binary
# with no refresh step anywhere (not here, not the publish workflow's five
# other packages) silently went stale against ABI 0.6 once already (gh #427),
# and two more had by 2026-11-10 while this check named only one of them
# (gh #591).
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

# gh #427 / gh #591: EVERY committed component must still match its source.
#
# This used to compare one fixture - `extensions/conformance`'s - and nothing
# else. Every other committed component was covered by nothing, and by 2026-11-10
# two of them (`codex`, `grok`) had drifted: a fresh build of their own crate
# imports `lca:host/resources@0.6.0` and the `net` interface's
# `response-headers`, while the committed file imports neither. A 0.6 host
# *loads* a guest that imports a subset, so the CLI's own e2e rows kept passing
# against the stale binary - the silent-drift shape gh #591 is about.
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
#
# The fixture list is DERIVED, never hard-coded (gh #591): every
# `extensions/*/fixtures/*.wasm` whose own crate is built above is compared, and
# each one is printed as it is checked so a failure names the artifact. A
# directory with no fresh build is skipped *with its reason* - a silent pass is
# the one outcome this must never produce.
# `cargo install` puts the binary in $CARGO_HOME/bin (usually ~/.cargo/bin),
# which is NOT always on PATH: this script also runs from `bash --norc`
# shells and from CI steps that never sourced a profile. Looking there too
# is what keeps the interface check from silently degrading to the
# wasm-tools-unavailable branch on a machine that has it installed.
wasm_tools=$(command -v wasm-tools 2>/dev/null || true)
if [ -z "$wasm_tools" ]; then
    for candidate in "${CARGO_HOME:-$HOME/.cargo}/bin/wasm-tools" "$HOME/.cargo/bin/wasm-tools"; do
        if [ -x "$candidate" ]; then wasm_tools=$candidate; break; fi
    done
fi
if [ -z "$wasm_tools" ]; then
    echo "wasm-check: installing wasm-tools for the fixture interface check"
    cargo install --quiet --locked wasm-tools >/dev/null 2>&1 || true
    if [ -x "${CARGO_HOME:-$HOME/.cargo}/bin/wasm-tools" ]; then
        wasm_tools="${CARGO_HOME:-$HOME/.cargo}/bin/wasm-tools"
    fi
fi
if [ -z "$wasm_tools" ]; then
    # Skip, never pass silently: say which fixtures went unchecked and what
    # would have checked them. The load guard is stronger where it applies, but
    # it covers one directory (crates/lca-ext-host/tests/fixture_current.rs).
    echoed=$(for f in extensions/*/fixtures/*.wasm; do [ -e "$f" ] && printf '%s ' "$f"; done)
    echo "wasm-check: wasm-tools unavailable; these external fixtures went UNCHECKED:" >&2
    echo "  $echoed" >&2
    echo "  the load guard in crates/lca-ext-host/tests/fixture_current.rs still runs" >&2
    exit 0
fi

checked=0
unbuilt=0
for fixture in extensions/*/fixtures/*.wasm; do
    [ -e "$fixture" ] || continue
    extension=$(basename "$(dirname "$(dirname "$fixture")")")
    # The release artifact's name: cargo writes `my_ext.wasm` for package
    # `my-ext`, so hyphens become underscores. A directory whose crate is not
    # in this workspace (or not a cdylib) simply has no artifact to compare.
    artifact="${CARGO_TARGET_DIR:-target}/$target/release/${extension//-/_}.wasm"
    if [ ! -f "$artifact" ]; then
        echo "wasm-check: skipping $extension/$(basename "$fixture") - no build at $artifact (not a workspace cdylib)"
        unbuilt=$((unbuilt + 1))
        continue
    fi
    committed=$("$wasm_tools" component wit "$fixture" 2>/dev/null || true)
    fresh=$("$wasm_tools" component wit "$artifact" 2>/dev/null || true)
    if [ -z "$committed" ] || [ -z "$fresh" ]; then
        echo "wasm-check: could not read a component interface from $fixture or $artifact" >&2
        exit 1
    fi
    if [ "$committed" != "$fresh" ]; then
        echo "wasm-check: $fixture no longer matches its source (gh #427/#591)" >&2
        echo "  the committed component's interface is not a fresh build's of $extension:" >&2
        diff <(printf '%s\n' "$committed") <(printf '%s\n' "$fresh") | head -30 >&2
        echo "  reinstall it: cp $artifact $fixture" >&2
        exit 1
    fi
    echo "wasm-check: $extension fixture matches a fresh build"
    checked=$((checked + 1))
done

if [ "$checked" -eq 0 ]; then
    echo "wasm-check: no committed component could be compared; the fixture layout changed" >&2
    exit 1
fi
echo "wasm-check: all $checked committed components match their sources ($unbuilt skipped)"
# A *missing* fixture is a different failure and is already owned: the load
# guard in crates/lca-ext-host/tests/fixture_current.rs asserts the conformance
# directory still lists at least one component that links.
