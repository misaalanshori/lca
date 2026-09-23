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
# auth:  GH_TOKEN from `gh auth token` (the same credential git uses).
set -euo pipefail
cd "$(dirname "$0")/.."

image="${1:?image, e.g. misaalanshori/lca-openai-compatible}"
tag="${2:?tag, e.g. abi-0.1}"
wasm="${3:?path to the component}"
manifest_toml="${4:?path to extension.toml}"

sha() { python3 -c "import hashlib,sys;print('sha256:'+hashlib.sha256(open(sys.argv[1],'rb').read()).hexdigest())" "$1"; }
json_get() { python3 -c "import json,sys;sys.stdout.write(json.load(sys.stdin)[sys.argv[1]])" "$1"; }

# A push needs an authenticated token: GH_TOKEN (a PAT from
# `gh auth token`) plus the registry user makes the token request
# itself authenticated; without them only the anonymous pull token is
# available and the manifest PUT is refused.
token_args=()
if [[ -n "${GH_TOKEN:-}" ]]; then
  token_args=(-u "${LCA_OCI_USER:-oauth2}:${GH_TOKEN}")
fi
token=$(curl -fsS --fail-with-body "${token_args[@]}" \
  "https://ghcr.io/token?service=ghcr.io&scope=repository:${image}:pull,push" \
  | json_get token)
auth=(-H "Authorization: Bearer ${token}")

upload_blob() {
  local file="$1" digest
  digest=$(sha "$file")
  curl -fsS --fail-with-body -X POST "${auth[@]}" \
    -H "Content-Type: application/octet-stream" \
    -H "Content-Length: $(wc -c < "$file")" \
    --data-binary "@${file}" \
    "https://ghcr.io/v2/${image}/blobs/uploads/?digest=${digest}" \
    -o /tmp/lca-publish-blob.out || { cat /tmp/lca-publish-blob.out >&2; exit 1; }
  echo "${digest}"
}

config_digest=$(upload_blob "$manifest_toml")
layer_digest=$(upload_blob "$wasm")
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

curl -fsS --fail-with-body -X PUT "${auth[@]}" \
  -H "Content-Type: application/vnd.oci.image.manifest.v1+json" \
  --data-binary "${image_manifest}" \
  "https://ghcr.io/v2/${image}/manifests/${tag}" \
  -o /tmp/lca-publish-manifest.out || { cat /tmp/lca-publish-manifest.out >&2; exit 1; }
echo "published ghcr.io/${image}:${tag} (${layer_digest})"
