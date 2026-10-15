#!/usr/bin/env bash
# ci-coverage.sh: every workspace member must be selected by at least one
# test-split group (the `packages:` lists in ci.yml), so a new crate
# cannot silently run nowhere on Linux/macOS. (Windows runs --workspace
# and would mask the hole: 25 packages once ran Windows-only.)
# Test-level timing exclusions are owned by the gates job's serial NFR
# run; hook-named units run in their home jobs via the scoped `-E`
# filter - see the matrix comment. Ignored (`#[ignore]`) witnesses are
# sanctioned red for future milestones (docs/pi-parity.md) and stay out.
set -euo pipefail
cd "$(dirname "$0")/.."

covered=$(grep -o "packages: '[^']*'" .github/workflows/ci.yml \
    | grep -o '\-p [A-Za-z0-9_-]*' | awk '{print $2}' | sort -u)
[ -n "$covered" ] || { echo "ci-coverage: no packages found in ci.yml"; exit 1; }

missing=$(python3 - "$covered" <<'PYEOF'
import json, subprocess, sys
covered = set(sys.argv[1].split())
meta = json.loads(subprocess.run(
    ["cargo", "metadata", "--no-deps", "--format-version", "1"],
    capture_output=True, text=True, check=True).stdout)
missing = sorted(p["name"] for p in meta["packages"] if p["name"] not in covered)
if missing:
    print("ci-coverage: these workspace members run in no test-split group:")
    for name in missing:
        print(f"  - {name}")
    sys.exit(1)
print(f"ci-coverage: all {len(meta['packages'])} members selected")
PYEOF
)
status=$?
echo "$missing"
exit $status
