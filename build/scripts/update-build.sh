#!/usr/bin/env bash

# Managed by componentized/build and synced into each repository by `make update-build`. Change it
# in https://github.com/componentized/build, `make check-build` fails when a copy is changed.

# Update the build synced from componentized/build to one of its releases, or to a commit of it.
#
#   build/scripts/update-build.sh [<version>]
#
# Downloads the release or commit, removes the files synced from the previous version, listed in
# build/MANIFEST, copies in the files of the release, listed in its SYNC file, and records the
# version in build/VERSION and a checksum of each file in build/MANIFEST, checked by
# build/scripts/check-build.sh. The current version the repository keeps in a synced file, the
# default of the bump-version workflow, is carried forward. A tool the repository's tools/Cargo.toml
# overrides is removed from it once the release's build/tools/Cargo.toml pins the same version or a
# newer one.
#
# <version>         the release to update to, e.g. v0.2.0, or a branch, tag, commit or other ref of
#                   the build's repository, e.g. main, 6f2b296 or refs/pull/23/head, recorded as its
#                   commit in build/VERSION. The latest release by default, unless build/VERSION is a
#                   commit newer than it, e.g. of main ahead of the latest release, which is kept: a
#                   release is newer when it includes the commit, or when it diverged from the
#                   commit, e.g. merged from a pull request by a squash, and was committed after it
# BUILD_REPOSITORY  the GitHub repository of the build, defaults to componentized/build
# BUILD_SOURCE      a checkout of the build repository to update from in place of a release, e.g. to
#                   try a change before releasing it, recorded as its commit in build/VERSION
# GH_TOKEN          a GitHub token for the API, optional, raises its rate limit

set -euo pipefail

# the update replaces this script, bash reads a script while it runs it, so the whole script is in a
# block bash reads before running any of it
{

cd "$(dirname "$0")/../.."

BUILD_REPOSITORY="${BUILD_REPOSITORY:-componentized/build}"
BUILD_SOURCE="${BUILD_SOURCE:-}"
version="${1:-}"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

if [[ -n "$BUILD_SOURCE" ]]; then
    source=$(cd "$BUILD_SOURCE" && pwd)
    if [[ -z "$version" ]]; then
        version="$(git -C "$source" rev-parse --verify --quiet HEAD || echo local)"
        [[ -n "$(git -C "$source" status --porcelain 2> /dev/null)" ]] && version="${version}+dirty"
    fi
else
    auth=()
    [[ -n "${GH_TOKEN:-}" ]] && auth=(--header "Authorization: Bearer ${GH_TOKEN}")
    # the GitHub API of the build's repository, e.g. `api releases/latest`
    api() {
        curl -fsSL ${auth[@]+"${auth[@]}"} "https://api.github.com/repos/${BUILD_REPOSITORY}/$1"
    }
    RELEASE_RE='^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$'
    COMMIT_RE='^[0-9a-f]{40}$'

    # whether a release is newer than a commit: it includes the commit, or diverged from it and was
    # committed after it, e.g. a commit of a pull request merged by a squash. A commit the repository
    # doesn't have, e.g. a local commit synced with BUILD_SOURCE, is older than any release.
    newer() {
        local compare status
        if ! compare=$(api "compare/$2...$1" 2> /dev/null); then
            echo "unable to compare ${BUILD_REPOSITORY} $2 to $1" >&2
            return 0
        fi
        status=$(jq -r .status <<< "$compare")
        case "$status" in
            ahead | identical) return 0 ;;
            behind) return 1 ;;
            diverged)
                [[ "$(api "commits/$1" | jq -r .commit.committer.date)" > "$(jq -r .base_commit.commit.committer.date <<< "$compare")" ]]
                ;;
            *)
                echo "unable to compare ${BUILD_REPOSITORY} $2 to $1, ${status}" >&2
                exit 1
                ;;
        esac
    }

    if [[ -z "$version" ]]; then
        version=$(api releases/latest | jq -r .tag_name)
        # the commit synced, e.g. of main, or from a checkout, which may be dirty
        current=$(cat build/VERSION 2> /dev/null || true)
        current="${current%+dirty}"
        if [[ "$current" =~ $COMMIT_RE ]] && ! newer "$version" "$current"; then
            echo "the build is ${BUILD_REPOSITORY} ${current}, newer than its latest release ${version}"
            exit
        fi
    fi
    if [[ "$version" =~ $RELEASE_RE ]]; then
        archive="refs/tags/${version}"
    else
        # a branch, tag, commit or other ref, recorded as its commit, e.g. main or refs/pull/23/head
        if [[ ! "$version" =~ ^[0-9A-Za-z._/-]+$ ]] || ! commit=$(api "commits/${version}" 2> /dev/null | jq -er .sha); then
            echo "unable to find '${version}' in ${BUILD_REPOSITORY}, a release, e.g. v0.2.0, or a branch, tag, commit or ref, e.g. main" >&2
            exit 1
        fi
        [[ "$commit" != "$version" ]] && echo "${version} is ${BUILD_REPOSITORY} ${commit}"
        version="$commit"
        archive="$commit"
    fi
    source="$work/source"
    mkdir -p "$source"
    echo "downloading ${BUILD_REPOSITORY} ${version}"
    curl -fsSL "https://github.com/${BUILD_REPOSITORY}/archive/${archive}.tar.gz" | tar -xz --strip-components 1 -C "$source"
fi

if [[ ! -f "$source/SYNC" ]]; then
    echo "unable to find the SYNC file of ${BUILD_REPOSITORY} ${version}" >&2
    exit 1
fi

# the line of a synced file the repository keeps current, the default version of the bump-version
# workflow, e.g. `default: "0.1.0-dev" # the current version, kept current by
# build/scripts/bump-version.sh`. Its value is the repository's own, left out of the checksum.
CURRENT_VERSION_RE='"[^"]*"( # the current version, kept current by build/scripts/bump-version\.sh)$'
# the checksum of a synced file, without the value the repository keeps current
checksum() {
    sed -E "s|${CURRENT_VERSION_RE}|\"\"\1|" "$1" | shasum -a 256 | cut -d ' ' -f 1
}

# the current version kept in a file, e.g. `0.1.0-dev`, also from a file of the build before it moved
# into build/, kept current by scripts/bump-version.sh
current_version() {
    [[ -f "$1" ]] && sed -nE 's|.*"([^"]*)" # the current version, kept current by (build/)?scripts/bump-version\.sh$|\1|p' "$1" | head -n 1
}

# the repository's version, the version of its interface package, e.g. `package componentized:http@0.1.0-dev;`
interface_version=$(sed -n 's/^package [^@]*@\(.*\);$/\1/p' wit/worlds.wit 2> /dev/null | head -n 1 || true)

# a path in the repository, relative and inside it, e.g. build/build.mk
check_path() {
    if [[ "$1" == /* || "/$1/" == */../* ]]; then
        echo "refusing to sync '$1', outside the repository" >&2
        exit 1
    fi
}

# the files of the release, each path in SYNC is a file or a directory of files
synced="$work/synced"
: > "$synced"
while read -r path; do
    [[ -z "$path" || "$path" == \#* ]] && continue
    check_path "$path"
    if [[ ! -e "$source/$path" ]]; then
        echo "unable to find ${path}, listed in the SYNC file of ${BUILD_REPOSITORY} ${version}" >&2
        exit 1
    fi
    # a checkout syncs only its tracked and unignored files
    if git -C "$source" rev-parse --is-inside-work-tree > /dev/null 2>&1; then
        git -C "$source" ls-files --cached --others --exclude-standard -- "$path" >> "$synced"
    else
        (cd "$source" && find "$path" -type f) >> "$synced"
    fi
done < "$source/SYNC"
sort -u -o "$synced" "$synced"

# the files synced from the previous version, a file the release no longer has is removed
if [[ -f build/MANIFEST ]]; then
    while read -r _ path; do
        check_path "$path"
        if ! grep -qxF "$path" "$synced"; then
            rm -f "$path"
            echo "removed ${path}"
        fi
    done < build/MANIFEST
fi

while read -r path; do
    mkdir -p "$(dirname "$path")"
    cp "$source/$path" "$path.tmp"
    # the repository's current version rather than the build's, or its interface version for a file
    # it didn't have yet
    current=$(current_version "$path" || true)
    current="${current:-$interface_version}"
    if [[ -n "$current" ]]; then
        sed -E "s|${CURRENT_VERSION_RE}|\"${current}\"\1|" "$path.tmp" > "$path.tmp.current"
        mv "$path.tmp.current" "$path.tmp"
    fi
    if cmp -s "$path.tmp" "$path"; then
        rm "$path.tmp"
    else
        # moved into place rather than overwritten, e.g. this script while it runs
        mv "$path.tmp" "$path"
        echo "updated ${path}"
    fi
    chmod "$( [[ -x "$source/$path" ]] && echo 755 || echo 644 )" "$path"
done < "$synced"

echo "$version" > build/VERSION
while read -r path; do
    echo "$(checksum "$path")  ${path}"
done < "$synced" > build/MANIFEST

# remove the overrides of the tools the build provides that the build has caught up with, pinned to
# the build's version or an older one, e.g. `wasm-tools = "=1.262.0"` when the build pins 1.263.0. A
# newer version stays, the repository is ahead of the build. The tool's dependency and its line in
# [package.metadata.bins] both start with its name, e.g. `wasm-tools = ["wasm-tools"]`.
if [[ -f tools/Cargo.toml ]]; then
    while read -r tool provided; do
        pinned=$(sed -n "s/^${tool} = \"=\(.*\)\"$/\1/p" tools/Cargo.toml)
        [[ -z "$pinned" ]] && continue
        if [[ "$(printf '%s\n%s\n' "$pinned" "$provided" | sort -V | tail -n 1)" == "$provided" ]]; then
            perl -ni -e 'print unless /^\Q'"${tool}"'\E = /' tools/Cargo.toml
            echo "removed ${tool} ${pinned} from tools/Cargo.toml, the build provides ${provided}"
        fi
    done < <(sed -n 's/^\([A-Za-z0-9_-]*\) = "=\(.*\)"$/\1 \2/p' build/tools/Cargo.toml)
fi

echo "updated the build to ${BUILD_REPOSITORY} ${version}"
exit
}
