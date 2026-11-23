#!/usr/bin/env bash

# Managed by componentized/build and synced into each repository by `make update-build`. Change it
# in https://github.com/componentized/build, `make check-build` fails when a copy is changed.

# Check the files synced from componentized/build are unchanged since build/scripts/update-build.sh
# synced them, the version in build/VERSION.
#
#   build/scripts/check-build.sh
#
# Fails when a synced file was changed or removed, or a file was added to build/. A change to the
# build belongs in componentized/build, released and synced to each repository, rather than here.
# The current version the repository keeps in a synced file, the default of the bump-version
# workflow, is its own, and may change.

set -euo pipefail

cd "$(dirname "$0")/../.."

# the line of a synced file the repository keeps current, the default version of the bump-version
# workflow, e.g. `default: "0.1.0-dev" # the current version, kept current by
# build/scripts/bump-version.sh`. Its value is the repository's own, left out of the checksum.
CURRENT_VERSION_RE='"[^"]*"( # the current version, kept current by build/scripts/bump-version\.sh)$'
# the checksum of a synced file, without the value the repository keeps current
checksum() {
    sed -E "s|${CURRENT_VERSION_RE}|\"\"\1|" "$1" | shasum -a 256 | cut -d ' ' -f 1
}

# componentized/build itself, the source of the files rather than a copy of them
if [[ -f SYNC ]]; then
    echo "the build repository, nothing synced to check"
    exit 0
fi

if [[ ! -f build/MANIFEST || ! -f build/VERSION ]]; then
    echo "unable to find build/MANIFEST and build/VERSION, run \`make update-build\` to sync the build" >&2
    exit 1
fi
version=$(cat build/VERSION)

status=0
while read -r sum path; do
    if [[ ! -f "$path" ]]; then
        echo "${path}: removed"
        status=1
    elif [[ "$(checksum "$path")" != "$sum" ]]; then
        echo "${path}: changed"
        status=1
    fi
done < build/MANIFEST

# build/ only holds synced files
while read -r path; do
    case "$path" in
        build/VERSION | build/MANIFEST) ;;
        *)
            if ! grep -qE "^[0-9a-f]{64}  ${path//./\\.}$" build/MANIFEST; then
                echo "${path}: not synced from componentized/build ${version}"
                status=1
            fi
            ;;
    esac
done < <(find build -type f | sort)

if [[ "$status" != 0 ]]; then
    echo "the build differs from componentized/build ${version}. Make the change in componentized/build and update to its release, or run \`make update-build BUILD_VERSION=${version}\` to undo it." >&2
fi
exit "$status"
