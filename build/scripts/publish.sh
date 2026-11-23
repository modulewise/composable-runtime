#!/usr/bin/env bash

# Managed by componentized/build and synced into each repository by `make update-build`. Change it
# in https://github.com/componentized/build, `make check-build` fails when a copy is changed.

# Publish a built component to an OCI registry, sign it with cosign, and attest its SBOM.
#
#   VERSION=<version> REPOSITORY=<repository> build/scripts/publish.sh <file>
#
# The file is one of the files built into the components directory, components are in a directory
# of their own, the interface is not, e.g. gate.wasm from target/components/gate/gate.wasm and
# interface.wasm from target/components/interface.wasm. The interface is published to the
# repository, each component to a repository of its own under it, a debug build is tagged with a
# `_debug` suffix. A debug build identical to the release build adds nothing, e.g. components
# without debug info, and is not published.
#
# The SBOM built beside the file, e.g. gate.cdx.json for gate.wasm, see build/scripts/sbom.sh, names
# the component it describes, by the directory's id of the package, its version and its image, and is
# attested to the image with cosign, a signed CycloneDX attestation, `cosign verify-attestation --type
# cyclonedx` verifies it.
#
#   interface.wasm   -> ${REPOSITORY}:0.1.0
#   gate.wasm        -> ${REPOSITORY}/gate:0.1.0
#   gate.debug.wasm  -> ${REPOSITORY}/gate:0.1.0_debug
#
# VERSION            the version to publish, with or without a leading `v`, e.g. v0.1.0 or 0.1.0. The
#                    org.opencontainers.image.version annotation always has the `v`, the tag never
#                    does, e.g. v0.1.0 is tagged 0.1.0
# REPOSITORY         the repository to publish to, e.g. ghcr.io/componentized/http
# GITHUB_REPOSITORY  the GitHub repository the components are built from, e.g. componentized/http
# COMPONENTS_DIR     the directory the components are built into, defaults to target/components
# COMPONENT_PATH     the directory of the component's sources, e.g. examples/hello, linked from the
#                    image, defaults to components/<component>
# SIGN               sign the published components with cosign, `false` to push without signing,
#                    e.g. to a local registry
# PUBLISH_LOG        append the published file, its size in bytes and its image to this file, e.g.
#                    `gate.wasm 1234567 ghcr.io/componentized/http/gate:0.1.0@sha256:...`
# COSIGN             the cosign binary, defaults to cosign on the PATH
# SBOM_DIR           the directory the SBOM published with the file is written to, laid out like the
#                    components directory, e.g. gate/gate.debug.cdx.json for gate.debug.wasm and
#                    interface.cdx.json for interface.wasm, defaults to target/sbom/published
# DRY_RUN            `true` to show what would be published without pushing or signing anything,
#                    the image is logged with a `dry-run` digest

set -euo pipefail

cd "$(dirname "$0")/../.."

file="${1:-}"
if [[ -z "$file" ]]; then
    echo "usage: $0 <file>, e.g. interface.wasm or gate.wasm" >&2
    exit 1
fi
: "${VERSION:?VERSION is undefined}"
: "${REPOSITORY:?REPOSITORY is undefined}"
GITHUB_REPOSITORY="${GITHUB_REPOSITORY:-componentized/$(basename "$(git rev-parse --show-toplevel)")}"
COMPONENTS_DIR="${COMPONENTS_DIR:-target/components}"
SIGN="${SIGN:-true}"
PUBLISH_LOG="${PUBLISH_LOG:-}"
SBOM_DIR="${SBOM_DIR:-target/sbom/published}"
COSIGN="${COSIGN:-cosign}"
DRY_RUN="${DRY_RUN:-false}"

component="${file%.wasm}"
component="${component%.debug}"
COMPONENT_PATH="${COMPONENT_PATH-components/${component}}"
debug=""
[[ "$file" == *.debug.wasm ]] && debug=true

# the directory's id of the package, e.g. componentized:http for the interface of componentized/http
# and componentized:http-gate for its gate component
if [[ "$file" == interface.wasm ]]; then
    component_file="$file"
    id="${GITHUB_REPOSITORY/\//:}"
else
    component_file="${component}/${file}"
    id="${GITHUB_REPOSITORY/\//:}-${component}"
fi
title="$id"
[[ -n "$debug" ]] && title="${title} (debug)"
sbom="${COMPONENTS_DIR}/${component_file%.wasm}.cdx.json"

if [[ -n "$debug" ]] && cmp -s "${COMPONENTS_DIR}/${component_file}" "${COMPONENTS_DIR}/${component}/${component}.wasm"; then
    echo "Not publishing ${file} as it is identical to ${component}.wasm"
    exit 0
fi

# the description is the line following the title in the readme
readme="${COMPONENTS_DIR}/$(dirname "$component_file")/README.md"
description=$(sed -n 3p "$readme")

commit=$(git rev-parse HEAD)
revision="$commit"
git diff --quiet HEAD || revision="${commit}+dirty"
url="https://github.com/${GITHUB_REPOSITORY}/tree/${commit}"
[[ -n "$COMPONENT_PATH" && -f "${COMPONENT_PATH}/README.md" ]] && url="${url}/${COMPONENT_PATH}"

# the version has a leading `v`, the tag does not, whether or not VERSION has one
component_version="v${VERSION#v}"
[[ -n "$debug" ]] && component_version="${component_version}+debug"
tag="${component_version#v}"
tag="${tag//+/_}"

if [[ "$file" == interface.wasm ]]; then
    image="${REPOSITORY}:${tag}"
else
    image="${REPOSITORY}/${component}:${tag}"
fi

echo "::group::${file} -> ${image}"
# close the group when the push or signing fails, so the error isn't hidden in it
trap 'echo "::endgroup::"' EXIT

annotations=(
    --annotation="org.opencontainers.image.title=${title}"
    --annotation="org.opencontainers.image.description=${description}"
    --annotation="org.opencontainers.image.version=${component_version}"
    --annotation="org.opencontainers.image.url=${url}"
    --annotation="org.opencontainers.image.source=https://github.com/${GITHUB_REPOSITORY}.git"
    --annotation="org.opencontainers.image.revision=${revision}"
    --annotation="org.opencontainers.image.licenses=Apache-2.0"
)

if [[ "$DRY_RUN" == true ]]; then
    echo "Not pushing ${COMPONENTS_DIR}/${component_file} to ${image}, DRY_RUN=${DRY_RUN}"
    printf '  %s\n' "${annotations[@]}"
    digest=dry-run
else
    digest=$(
        wkg oci push \
            "${annotations[@]}" \
            "$image" \
            "${COMPONENTS_DIR}/${component_file}" \
            2>&1 \
            | tee /dev/stderr \
            | grep -o 'sha256:[a-f0-9]\{64\}'
    )
fi

if [[ -n "$PUBLISH_LOG" ]]; then
    size=$(wc -c < "${COMPONENTS_DIR}/${component_file}" | tr -d ' ')
    echo "${file} ${size} ${image}@${digest}" >> "$PUBLISH_LOG"
fi

# the SBOM of the component published, the component already named by its id, with its version and
# image, e.g. pkg:oci/gate@sha256%3A...?repository_url=ghcr.io/componentized/http/gate&tag=0.1.0
published_sbom="${SBOM_DIR}/${component_file%.wasm}.cdx.json"
mkdir -p "$(dirname "$published_sbom")"
repository="${image%:*}"
purl="pkg:oci/${repository##*/}"
[[ "$digest" == sha256:* ]] && purl="${purl}@${digest/:/%3A}"
purl="${purl}?repository_url=${repository}&tag=${tag}"
jq \
    --arg version "${component_version#v}" \
    --arg purl "$purl" \
    --arg description "$description" \
    --arg source "https://github.com/${GITHUB_REPOSITORY}.git" \
    --arg url "$url" \
    --arg revision "$revision" \
    '.metadata.component += {
        version: $version,
        description: $description,
        purl: $purl,
        licenses: [{expression: "Apache-2.0"}],
        externalReferences: [{type: "vcs", url: $source, comment: "revision \($revision)"}, {type: "website", url: $url}]
    }' "$sbom" > "$published_sbom"

if [[ "$DRY_RUN" == true ]]; then
    echo "Not signing ${image}@${digest}, nor attesting its SBOM ${published_sbom}, DRY_RUN=${DRY_RUN}"
elif [[ "$SIGN" == true ]]; then
    "$COSIGN" sign --yes "${image}@${digest}"
    "$COSIGN" attest --yes --type cyclonedx --predicate "$published_sbom" "${image}@${digest}"
else
    echo "Not signing ${image}@${digest}, nor attesting its SBOM ${published_sbom}, SIGN=${SIGN}"
fi
