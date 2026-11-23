#!/usr/bin/env bash

# Managed by componentized/build and synced into each repository by `make update-build`. Change it
# in https://github.com/componentized/build, `make check-build` fails when a copy is changed.

# Pack the components with an SBOM for each, the SBOM it's published with when it is, into a tar, e.g.
# the components.tar of a release.
#
#   build/scripts/pack-components.sh <components dir> <published sboms dir> <tar>
#
# e.g. `build/scripts/pack-components.sh target/components target/sbom/published components.tar`. The
# published SBOMs are laid out like the components directory, see build/scripts/publish.sh. Each wasm
# file is packed with, beside it as <name>.cdx.json:
#
#   - the SBOM it's published with, naming its version and image, e.g. greeter.wasm's
#   - for a debug build not published as it's identical to the release build, the release build's
#     published SBOM, it's the same file, e.g. defaults.debug.wasm's is defaults.wasm's
#   - otherwise the SBOM of the build, for a file that isn't published, e.g. a component pulled with
#     wkg, whose SBOM is the one it's published with where cosign verifies it, see
#     build/scripts/sbom.sh, or an internal component
#
# The components directory isn't changed, the tar is packed from a copy of it.

set -euo pipefail

components="${1:-}"
published="${2:-}"
tar="${3:-}"
if [[ -z "$components" || -z "$published" || -z "$tar" ]]; then
    echo "usage: $0 <components dir> <published sboms dir> <tar>, e.g. target/components target/sbom/published components.tar" >&2
    exit 1
fi

staging=$(mktemp -d)
trap 'rm -rf "$staging"' EXIT
cp -R "${components}/." "$staging"

while IFS= read -r wasm; do
    file="${wasm#./}"
    sbom="${file%.wasm}.cdx.json"
    release="${file%.debug.wasm}.wasm"
    if [[ -f "${published}/${sbom}" ]]; then
        cp "${published}/${sbom}" "${staging}/${sbom}"
        echo "${file}: the SBOM it's published with"
    elif [[ "$file" == *.debug.wasm && -f "${published}/${release%.wasm}.cdx.json" ]] \
        && cmp -s "${staging}/${file}" "${staging}/${release}"; then
        cp "${published}/${release%.wasm}.cdx.json" "${staging}/${sbom}"
        echo "${file}: the SBOM ${release} is published with, it's the same file"
    elif [[ -f "${staging}/${sbom}" ]]; then
        echo "${file}: the SBOM of the build, it isn't published"
    else
        echo "${file}: no SBOM" >&2
        exit 1
    fi
done < <(cd "$staging" && find . -name '*.wasm' | sort)

tar -cf "$tar" -C "$staging" .
