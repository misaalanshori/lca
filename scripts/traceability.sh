#!/usr/bin/env bash
# NFR-30 requirements traceability: every FR/NFR id in docs/lca-srdd.md
# must be referenced by at least one `// Verifies: FR-X-N` marker in the
# test tree. Advisory before Phase 8, required from Phase 8 (SRDD NFR-30).
#
# Verifies: NFR-30 (this script is the automated check NFR-30 names).
# Exit codes:0 = full coverage,1 = untagged requirements remain (report on
# stdout),2 = unknown ids referenced by tests (stale markers, warning only
# per docs/testing-plan.md section11: printed but not fatal).
set -u
cd "$(dirname "$0")/.."

requirements=$(mktemp)
markers=$(mktemp)
trap 'rm -f "$requirements" "$markers"' EXIT

# Every requirement id the SRDD defines.
grep -oE '\b(NFR|FR)-([A-Z]+-)?[0-9]+\b' docs/lca-srdd.md | sort -u > "$requirements"

# Every marker in the test tree (unit, integration, e2e, regressions).
grep -rhA3 -E 'Verifies:' crates extensions tests scripts .github 2>/dev/null \
  | grep -oE '\b(NFR|FR)-([A-Z]+-)?[0-9]+\b' | sort -u > "$markers"

untagged=$(comm -23 "$requirements" "$markers")
stale=$(comm -13 "$requirements" "$markers")

if [ -n "$stale" ]; then
  echo "warning: markers reference ids the SRDD does not define:"
  echo "$stale" | sed 's/^/  /'
fi

count_total=$(wc -l < "$requirements")
count_missing=$(printf '%s' "$untagged" | grep -c . || true)

if [ -n "$untagged" ]; then
  echo "UNTAGGED ($count_missing of $count_total requirements):"
  echo "$untagged" | sed 's/^/  /'
  exit 1
fi

echo "traceability: all $count_total requirements have at least one verifying test"
