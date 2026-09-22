#!/bin/bash
# Re-run the cross-compilation matrix targets that failed on zstd-sys (the
# spike's wasmtime `cache` feature) after dropping that feature.
set -u
export PATH="$HOME/.cargo/bin:$HOME/tools/zig-x86_64-linux-0.16.0:$PATH"
cd "$(dirname "$0")" || exit 1
LOG=matrix-rerun.log
: > "$LOG"
run() {
  local name="$1"; shift
  local start=$(date +%s)
  echo "=== $name ===" >> "$LOG"
  if "$@" >> "$LOG" 2>&1; then
    echo "$name OK ($(( $(date +%s) - start ))s)" >> "$LOG"
  else
    echo "$name FAIL rc=$?" >> "$LOG"
  fi
}
ZIG="$HOME/tools/zig-x86_64-linux-0.16.0/zig"
run aarch64-linux-musl cargo zigbuild --release -p host --target aarch64-unknown-linux-musl
run x86_64-apple-darwin cargo zigbuild --release -p host --target x86_64-apple-darwin
run aarch64-apple-darwin cargo zigbuild --release -p host --target aarch64-apple-darwin
run x86_64-pc-windows-msvc cargo xwin build --release -p host --target x86_64-pc-windows-msvc
run aarch64-pc-windows-msvc cargo xwin build --release -p host --target aarch64-pc-windows-msvc
echo "rerun done" >> "$LOG"
