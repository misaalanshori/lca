#!/usr/bin/env bash
# Publish one extension as an OCI artifact to ghcr.io (Phase 5's
# "publish the reference extensions to a public registry").
#
# The artifact follows lca-registry's pull convention: the OCI config
# blob carries extension.toml, layer0 carries the component
# (application/wasm), and both blobs are uploaded before the manifest
# that references them. Two tags per release, per the authoring guide:
# an immutable version tag and the moving `abi-X.Y` line tag that
# `ext update` resolves (ADR-0009's scheme).
#
# usage: publish-oci.sh <image> <tag> <component.wasm> <extension.toml>
# auth:  GH_TOKEN + LCA_OCI_USER, so the registry mints a token with
#        push scope (an anonymous request only ever gets pull).
set -euo pipefail
cd "$(dirname "$0")/.."

image="${1:?image, e.g. misaalanshori/lca/openai-compatible}"
tag="${2:?tag, e.g. abi-0.1}"
wasm="${3:?path to the component}"
manifest_toml="${4:?path to extension.toml}"

sha() { python3 -c "import hashlib,sys;print('sha256:'+hashlib.sha256(open(sys.argv[1],'rb').read()).hexdigest())" "$1"; }

token_args=()
if [[ -n "${GH_TOKEN:-}" ]]; then
  token_args=(-u "${LCA_OCI_USER:-oauth2}:${GH_TOKEN}")
fi
token=$(curl -fsS "${token_args[@]}" \
  "https://ghcr.io/token?service=ghcr.io&scope=repository:${image}:pull,push" \
  | python3 -c "import json,sys;sys.stdout.write(json.load(sys.stdin)['token'])")
auth=(-H "Authorization: Bearer ${token}")

# status-code check the old curl on some runners can manage
put() { # put <url> <content-type> <file>
  local code
  code=$(curl -sS "${auth[@]}" -H "Content-Type: $2" --data-binary "@$3" "$1" \
    -o /tmp/lca-publish-body.out -w "%{http_code}")
  if [[ "$code" != 2* ]]; then
    echo "request failed: HTTP $code ($1)" >&2
    cat /tmp/lca-publish-body.out >&2
    exit 1
  fi
}
post() { # post <url> <file>
  local code
  code=$(curl -sS "${auth[@]}" -H "Content-Type: application/octet-stream" \
    --data-binary "@$2" "$1" -o /tmp/lca-publish-body.out -w "%{http_code}")
  if [[ "$code" != 2* ]]; then
    echo "upload failed: HTTP $code ($1)" >&2
    cat /tmp/lca-publish-body.out >&2
    exit 1
  fi
}

config_digest=$(sha "$manifest_toml")
layer_digest=$(sha "$wasm")
post "https://ghcr.io/v2/${image}/blobs/uploads/?digest=${config_digest}" "$manifest_toml"
post "https://ghcr.io/v2/${image}/blobs/uploads/?digest=${layer_digest}" "$wasm"
config_size=$(wc -c < "$manifest_toml")
layer_size=$(wc -c < "$wasm")

image_manifest=$(cat <<JSON
{
  "schemaVersion": 2,
  "mediaType": "application/vnd.oci.image.manifest.v1+json",
  "config": {
    "mediaType": "text/plain",
    "digest": "${config_digest}",
    "size": ${config_size}
  },
  "layers": [
    {
      "mediaType": "application/wasm",
      "digest": "${layer_digest}",
      "size": ${layer_size}
    }
  ]
}
JSON
)
printf '%s' "${image_manifest}" > /tmp/lca-publish-manifest.json
put "https://ghcr.io/v2/${image}/manifests/${tag}" \
  "application/vnd.oci.image.manifest.v1+json" \
  /tmp/lca-publish-manifest.json
echo "published ghcr.io/${image}:${tag} (${layer_digest})"
