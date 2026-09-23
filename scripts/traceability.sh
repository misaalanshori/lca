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
deferred_ids=$(mktemp)
trap 'rm -f "$requirements" "$markers" "$deferred_ids"' EXIT

# Deferred by the sanctioned cut (see the file: the web target, under
# the SRDD risk table's lever). Printed as deferred, never silently
# dropped - NFR-30's "zero untagged" holds over the requirements in
# force for this release.
grep -E '^(NFR|FR)-' scripts/deferred-requirements.txt > "$deferred_ids" || true

# Every requirement id the SRDD defines, minus the explicitly deferred
# ones (their ids are still gathered below so a stale-marker warning
# about them stays visible).
grep -oE '\b(NFR|FR)-([A-Z]+-)?[0-9]+\b' docs/lca-srdd.md | sort -u > "$requirements.all"
comm -23 "$requirements.all" "$deferred_ids" > "$requirements"
def_count=$(wc -l < "$deferred_ids")
if [ "$def_count" -gt 0 ]; then
  echo "deferred (see scripts/deferred-requirements.txt):"
  sed 's/^/  /' "$deferred_ids"
fi

# Every marker in the test tree (unit, integration, e2e, regressions).
grep -rhA8 -E 'Verifies:' crates extensions tests scripts .github 2>/dev/null \
  | grep -oE '\b(NFR|FR)-([A-Z]+-)?[0-9]+\b' | sort -u > "$markers"

untagged=$(comm -23 "$requirements" "$markers")
stale=$(comm -13 "$requirements" "$markers")

# Verifies: NFR-24 (a released defect's test lands in
# tests/regressions/<issue-id>-<short-slug>.rs, written before the fix
# and in the same change; no defect has reached a release yet, so the
# directory's presence is what this checks today).
if [ ! -d tests/regressions ]; then
  echo "missing tests/regressions/ (NFR-24's home for a released defect's test)"
  exit 1
fi

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
