#!/usr/bin/env bash
# Verifies: NFR-1 (size threshold), NFR-3 (startup threshold), NFR-7
# (the pipeline enforces both on every merge to main). Thresholds were fixed at the Phase 0 exit
# test (docs/phase0-report.md) and only move with a recorded measurement.
set -euo pipefail
cd "$(dirname "$0")/.."

binary=target/release/lca
size=$(stat -c %s "$binary" 2>/dev/null || stat -f %z "$binary")
max_size=$((25 * 1024 * 1024))
echo "binary size: $size bytes (limit $max_size)"
if [ "$size" -gt "$max_size" ]; then
  echo "NFR-1 exceeded: binary is larger than25 MB"
  exit 1
fi

# Cold start proxy: process spawn to first output of a trivial command.
# The interactive-prompt number (NFR-3,150 ms) is checked against this
# lower-bound measurement; the TUI adds its own cost on top and is
# profiled when the interface changes.
startup=$(python3 - <<'PY'
import subprocess, statistics, time
samples = []
for _ in range(30):
    start = time.perf_counter()
    subprocess.run(["target/release/lca", "--version"], check=True, capture_output=True)
    samples.append((time.perf_counter() - start) * 1000)
print(f"{statistics.median(samples):.1f}")
PY
)
max_startup_ms=150
echo "cold start (--version proxy): ${startup} ms (limit ${max_startup_ms} ms)"
if python3 -c "import sys; sys.exit(0 if float('$startup') <= $max_startup_ms else 1)"; then
  echo "gates passed"
else
  echo "NFR-3 proxy exceeded: startup slower than150 ms"
  exit 1
fi
