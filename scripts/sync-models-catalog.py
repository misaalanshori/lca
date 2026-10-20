#!/usr/bin/env python3
"""Download `models.dev/api.json`, strip it, and write the gzipped catalog.

gh #260: the embedded model catalog is a build artifact, never a committed
file. `crates/lca-registry/build.rs` runs this script (or, when it is not
available, does the same work in-process) and embeds the result with
`include_bytes!`.

What survives the strip: `cost`, `limit`, `reasoning`, and
`canonical_model_id` - the four fields `lca_registry::models` reads.
Everything descriptive (`description`, `name`, `doc`, `npm`, `env`,
`modalities`, `release_date`, ...) is dropped, because the binary pays for
every byte and nothing reads them.

Usage:
    scripts/sync-models-catalog.py --out target/models_catalog.json.gz
    scripts/sync-models-catalog.py --out out.json.gz --from api.json
    scripts/sync-models-catalog.py --out out.json.gz --source https://host/api.json

Exit codes: 0 written, 2 nothing written (the caller keeps its stub), 3 a
local input was unreadable. The script never raises a traceback at the
caller: `build.rs` treats any failure as "stay on the stub".
"""

from __future__ import annotations

import argparse
import gzip
import json
import sys
import urllib.error
import urllib.request

DEFAULT_SOURCE = "https://models.dev/api.json"

# The model fields the runtime reads. Anything else is dead weight in a
# static binary.
MODEL_FIELDS = ("canonical_model_id", "limit", "cost", "reasoning")
COST_FIELDS = ("input", "output", "cache_read", "cache_write")
LIMIT_FIELDS = ("context", "output")


def strip(document: dict) -> dict:
    """Reduce a raw `models.dev/api.json` document to the fields we read.

    Providers keep their identity (the key) and their model map. A model
    keeps only the four fields above, and `cost`/`limit` keep only the
    members LCA can bill or divide by.
    """
    stripped: dict = {}
    for provider_id, provider in document.items():
        if not isinstance(provider, dict):
            continue
        models = provider.get("models")
        if not isinstance(models, dict):
            models = {}
        kept_models: dict = {}
        for model_id, model in models.items():
            if not isinstance(model, dict):
                continue
            kept = {field: model[field] for field in MODEL_FIELDS if field in model}
            cost = kept.get("cost")
            if isinstance(cost, dict):
                kept["cost"] = {k: cost[k] for k in COST_FIELDS if k in cost}
                if not kept["cost"]:
                    del kept["cost"]
            limit = kept.get("limit")
            if isinstance(limit, dict):
                kept["limit"] = {k: limit[k] for k in LIMIT_FIELDS if k in limit}
                if not kept["limit"]:
                    del kept["limit"]
            # A model with nothing billable and no limit is a name and
            # nothing else: drop the row rather than pay for it.
            if not kept:
                continue
            kept_models[model_id] = kept
        stripped[provider_id] = {"models": kept_models}
    return stripped


def read_source(source: str, local: str | None) -> dict:
    if local:
        with open(local, "rb") as handle:
            raw = handle.read()
        # Accept a gzipped input too. `LCA_MODELS_CATALOG` is used by release
        # builds to pin the catalog, and a pinned artifact is most useful as
        # the already-stripped ~134 KiB gzip rather than the raw 5.6 MB
        # download - a gzip is what the build embeds, so pinning it removes
        # the strip step from the reproducibility argument entirely.
        if raw[:2] == b"\x1f\x8b":
            raw = gzip.decompress(raw)
        return json.loads(raw)
    request = urllib.request.Request(
        source,
        headers={"user-agent": "lca-build-catalog/1"},
    )
    with urllib.request.urlopen(request, timeout=30) as response:  # noqa: S310 - the URL is a constant or an explicit operator flag
        return json.load(response)


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True, help="where to write the gzip")
    parser.add_argument(
        "--from",
        dest="local",
        default=None,
        help="read a local api.json instead of the network (reproducible builds, tests)",
    )
    parser.add_argument(
        "--source",
        default=DEFAULT_SOURCE,
        help=f"the catalog URL (default {DEFAULT_SOURCE})",
    )
    args = parser.parse_args(argv)

    try:
        document = read_source(args.source, args.local)
    except (OSError, ValueError, urllib.error.URLError) as error:
        print(f"sync-models-catalog: no catalog ({error})", file=sys.stderr)
        return 2 if args.local is None else 3
    if not isinstance(document, dict) or not document:
        print("sync-models-catalog: catalog is not a JSON object", file=sys.stderr)
        return 2

    stripped = strip(document)
    payload = json.dumps(stripped, separators=(",", ":"), sort_keys=True).encode()
    # mtime=0: a byte-identical input must produce a byte-identical gzip,
    # or the reproducible double build (NFR-17) fails on a header field.
    blob = gzip.compress(payload, compresslevel=9, mtime=0)
    with open(args.out, "wb") as handle:
        handle.write(blob)

    providers = len(stripped)
    models = sum(len(entry["models"]) for entry in stripped.values())
    print(
        f"sync-models-catalog: {providers} providers, {models} models, "
        f"{len(payload)} bytes raw, {len(blob)} bytes gzipped",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
