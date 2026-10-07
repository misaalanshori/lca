#!/usr/bin/env bash
# Docs consistency (#106, #95): `docs/release-policy.md` carries the one
# canonical eleven-gate table; AGENTS.md and README.md point at it instead
# of copying it (four hand-maintained copies drifted once already, QA-005).
# The TOML resources and the manifest schema are the single source of
# truth for preset and capability facts; docs that restate them are
# checked below (QA-009).
#
# The checker is dumb on purpose: gate-name presence in the canonical
# table, pointer presence plus no gate-shaped copy in the other two,
# then explicit data-vs-docs counts and name lookups.
# Exit 0 = consistent, 1 = drift.
set -u
# A test seam and nothing else: the self-test points the net at a
# fixture tree, every other caller resolves the working copy.
cd "${DOCS_CONSISTENCY_ROOT:-$(dirname "$0")/..}" || exit 2

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

# Data-vs-docs drift net (#95): the TOML resources and the manifest
# schema are the single source of truth; docs that restate their facts
# are checked here, dumbly and explicitly. A restated fact edits both
# the data and this list together.
presets_toml="extensions/openai-compatible/resources/provider-presets.toml"
presets_doc="docs/providers/README.md"
preset_blocks=$(grep -c '^\[\[preset\]\]' "$presets_toml")
# The prose count ("19 presets ship today") and one table row per preset.
prose_count=$(grep -oE '^[0-9]+ presets ship today' "$presets_doc" | grep -oE '^[0-9]+')
table_rows=$(grep -cE '^\| `[a-z0-9-]+` \|' "$presets_doc")
if [ "$prose_count" != "$preset_blocks" ]; then
  echo "docs-consistency: $presets_doc claims $prose_count presets, data has $preset_blocks"
  fail=1
fi
if [ "$table_rows" != "$preset_blocks" ]; then
  echo "docs-consistency: $presets_doc tables $table_rows presets, data has $preset_blocks"
  fail=1
fi
# Every id the table names exists as an `id = "..."` in the data.
for id in $(grep -oE '^\| `[a-z0-9-]+`' "$presets_doc" | tr -d '| `' | sort -u); do
  if ! grep -qF "id = \"$id\"" "$presets_toml"; then
    echo "docs-consistency: preset id '$id' is tabled in docs but missing from $presets_toml"
    fail=1
  fi
done

# Every `[capabilities.X]` section docs/capabilities.md declares parses
# in the manifest schema's capability vocabulary.
caps_doc="docs/capabilities.md"
schema="schemas/extension-manifest.schema.json"
for cap in $(grep -oE '^\[capabilities\.[a-z-]+\]' "$caps_doc" | sed 's/^\[capabilities\.//; s/\]$//' | sort -u); do
  if ! grep -qF "\"$cap\":" "$schema"; then
    echo "docs-consistency: capability '$cap' is documented but missing from $schema"
    fail=1
  fi
done

# Provider decoupling (gh #157): the host crate embeds no provider
# literals. `api.openai.com` in `crates/lca-cli/src` is the QA-018
# smell verbatim - the manifest owns default hosts now. Test fixtures
# use fictional providers, so the whole crate tree must read clean.
if grep -rn 'api\.openai\.com' crates/lca-cli/ | grep -q .; then
  echo "docs-consistency: provider literal leaks into lca-cli:"
  grep -rn 'api\.openai\.com' crates/lca-cli/
  fail=1
fi

if [ "$fail" = 0 ]; then
  echo "docs-consistency: one gate list, two pointers, no copies"
  echo "docs-consistency: preset table and capability sections match their data"
fi
exit "$fail"
