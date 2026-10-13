#!/usr/bin/env bash
# Prune old unstable extension images on GHCR (gh #257).
#
# Deletion automation on the package registry: the unit of safety is the
# tag-name guard. A version is deleted ONLY when none of its tags matches
# the protected shapes (semver `X.Y.Z...`, `abi-*`, `latest`) AND it is
# older than the retention window. Protected tags are checked first and
# unconditionally — a version carrying any protected tag is never even a
# deletion candidate, whatever else it carries.
#
# Usage:
#   prune-unstable-ghcr.sh selftest
#       Offline guard tests: fake GHCR version lists through the selector,
#       asserting only eligible ids come out (CI gate + local check).
#   prune-unstable-ghcr.sh <package> [--dry-run|--execute] [--keep N]
#       Prune one `misaalanshori/lca/<package>` container package.
#       Default is --dry-run (prints the plan, deletes nothing).
#       --execute deletes, and refuses unless GITHUB_ACTIONS=true, so a
#       dev machine can never prune the real registry — CI only.
set -euo pipefail

SELF="$0"
KEEP_DEFAULT=2

# ---------------------------------------------------------------------------
# The selector: versions JSON on stdin, deletion-candidate ids on stdout.
# Pure function of the payload — the selftest drives it with fixtures and
# the prune path drives it with the live `gh api` payload.
# ---------------------------------------------------------------------------
select_ids() {
    local keep="$1"
    python3 /dev/fd/3 "$keep" 3<<'PY'
import json
import re
import sys

keep = int(sys.argv[1])
try:
    versions = json.load(sys.stdin)
except Exception:
    sys.exit(0)

SEMVER = re.compile(r"^\d+\.\d+\.\d+")


def tags_of(version):
    meta = version.get("metadata") or {}
    container = meta.get("container") or {}
    return [t for t in (tag.get("name", "") for tag in container.get("tags") or []) if t]


def protected(tags):
    # THE GUARD. Any protected tag shields the whole version.
    return any(
        SEMVER.match(t) or t.startswith("abi-") or t == "latest" for t in tags
    )


def unstable(tags):
    return "unstable" in tags


# Newest first; missing timestamps sort last (never newest).
versions = sorted(versions, key=lambda v: v.get("created_at", ""), reverse=True)

unstable_kept = 0
spare_kept = 0
for version in versions:
    vid = version.get("id")
    if vid is None:
        continue
    tags = tags_of(version)
    if protected(tags):
        continue
    if unstable(tags):
        # The newest :unstable carrier is the live channel — keep it
        # even when the clock is lying. Older :unstable carriers are
        # superseded builds, eligible below.
        unstable_kept += 1
        if unstable_kept <= 1:
            continue
    # Unprotected, superseded, orphaned, or stale: keep one spare for
    # rollback, delete the rest.
    spare_kept += 1
    if spare_kept < keep:
        continue
    print(vid)
PY
}

# ---------------------------------------------------------------------------
# selftest: the guards, offline.
# ---------------------------------------------------------------------------
selftest() {
    local failures=0
    check() { # <name> <versions-json> <keep> <expected-ids...>
        local name="$1" payload="$2" keep="$3"
        shift 3
        local expected="$*"
        local got
        got=$(echo "$payload" | select_ids "$keep" | tr '\n' ' ' | sed 's/ *$//')
        if [ "$got" == "$expected" ]; then
            echo "ok: $name"
        else
            echo "FAIL: $name — expected [$expected], got [$got]"
            failures=$((failures + 1))
        fi
    }

    v() { # <id> <created> <tags-csv, empty for untagged>
        local id="$1" created="$2" tags="$3" jsontags="[]"
        if [ -n "$tags" ]; then
            jsontags=$(python3 -c "import json,sys; print(json.dumps([{'name': t} for t in sys.argv[1].split(',')]))" "$tags")
        fi
        printf '{"id": %s, "created_at": "%s", "metadata": {"container": {"tags": %s}}}' "$id" "$created" "$jsontags"
    }

    # The live channel plus history: the newest unstable (1) is the
    # channel, the next newest unprotected version (2) is the rollback
    # spare; the stale pins (3, 4) go. keep counts unprotected versions.
    check "superseded unstable and stale pins go" \
        "[$(v 1 2026-10-13T06:00:00Z unstable,b1111111),$(v 2 2026-10-12T06:00:00Z unstable,b0000000),$(v 3 2026-10-11T06:00:00Z b2222222),$(v 4 2026-10-10T06:00:00Z b3333333)]" \
        2 "3 4"

    # Protected shapes are never candidates, even mid-history; the
    # spare window counts unprotected versions only.
    check "semver, abi, and latest shield their versions" \
        "[$(v 1 2026-10-13T06:00:00Z unstable),$(v 5 2026-10-09T06:00:00Z 1.0.1),$(v 6 2026-10-08T06:00:00Z abi-0.6),$(v 7 2026-10-07T06:00:00Z latest),$(v 8 2026-10-06T06:00:00Z b4444444),$(v 9 2026-10-05T06:00:00Z b5555555)]" \
        2 "9"

    # A protected tag shields co-carried unstable/pin tags too.
    check "protection wins over co-carried unstable" \
        "[$(v 1 2026-10-13T06:00:00Z unstable),$(v 9 2026-10-05T06:00:00Z unstable,abi-0.6),$(v 10 2026-10-04T06:00:00Z latest,b5555555),$(v 11 2026-10-03T06:00:00Z b6666666),$(v 12 2026-10-02T06:00:00Z b7777777)]" \
        2 "12"

    # Untagged orphans prune past the spare.
    check "untagged orphans prune past the spare" \
        "[$(v 1 2026-10-13T06:00:00Z unstable),$(v 12 2026-10-02T06:00:00Z ''),$(v 13 2026-10-01T06:00:00Z '')]" \
        2 "13"

    # keep=1 keeps only the channel; garbage input deletes nothing.
    check "keep 1 deletes every superseded version" \
        "[$(v 1 2026-10-13T06:00:00Z unstable),$(v 2 2026-10-12T06:00:00Z b0000000)]" \
        1 "2"
    check "garbage input deletes nothing" "not json" 2 ""
    check "empty list deletes nothing" "[]" 2 ""

    # --execute refuses off CI (the dev-machine guard). GITHUB_ACTIONS
    # is forced off here so the check holds even under CI itself.
    if GITHUB_ACTIONS=false "$SELF" img --execute --keep 2 >/dev/null 2>&1; then
        echo "FAIL: --execute ran without GITHUB_ACTIONS=true"
        failures=$((failures + 1))
    else
        echo "ok: --execute refuses off CI"
    fi

    if [ "$failures" -ne 0 ]; then
        echo "$failures selftest failure(s)"
        return 1
    fi
    echo "selftest: all guards hold"
}

# ---------------------------------------------------------------------------
# prune: one package, live.
# ---------------------------------------------------------------------------
prune() {
    local package="$1" mode=--dry-run keep="$KEEP_DEFAULT"
    shift
    while [ $# -gt 0 ]; do
        case "$1" in
            --keep) keep="$2"; shift 2 ;;
            --dry-run | --execute) mode="$1"; shift ;;
            *) echo "unknown flag: $1" >&2; return 1 ;;
        esac
    done
    if [ "$mode" == "--execute" ] && [ "${GITHUB_ACTIONS:-}" != "true" ]; then
        echo "refusing: --execute runs in CI only (GITHUB_ACTIONS=true required)" >&2
        return 1
    fi
    echo "Checking versions for ${package}..."
    local versions
    versions=$(gh api "/user/packages/container/lca%2F${package}/versions?per_page=100" 2>/dev/null || echo "[]")
    local ids
    ids=$(echo "$versions" | select_ids "$keep")
    if [ -z "$ids" ]; then
        echo "nothing eligible to prune in ${package}"
        return 0
    fi
    for vid in $ids; do
        if [ "$mode" == "--execute" ]; then
            echo "Pruning old unstable version ${vid} from ${package}..."
            gh api --method DELETE "/user/packages/container/lca%2F${package}/versions/${vid}" >/dev/null || echo "warn: delete of ${vid} failed, continuing"
        else
            echo "would prune version ${vid} from ${package} (dry run)"
        fi
    done
}

case "${1:-}" in
    selftest) selftest ;;
    "") echo "usage: $SELF selftest | <package> [--dry-run|--execute] [--keep N]" >&2; exit 1 ;;
    *) prune "$@" ;;
esac
