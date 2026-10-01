#!/usr/bin/env bash
# Gate 11: every tracked Rust source stays under the workspace's
# 1,200-line file ceiling (docs/testing-plan.md's file-size rule; the
# ceiling that twice needed a manual pass - this check is the root-cause
# fix). Prints the offenders and fails.
set -euo pipefail
cd "$(dirname "$0")/.."

limit=1200

offenders=$(git ls-files '*.rs' | xargs wc -l | awk -v l="$limit" \
  '$2 != "total" && $1 > l { printf "  %s (%s lines)\n", $2, $1 }')

if [ -n "$offenders" ]; then
  echo "ceiling-check: over ${limit} lines:"
  echo "$offenders"
  exit 1
fi

echo "ceiling-check: every tracked .rs file <= ${limit} lines"
