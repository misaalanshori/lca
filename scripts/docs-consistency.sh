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

# The reverse direction (gh #307): every capability the SCHEMA declares
# must have its own `### <name>` section in docs/capabilities.md, so a
# capability added to the normative source is never shipped undocumented.
# `crates/lca-registry` has the matching guard on the consent side
# (`every_schema_capability_has_a_consent_line`), reading this same file,
# which is how the missing `tools` consent arm will be caught next time.
for cap in $(python3 - "$schema" <<'PYS'
import json, sys
props = json.load(open(sys.argv[1]))["properties"]["capabilities"]["properties"]
print("\n".join(sorted(props)))
PYS
); do
  if ! grep -qE "^### ${cap}$" "$caps_doc"; then
    echo "docs-consistency: capability '$cap' is in $schema but has no '### $cap' section in $caps_doc"
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

# Shared wire kits (gh #189): the SSE engines live in exactly one
# place each. A second `SseDecoder`, `ResponsesStream`,
# `AnthropicStream`, request-body builder, or `StreamFailure` outside
# `crates/lca-wire-*` is a forked engine, not a use of the kit.
# `crates/lca-mcp` is the second sanctioned home (gh #418): its SSE envelope
# is the MCP streamable-HTTP transport, which is a different wire protocol
# from the model-provider kits above, so the guard still means "no forked
# provider engine" rather than "one SSE parser in the tree".
leak=$(grep -rn --include='*.rs' -E 'struct SseDecoder|struct ResponsesStream|struct AnthropicStream|fn build_responses_body|fn build_messages_body|fn parse_sse|struct StreamFailure' crates/ extensions/ | grep -v -e '^crates/lca-wire-' -e '^crates/lca-mcp/' || true)
if [ -n "$leak" ]; then
  echo "docs-consistency: residual wire engine outside the kits:"
  echo "$leak"
  fail=1
fi

# Reserved-string vocabularies (gh #108, QA-022): explicit value lists
# on both sides; a rename edits the docs table, the code, and this
# list together. The Rust vocabulary test pins parse/serialize
# behavior; this pins the docs tables to the same values.
record_tags=(
  session-start user assistant tool-call tool-result permission
  extension-event compaction fork-point model-change thinking-level-change
  usage label session-info custom custom-message context-edit
  branch-point branch-summary session-end
)
for tag in "${record_tags[@]}"; do
  if ! grep -qF "\`$tag\`" docs/session-log-format.md; then
    echo "docs-consistency: record tag '$tag' is known to the reader but missing from docs/session-log-format.md"
    fail=1
  fi
  if ! grep -qF "\"$tag\"" crates/lca-session/src/store.rs; then
    echo "docs-consistency: record tag '$tag' is documented but missing from the reader's known types"
    fail=1
  fi
done
scopes=(workspace private home-config temp)
for scope in "${scopes[@]}"; do
  if ! grep -qF "\`$scope\`" docs/capabilities.md; then
    echo "docs-consistency: scope '$scope' parses but is missing from docs/capabilities.md"
    fail=1
  fi
  if ! grep -qF "\"$scope\"" crates/lca-permissions/src/scope.rs; then
    echo "docs-consistency: scope '$scope' is documented but missing from the scope vocabulary"
    fail=1
  fi
done
extras_keys=(image.vision image.resize prompt_cache thinking_levels)
for key in "${extras_keys[@]}"; do
  if ! grep -qF "\`$key\`" docs/providers/openai-compatible.md; then
    echo "docs-consistency: extras key '$key' is a constant but missing from docs/providers/openai-compatible.md"
    fail=1
  fi
done

# User-docs link rot (#84): every `docs/*.md` link the README names
# resolves to a file. Anchors are the author's business.
for link in $(grep -oE '\(docs/[a-z0-9/_.-]+\.md(#[-a-z0-9]+)?\)' README.md | tr -d '()' | cut -d'#' -f1 | sort -u); do
  if [ ! -f "$link" ]; then
    echo "docs-consistency: README links '$link', which does not exist"
    fail=1
  fi
done

if [ "$fail" = 0 ]; then
  echo "docs-consistency: one gate list, two pointers, no copies"
  echo "docs-consistency: preset table and capability sections match their data"
  echo "docs-consistency: vocabulary tables match their validators"
  echo "docs-consistency: README doc links resolve"
fi
exit "$fail"
