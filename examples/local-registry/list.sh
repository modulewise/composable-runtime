#!/bin/bash

set -euo pipefail

REGISTRY="${REGISTRY:-localhost:5001}"
BASE="http://$REGISTRY/v2"

if ! curl -sf "$BASE/" -o /dev/null; then
  echo "Error: no registry responding at $BASE (./setup.sh starts one)"
  exit 1
fi

for repository in $(curl -s "$BASE/_catalog" | jq -r '.repositories[]?'); do
  for tag in $(curl -s "$BASE/$repository/tags/list" | jq -r '.tags[]?'); do
    echo "$repository:$tag"

    # The layer digest, which the runtime's cache is keyed on. The manifest
    # digest that `wkg oci push` reports is a separate value.
    curl -s -H "Accept: application/vnd.oci.image.manifest.v1+json" \
      "$BASE/$repository/manifests/$tag" |
      jq -r '.layers[]? | "    \(.mediaType)  \(.size) bytes  \(.digest)"'
  done
done
