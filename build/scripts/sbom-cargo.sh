#!/usr/bin/env bash

# Managed by componentized/build and synced into each repository by `make update-build`. Change it
# in https://github.com/componentized/build, `make check-build` fails when a copy is changed.

# Generate the CycloneDX SBOM of each package of the cargo workspace, built for a target, with
# cargo-cyclonedx.
#
#   CARGO_CYCLONEDX=<cargo-cyclonedx> build/scripts/sbom-cargo.sh <target> <dir>
#
# Writes <dir>/<package>.cdx.json for each package of the workspace, e.g.
# target/sbom/cargo/wasm32-wasip2/greeter.cdx.json, the dependencies the package has built for the
# target, without the build dependencies. build/scripts/sbom.sh makes the SBOM of a component from
# its package's. cargo-cyclonedx writes the SBOM of each package beside its Cargo.toml, under a name of
# its own for each target so that two targets can be generated at once, and they're moved to <dir>.
#
# CARGO_CYCLONEDX    the cargo-cyclonedx binary, run as `$CARGO_CYCLONEDX cyclonedx`
# SOURCE_DATE_EPOCH  the time the SBOMs are generated at, e.g. the time of the commit, for SBOMs that
#                    are the same each time they are generated

set -euo pipefail

cd "$(dirname "$0")/../.."

target="${1:-}"
dir="${2:-}"
if [[ -z "$target" || -z "$dir" ]]; then
    echo "usage: $0 <target> <dir>, e.g. wasm32-wasip2 target/sbom/cargo/wasm32-wasip2" >&2
    exit 1
fi
: "${CARGO_CYCLONEDX:?CARGO_CYCLONEDX is undefined}"

# the name of each package's SBOM beside its Cargo.toml, cargo-cyclonedx adds `.json`
name=".sbom-${target}"

# the name and directory of each package of the workspace
packages=$(
    cargo metadata --no-deps --format-version 1 \
        | jq -r '.workspace_members as $members | .packages[] | select(.id | IN($members[])) | "\(.name)\t\(.manifest_path | rtrimstr("/Cargo.toml"))"'
)

# remove what's left beside the packages' Cargo.toml when generating fails
trap 'while IFS=$'"'"'\t'"'"' read -r _ path; do rm -f "${path}/${name}.json"; done <<< "$packages"' EXIT

"$CARGO_CYCLONEDX" cyclonedx \
    --manifest-path Cargo.toml \
    --format json \
    --spec-version 1.6 \
    --target "$target" \
    --no-build-deps \
    --override-filename "$name" \
    --quiet

mkdir -p "$dir"
while IFS=$'\t' read -r package path; do
    mv "${path}/${name}.json" "${dir}/${package}.cdx.json"
done <<< "$packages"
