#!/usr/bin/env bash
# Verifies: NFR-1 (size threshold), NFR-3 (startup threshold), NFR-6
# (idle memory), NFR-7 (the pipeline enforces all on every merge to
# main), and NFR-31 (the cache-hit-ratio benchmark runs in this same
# gate too). Thresholds were fixed at the Phase 0 exit test
# (docs/phase0-report.md; the cache ratio's 0.90 at the Phase 3 exit)
# and only move with a recorded measurement.
#
# Two more used to be checked here, on the phase-0 spike host's pulley
# build: the interpreter-only build's size budget and its
# no-compiler-symbol count. That workspace was removed on 2026-10-01
# (commit 6a17652), so both requirements are named in
# scripts/deferred-requirements.txt instead of being checked against a
# build the tree no longer contains. Restore a build worth measuring
# and take them back out of that file.
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

# Verifies: NFR-31 - the canonical twenty-turn scripted conversation,
# run in this same gate so a cache regression fails the build the way a
# binary-size regression does (testing-plan section9).
cargo test --release -p lca-core --test loop \
  twenty_clean_turns_report_zero_cache_waste_and_hold_the_ratio

# Verifies: NFR-6 (idle memory with no extensions enabled). The
# interface idles in a pty until we sample its resident set.
python3 - <<'PY'
import os, re, signal, subprocess, time
# An IDLE session, not a flag: the interface sits at its prompt with no
# extensions enabled while we sample.
env = dict(os.environ, TERM="xterm")
proc = subprocess.Popen(
    ["script", "-q", "-c", "target/release/lca", "/dev/null"],
    stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=env,
)
peak = 0
try:
    for _ in range(30):
        time.sleep(0.1)
        children = subprocess.run(
            ["pgrep", "-P", str(proc.pid)], capture_output=True, text=True
        ).stdout.split()
        pids = [proc.pid] + [c.strip() for c in children if c.strip().isdigit()]
        rss = 0
        for pid in pids:
            try:
                with open(f"/proc/{pid}/status") as f:
                    for line in f:
                        if line.startswith("VmRSS:"):
                            rss += int(re.findall(r"\d+", line)[0])
            except FileNotFoundError:
                pass
        peak = max(peak, rss)
        if proc.poll() is not None:
            break
finally:
    if proc.poll() is None:
        proc.kill()
        proc.wait()
print(f"idle resident memory: {peak /1024:.1f} MB (limit80 MB)")
if peak == 0:
    raise SystemExit("NFR-6: could not sample the idle interface (did the TUI start?)")
if peak >80 *1024:
    raise SystemExit("NFR-6 exceeded: idle memory over80 MB")
PY
echo "gates passed"