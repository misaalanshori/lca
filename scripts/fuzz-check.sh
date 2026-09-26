#!/usr/bin/env bash
# The fuzz workspace's build gate.
#
# The fuzz crate is its own workspace, so `cargo check --workspace` from the
# root never touches it: a parser change can leave a fuzz target
# uncompilable, and the nightly `fuzz` workflow then dies at build *after*
# the other targets have burned their full 600s. This checks the fuzz
# workspace cheaply on every push so that class of break is caught in CI
# minutes instead of overnight.
#
# Verifies: docs/testing-plan.md section 13 (the fuzz schedule's targets must
# actually build), FR-DIST-3.
set -euo pipefail

cd "$(dirname "$0")/../fuzz"

echo "fuzz-check: building every fuzz target"
cargo check --bins --quiet
echo "fuzz-check: all fuzz targets build"
