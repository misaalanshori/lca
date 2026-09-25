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

# Every marker in the test tree. A Rust marker counts only when it sits in
# the contiguous comment block that names the requirement, so a sentence in a
# neighbouring test cannot claim one; a shell/workflow marker counts when it
# sits within the 8 lines after a `Verifies` mention, where it names a named
# pipeline check rather than a test function.
python3 - "$markers" <<'PY'
import pathlib, re, sys

ids = set()
pattern = re.compile(r'\b(?:FR|NFR)-[A-Z]*-?\d+\b')

for root in ("crates", "extensions", "tests"):
    base = pathlib.Path(root)
    if not base.exists():
        continue
    for path in base.rglob("*.rs"):
        lines = path.read_text(errors="ignore").splitlines()
        i = 0
        while i < len(lines):
            if "Verifies:" in lines[i]:
                start = i
                while start > 0 and lines[start - 1].lstrip().startswith("//"):
                    start -= 1
                end = i
                while end + 1 < len(lines) and lines[end + 1].lstrip().startswith("//"):
                    end += 1
                ids.update(pattern.findall("\n".join(lines[start:end + 1])))
                i = end
            i += 1

pipeline = list(pathlib.Path("scripts").glob("*.sh"))
pipeline += list(pathlib.Path(".github").rglob("*.yml"))
for path in pipeline:
    lines = path.read_text(errors="ignore").splitlines()
    for i, line in enumerate(lines):
        if "Verifies" in line:
            ids.update(pattern.findall("\n".join(lines[i:i + 9])))

with open(sys.argv[1], "w") as out:
    for value in sorted(ids):
        out.write(value + "\n")
PY

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
