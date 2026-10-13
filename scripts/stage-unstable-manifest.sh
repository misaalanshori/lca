#!/usr/bin/env bash
# Stage one extension manifest with a baked unstable version
# (gh #251): copy <src> to <dst> with the first `version = "..."`
# line replaced by <version>. The rolling channel publishes the
# staged copy, never a rewritten repo file.
#
# usage: stage-unstable-manifest.sh <src> <dst> <version>
set -euo pipefail

src="${1:?src extension.toml}"
dst="${2:?dst staged extension.toml}"
ver="${3:?baked version, e.g. 1.0.1-unstable.b3f4a1c}"

mkdir -p "$(dirname "$dst")"
python3 - "$src" "$dst" "$ver" <<'PY'
import re
import sys
src, dst, ver = sys.argv[1], sys.argv[2], sys.argv[3]
content = open(src, "r", encoding="utf-8").read()
updated, count = re.subn(r'(?m)^version\s*=\s*"[^"]+"', f'version = "{ver}"', content, count=1)
if count != 1:
    print(f"stage-unstable-manifest: no version line in {src}", file=sys.stderr)
    sys.exit(1)
open(dst, "w", encoding="utf-8").write(updated)
PY
