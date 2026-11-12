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

# Shipped-manifest conformance (gh #340). The manifest schema is the
# normative document an author validates against, so a first-party manifest
# that fails it is the schema contradicting the product -- and that is exactly
# what shipped: `extensions/openai-compatible` declares a top-level
# `resources` key, and `extensions/webfetch` ships `hosts = []` as a
# declaration (gh #302), both of which the old schema refused.
#
# This is the COMPLETE check (real JSON-Schema validation through
# `jsonschema`), because it is cheap here and the script already runs
# python3 for the capability list below. The subset of the same rules in
# Rust (`tests/regressions/340-shipped-manifests-validate.rs`) exists so the
# rules are also pinned on a machine without that module.
if python3 -c 'import jsonschema' >/dev/null 2>&1; then
  schema_errors=$(python3 - "$schema" <<'PYS'
import glob, json, sys
import tomllib
import jsonschema
schema = json.load(open(sys.argv[1]))
validator = jsonschema.Draft202012Validator(schema)
bad = 0
for path in sorted(glob.glob("extensions/*/extension.toml")):
    manifest = tomllib.load(open(path, "rb"))
    for error in validator.iter_errors(manifest):
        where = "/".join(str(part) for part in error.absolute_path) or "(root)"
        print(f"{path}: {where}: {error.message}")
        bad = 1
sys.exit(bad)
PYS
) || fail=1
  if [ -n "${schema_errors:-}" ]; then
    echo "$schema_errors"
  fi
else
  # The house pattern for a missing harness: skip with a named reason, never
  # pass silently. CI installs the module (`.github/workflows/ci.yml`), so an
  # absent module is a local-machine condition rather than a gate.
  echo "docs-consistency: skipping the shipped-manifest check (python3 jsonschema is not installed)"
fi

# The schema's own headline facts, so a future edit cannot quietly revert them:
# `resources` is a declared top-level key (ADR-0030's data-only shape), and
# `limits.fuel_per_call`'s documented default is the host's real one.
resources_kind=$(python3 - "$schema" <<'PYS'
import json, sys
prop = json.load(open(sys.argv[1]))["properties"].get("resources")
print("missing" if prop is None else prop.get("type", "untyped"))
PYS
)
if [ "$resources_kind" != "array" ]; then
  echo "docs-consistency: $schema's top-level \"resources\" property is $resources_kind, want an array (the ADR-0030 declared-kinds list)"
  fail=1
fi
fuel_default=$(python3 - "$schema" <<'PYS'
import json, sys
print(json.load(open(sys.argv[1]))["properties"]["limits"]["properties"]["fuel_per_call"]["default"])
PYS
)
if [ "$fuel_default" != "25000000" ]; then
  echo "docs-consistency: $schema documents fuel_per_call default $fuel_default, host default is 25000000"
  fail=1
fi

# Crate count and coverage (gh #340): the README's sentence and its table are
# the only place the crate inventory is restated, and both drifted silently
# (sixteen vs twenty-one, five crates absent from the table).
crate_count=$(ls -d crates/*/ | wc -l | tr -d ' ')
readme_crates=$(grep -oE '^One process, [a-z-]+ crates' README.md | sed 's/^One process, //; s/ crates$//')
if [ -z "$readme_crates" ]; then
  echo "docs-consistency: README.md no longer opens its architecture section with the crate count"
  fail=1
else
  # The sentence is spelled out in English; the count is its own answer.
  case "$crate_count" in
    21) want="twenty-one" ;;
    22) want="twenty-two" ;;
    *)  want="(update the spelling)" ;;
  esac
  if [ "$readme_crates" != "$want" ]; then
    echo "docs-consistency: README says '$readme_crates crates', the tree has $crate_count"
    fail=1
  fi
fi
for dir in crates/*/; do
  name=$(basename "$dir")
  if ! grep -qE "^\| \`$name\` \|" README.md; then
    echo "docs-consistency: crate '$name' has no row in README.md's architecture table"
    fail=1
  fi
done

# ADR count and index coverage (gh #340): the SRDD's prose named a stale range,
# and two records had no index row. Both are mechanical.
adr_hi=$(ls docs/adr | grep -oE '^[0-9]{4}' | sort -n | tail -1)
if ! grep -qF "0001 through $adr_hi" docs/lca-srdd.md; then
  echo "docs-consistency: docs/lca-srdd.md does not name the ADR range as 0001 through $adr_hi"
  fail=1
fi
# The index keys on the record's NUMBER ("| 0049 |"), not its whole filename,
# so compare the number part only.
for f in docs/adr/*.md; do
  number=$(basename "$f" .md | cut -c1-4)
  [ "$(basename "$f" .md)" = "README" ] && continue
  if ! grep -qE "^\| $number \|" docs/adr/README.md; then
    echo "docs-consistency: ADR '$number' ($f) has no row in docs/adr/README.md's index"
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
# Gh #576: `StreamFailure` is deliberately excluded now - it is ONE shared type in
# `lca-protocol`, not a forked wire engine, and excluding it is what lets the kits
# re-export it instead of each defining one. The forked-engine needles below are the
# ones that mean "a second SSE engine exists".
leak=$(grep -rn --include='*.rs' -E 'struct SseDecoder|struct ResponsesStream|struct AnthropicStream|fn build_responses_body|fn build_messages_body|fn parse_sse' crates/ extensions/ | grep -v -e '^crates/lca-wire-' -e '^crates/lca-mcp/' || true)
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
  # Gh #564 split `store.rs` into `store/`; the reader's known types live in one
  # of its files. The *glob* rather than `grep -r` on purpose: this script is
  # also run against a fixture tree of symlinks (tests/regressions/gh95-*), and
  # `grep -r` silently skips a symlinked file (only `-R` follows one), which
  # made the clean fixture look drifted. A shell glob hands grep the paths as
  # arguments, so a symlink reads exactly like the file it points at.
  if ! grep -qF "\"$tag\"" crates/lca-session/src/store/*.rs; then
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

# Structural drift in the user docs (2026-10-24). Two real defects shipped
# because a docs edit landed in the wrong place and nothing looked:
# `/permissions` (#261) inserted its section in the MIDDLE of usage.md's command
# table, so the remaining rows lost their header and columns and every row after
# it became a stray line of pipes; #412 left a lead-in sentence duplicated with a
# blank line between the copies. Both are mechanical to detect.
#
# (a) An orphaned table row: a `|` line inside a block that never carried a
# header separator (`| --- |`). Markdown only honours a table whose block opens
# with that separator, so a row without one renders as literal pipes - which is
# exactly what a reader saw. This catches the split wherever the interrupting
# heading now sits, not just the blank-then-row case.
for f in docs/*.md; do
  # A markdown table is valid only when its block's SECOND line is the header
  # separator; any other shape renders as literal pipes. Buffer each contiguous
  # `|` block and judge it on its first two lines, so a table whose header was
  # cut away by an inserted section is named at its first surviving row.
  orphans=$(awk '
    function flush() {
      if (n >= 2 && !sep) { printf "%d ", start; bad=1 }
      n=0; sep=0; start=0
    }
    BEGIN { n=0; sep=0; start=0 }
    /^\|/ {
      if (n == 0) start=NR
      n++
      if (n == 2) { if ($0 ~ /^\|[ :|-]+\|?[ :|-]*$/) sep=1 }
      next
    }
    { flush() }
    END { flush(); exit bad ? 0 : 1 }
  ' "$f" || true)
  if [ -n "$orphans" ]; then
    echo "docs-consistency: $f has a table with no header separator starting at line: $orphans"
    fail=1
  fi
  # (b) A line repeated verbatim across blank lines is a paste artefact: it
  # reads as a stutter and hides which copy is the real one.
  # Structural lines are excluded: two adjacent code fences are two blocks, two
  # adjacent `---` are a rule and a deeper rule, and a pipe row legitimately
  # repeats in a table of similar shapes. The artefact this catches is prose.
  dup=$(awk '$0!="" && $0!~/^```/ && $0!~/^---$/ && $0!~/^\|/ { if (prev==$0) print NR": "$0; prev=$0 }' "$f")
  if [ -n "$dup" ]; then
    echo "docs-consistency: $f repeats a line verbatim: $dup"
    fail=1
  fi
done

if [ "$fail" = 0 ]; then
  echo "docs-consistency: one gate list, two pointers, no copies"
  echo "docs-consistency: preset table and capability sections match their data"
  echo "docs-consistency: vocabulary tables match their validators"
  echo "docs-consistency: README doc links resolve"
  echo "docs-consistency: no doc table is split, no line repeats as a paste artefact"
  echo "docs-consistency: every shipped manifest validates, and the crate/ADR counts match the tree"
fi
exit "$fail"
