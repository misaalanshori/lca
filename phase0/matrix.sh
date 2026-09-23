#!/bin/bash
# Verifies: NFR-8 (linux x86_64 + aarch64 musl), NFR-9 (macOS
# x86_64 + aarch64), and NFR-10 (Windows x86_64 + aarch64 MSVC): one
# matrix, six targets, cross-compiled from Linux (docs/phase0-report.md
# carries the receipt; re-run by hand, tagged here because the script is
# the executable form of the check).
# Phase 0 cross-compilation matrix: build the spike host for all six native
# targets and record what each one needs. Results land in phase0/matrix.log.
set -u
export PATH="$HOME/.cargo/bin:$HOME/tools/zig-x86_64-linux-0.16.0:$PATH"
cd "$(dirname "$0")" || exit 1

LOG=matrix.log
: > "$LOG"

run() {
  local name="$1"; shift
  local start=$(date +%s)
  echo "=== $name ===" >> "$LOG"
  if "$@" >> "$LOG" 2>&1; then
    local dur=$(( $(date +%s) - start ))
    echo "$name OK (${dur}s)" >> "$LOG"
  else
    local rc=$?
    echo "$name FAIL rc=$rc" >> "$LOG"
  fi
}

ZIG="$HOME/tools/zig-x86_64-linux-0.16.0/zig"

# 1. x86_64 Linux musl, static, straight rustc with musl-gcc.
run x86_64-linux-musl cargo build --release -p host --target x86_64-unknown-linux-musl

# 2. aarch64 Linux musl via cargo-zigbuild (zig provides the cross linker/cc).
run aarch64-linux-musl env \
  CC_aarch64_unknown_linux_musl="$ZIG cc -target aarch64-linux-musl" \
  CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER="$ZIG" \
  cargo zigbuild --release -p host --target aarch64-unknown-linux-musl

# 3/4. macOS via cargo-zigbuild.
run x86_64-apple-darwin env \
  CC_x86_64_apple_darwin="$ZIG cc -target x86_64-macos" \
  CARGO_TARGET_X86_64_APPLE_DARWIN_LINKER="$ZIG" \
  cargo zigbuild --release -p host --target x86_64-apple-darwin

run aarch64-apple-darwin env \
  CC_aarch64_apple_darwin="$ZIG cc -target aarch64-macos" \
  CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER="$ZIG" \
  cargo zigbuild --release -p host --target aarch64-apple-darwin

# 5/6. Windows MSVC via cargo-xwin (downloads the MSVC CRT and Windows SDK).
run x86_64-pc-windows-msvc cargo xwin build --release -p host --target x86_64-pc-windows-msvc

run aarch64-pc-windows-msvc cargo xwin build --release -p host --target aarch64-pc-windows-msvc

echo "matrix done" >> "$LOG"
