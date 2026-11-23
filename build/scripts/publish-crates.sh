#!/usr/bin/env bash

# Managed by componentized/build and synced into each repository by `make update-build`. Change it
# in https://github.com/componentized/build, `make check-build` fails when a copy is changed.

# Publish crates of the workspace to crates.io, skipping those already published, retrying a failure.
#
#   [DRY_RUN=true] build/scripts/publish-crates.sh <crate>...
#
# cargo publishes the crates together, a library before the crates that depend on it, waiting for each
# to show up in the index before publishing its dependents. A crate can still fail to publish, e.g.
# the index is slow to show a library its cli depends on, or crates.io is unavailable or rate limits
# the upload, leaving the crates before it published. cargo refuses to publish a version again, so
# each attempt publishes only the crates whose version isn't in the crates.io index yet, and running
# it again, e.g. rerunning the release workflow, finishes what a failed run started.
#
# DRY_RUN           true checks the crates with `cargo publish --dry-run` rather than publishing them
# PUBLISH_ATTEMPTS  the attempts to publish the crates, defaults to 3
# PUBLISH_DELAY     the seconds to wait before the second attempt, doubled for each attempt after it,
#                   defaults to 30
# CRATES_INDEX      the sparse index of the registry, defaults to https://index.crates.io

set -euo pipefail

cd "$(dirname "$0")/../.."

DRY_RUN="${DRY_RUN:-false}"
PUBLISH_ATTEMPTS="${PUBLISH_ATTEMPTS:-3}"
PUBLISH_DELAY="${PUBLISH_DELAY:-30}"
CRATES_INDEX="${CRATES_INDEX:-https://index.crates.io}"

if [[ $# == 0 ]]; then
    echo "usage: $0 <crate>..., e.g. componentized-constants componentized-constants-cli" >&2
    exit 1
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

metadata=$(cargo metadata --no-deps --format-version 1 --locked)

# the version of a crate of the workspace, e.g. 0.2.0
version_of() {
    jq -r --arg name "$1" '.packages[] | select(.name == $name) | .version' <<< "$metadata"
}

# the path of a crate in a sparse index, e.g. co/mp/componentized-build-cli, 3/s/syn
index_path() {
    local name
    name=$(tr '[:upper:]' '[:lower:]' <<< "$1")
    case "${#name}" in
        1) echo "1/${name}" ;;
        2) echo "2/${name}" ;;
        3) echo "3/${name:0:1}/${name}" ;;
        *) echo "${name:0:2}/${name:2:2}/${name}" ;;
    esac
}

# whether a version of a crate is in the index, failing when the index can't be read
published() {
    local status
    status=$(curl -sSL -o "$work/index" -w '%{http_code}' "${CRATES_INDEX}/$(index_path "$1")" || true)
    case "$status" in
        200) jq -se --arg version "$2" 'any(.[]; .vers == $version)' "$work/index" > /dev/null ;;
        # a crate never published
        404) return 1 ;;
        *)
            echo "unable to read ${CRATES_INDEX}/$(index_path "$1"), HTTP ${status}" >&2
            exit 1
            ;;
    esac
}

dry_run=()
[[ "$DRY_RUN" == true ]] && dry_run=(--dry-run)

for (( attempt = 1; ; attempt++ )); do
    pending=()
    for crate in "$@"; do
        version=$(version_of "$crate")
        if [[ -z "$version" ]]; then
            echo "unable to find the crate ${crate} in the workspace" >&2
            exit 1
        fi
        if published "$crate" "$version"; then
            echo "${crate} ${version} is already published, skipped"
        else
            pending+=("$crate")
        fi
    done
    if [[ ${#pending[@]} == 0 ]]; then
        echo "the crates are published"
        exit 0
    fi

    echo "publishing ${pending[*]}, attempt ${attempt} of ${PUBLISH_ATTEMPTS}"
    if cargo publish --locked ${dry_run[@]+"${dry_run[@]}"} "${pending[@]/#/--package=}"; then
        exit 0
    fi
    # a dry run publishes nothing, it fails the same way again
    if [[ "$DRY_RUN" == true || "$attempt" -ge "$PUBLISH_ATTEMPTS" ]]; then
        echo "unable to publish ${pending[*]}, run it again to publish the crates not yet published" >&2
        exit 1
    fi
    delay=$(( PUBLISH_DELAY * (1 << (attempt - 1)) ))
    echo "publishing failed, trying again in ${delay}s" >&2
    sleep "$delay"
done
