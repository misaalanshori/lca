#!/usr/bin/env bash
# Gate-list consistency (#106): `docs/release-policy.md` carries the one
# canonical eleven-gate table; AGENTS.md and README.md point at it instead
# of copying it (four hand-maintained copies drifted once already, QA-005).
#
# The checker is dumb on purpose: gate-name presence in the canonical
# table, pointer presence plus no gate-shaped copy in the other two.
# Exit 0 = consistent, 1 = drift.
set -u
cd "$(dirname "$0")/.." || exit 2

fail=0
canonical="docs/release-policy.md"

# The eleven canonical gate names, exactly as the table's Gate column spells
# them. A rename edits both this list and the table together.
gates=(
  "Format and lint"
  "Test suite"
  "NFR timing"
  "Dependency audit and license check"
  "Requirements traceability"
  "Fuzz targets build"
  "Extension components build"
  "Release targets build"
  "Size and startup"
  "Installers"
  "File-size ceiling"
)

for name in "${gates[@]}"; do
  if ! grep -qF "$name" "$canonical"; then
    echo "docs-consistency: canonical table is missing gate: $name"
    fail=1
  fi
done

rows=$(grep -cE '^\|[[:space:]]*[0-9]+[[:space:]]*\|' "$canonical")
if [ "$rows" != "11" ]; then
  echo "docs-consistency: canonical table has $rows numbered rows, want 11"
  fail=1
fi

for file in AGENTS.md README.md; do
  if ! grep -qF "docs/release-policy.md#gate-list" "$file"; then
    echo "docs-consistency: $file has no pointer to the canonical gate list"
    fail=1
  fi
  # A numbered pipe-table row is gate-table-shaped whatever it names; no
  # other table in these two files uses that shape, so any occurrence is a
  # copy until a human teaches this script otherwise.
  if grep -nE "^\|[[:space:]]*[0-9]+[[:space:]]*\|" "$file"; then
    echo "docs-consistency: $file carries a hand-copied gate table"
    fail=1
  fi
  for name in "${gates[@]}"; do
    # A numbered-list item (`3. NFR timing (...)`) naming a gate is a hand
    # copy. (Pipe-table rows are caught by the shape check above
    # regardless of what they name.)
    if grep -nE "^[0-9]+\.[[:space:]]" "$file" | grep -qF "$name"; then
      echo "docs-consistency: $file carries a hand-copied gate list item for: $name"
      fail=1
    fi
  done
done

if [ "$fail" = 0 ]; then
  echo "docs-consistency: one gate list, two pointers, no copies"
fi
exit "$fail"
