#!/usr/bin/env bash

# Managed by componentized/build and synced into each repository by `make update-build`. Change it
# in https://github.com/componentized/build, `make check-build` fails when a copy is changed.

# Bump the version of the wit interface package, and of the crates.
#
#   build/scripts/bump-version.sh <new-version>
#
# Updates the package declaration and every reference to the package in tracked files, then
# refreshes the generated wit dependencies. The crates share the interface's version: the
# workspace version the crates inherit, and the workspace's requirements on its own crates that name
# a version, e.g. `componentized-constants = { path = "./crates/componentized-constants", version =
# "=0.2.1-dev" }`, move to the new version too. A release, not a prerelease, also moves the pins of
# the workspace's crates in the tools manifests, e.g. `componentized-build-cli = "=0.1.6"` in
# build/tools/Cargo.toml. A repository without a workspace version only bumps the interface. The default version of the bump-version workflow moves to the new version, the next bump
# is offered from it. Items whose `@since` names an unreleased (prerelease)
# version move to the new version, since they were never published under the old one. Items
# released under the old version keep their `@since`.
#
#   0.1.0-dev -> 0.1.0      releases 0.1.0, `@since(version = 0.1.0-dev)` becomes 0.1.0
#   0.1.0     -> 0.2.0-dev  starts 0.2.0, `@since(version = 0.1.0)` is unchanged
#
# PACKAGE  the interface package, the first package declared in wit/worlds.wit by default, e.g.
#          componentized:valkey

set -euo pipefail

cd "$(dirname "$0")/../.."

PACKAGE="${PACKAGE:-$(sed -n 's/^package \([^@]*\)@.*;$/\1/p' wit/worlds.wit | head -n 1)}"
SEMVER='^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$'

new="${1:-}"
if [[ ! "$new" =~ $SEMVER ]]; then
    echo "usage: $0 <new-version>, e.g. 0.1.0 or 0.2.0-dev" >&2
    exit 1
fi

old=$(sed -n "s/^package ${PACKAGE}@\(.*\);$/\1/p" wit/worlds.wit)
if [[ -z "$old" ]]; then
    echo "unable to find the ${PACKAGE} package declaration in wit/worlds.wit" >&2
    exit 1
fi

# succeeds when version $1 is lower than version $2, a prerelease is lower than its release
version_lt() {
    local a_core="${1%%-*}" b_core="${2%%-*}"
    local a_pre="" b_pre=""
    [[ "$1" == *-* ]] && a_pre="${1#*-}"
    [[ "$2" == *-* ]] && b_pre="${2#*-}"
    if [[ "$a_core" != "$b_core" ]]; then
        local IFS=.
        # shellcheck disable=SC2206 # split on the dots
        local -a a=($a_core) b=($b_core)
        for i in 0 1 2; do
            (( a[i] < b[i] )) && return 0
            (( a[i] > b[i] )) && return 1
        done
    fi
    [[ -n "$a_pre" && -z "$b_pre" ]] && return 0
    [[ -z "$a_pre" && -n "$b_pre" ]] && return 1
    [[ -n "$a_pre" && "$a_pre" < "$b_pre" ]]
}

if ! version_lt "$old" "$new"; then
    echo "the new version ${new} must be greater than the current version ${old}" >&2
    exit 1
fi

old_re="${old//./\\.}"

# the bump-version workflow offers the current version as the default for the next bump, checked
# before changing anything
workflow=.github/workflows/bump-version.yaml
workflow_default="default: \"${old}\" # the current version, kept current by build/scripts/bump-version.sh"
if ! grep -qF "$workflow_default" "$workflow"; then
    echo "unable to find the current version as the default in ${workflow}, expected: ${workflow_default}" >&2
    exit 1
fi

# the version in the [workspace.package] section, inherited by the crates
workspace_version=$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version = "\(.*\)"$/\1/p' Cargo.toml 2> /dev/null || true)
# the workspace's requirements on its own crates that name a version, e.g. a cli requiring the
# library at exactly the current version, publishing fails without a version requirement
crate_re='^[a-z0-9_-]+ = \{.*\bpath = "\./crates/[^"]*".*\bversion = "'
if [[ -n "$workspace_version" ]]; then
    if [[ "$workspace_version" != "$old" ]]; then
        echo "the workspace version ${workspace_version} in Cargo.toml does not match the interface version ${old}" >&2
        exit 1
    fi
    # checked before changing anything
    requirements=$(grep -E "$crate_re" Cargo.toml | grep -vE "\bversion = \"=?${old_re}\"" || true)
    if [[ -n "$requirements" ]]; then
        echo "unable to bump requirements in Cargo.toml that are not on the current version ${old}:" >&2
        echo "$requirements" >&2
        exit 1
    fi
fi

# references to the package or one of its interfaces, an interface named for a keyword is escaped
# with `%`, e.g. `componentized:constants/%u8@0.2.1-dev`
ref_re="${PACKAGE}(/%?[a-z0-9-]+)?"
# the fetched wit dependencies and the wkg.lock files are left to `make wit`, wkg replaces the
# dependencies and updates the locks for the new version. The files synced from componentized/build
# are left alone, an example in them may name the package.
synced=()
if [[ -f build/MANIFEST ]]; then
    while read -r _ path; do
        synced+=(":(exclude,literal)${path}")
    done < build/MANIFEST
fi
files=$(git grep --untracked -l -E "${ref_re}@${old_re}" -- ':(exclude,glob)**/wit/deps/**' ':(exclude,glob)**/wkg.lock' ${synced[@]+"${synced[@]}"} || true)
for file in $files; do
    sed -i.bak -E "s#(${ref_re})@${old_re}#\1@${new}#g" "$file"
    rm "$file.bak"
    echo "updated ${file}"
done

if [[ "$old" == *-* ]]; then
    files=$(git grep --untracked -l -F "@since(version = ${old})" -- 'wit/*.wit' || true)
    for file in $files; do
        sed -i.bak "s/@since(version = ${old_re})/@since(version = ${new})/g" "$file"
        rm "$file.bak"
        echo "updated @since in ${file}"
    done
fi

if [[ -n "$workspace_version" ]]; then
    perl -pi -e 'if (/^\[workspace\.package\]/ .. /^\[(?!workspace\.package\])/) { s/^version = "[^"]*"/version = "'"${new}"'"/ }' Cargo.toml
    echo "updated the workspace version in Cargo.toml"
    for crate in $(grep -E "$crate_re" Cargo.toml | sed 's/ .*//'); do
        perl -pi -e 's/^(\Q'"${crate}"'\E = \{.*\bversion = "=?)[^"]*(")/${1}'"${new}"'${2}/' Cargo.toml
        echo "updated the ${crate} requirement in Cargo.toml"
    done
fi

# the tools manifests' pins of the workspace's own crates, e.g. componentized/build pins its cli in
# build/tools/Cargo.toml for the repositories it's synced to, while it builds the cli itself with
# LOCAL_TOOLS. Moved only by a release, a prerelease isn't published to crates.io, so the pin stays
# on the last release until the next one, published before a repository syncs it. A manifest synced
# from componentized/build is left alone.
if [[ -n "$workspace_version" && "$new" != *-* ]]; then
    crates=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[].name')
    for manifest in build/tools/Cargo.toml tools/Cargo.toml; do
        [[ -f "$manifest" ]] || continue
        if [[ -f build/MANIFEST ]] && awk -v path="$manifest" '$2 == path { found = 1 } END { exit !found }' build/MANIFEST; then
            continue
        fi
        for crate in $crates; do
            grep -qE "^${crate} = \"=[^\"]*\"" "$manifest" || continue
            perl -pi -e 's/^(\Q'"${crate}"'\E = "=)[^"]*(")/${1}'"${new}"'${2}/' "$manifest"
            echo "updated the ${crate} pin in ${manifest}"
        done
    done
fi

perl -pi -e 's{^(\s+)\Q'"${workflow_default}"'\E$}{${1}'"${workflow_default/\"${old}\"/\"${new}\"}"'}' "$workflow"
echo "updated the default version in ${workflow}"

# regenerate the wit dependencies for the new version
make generate components test

echo "bumped ${PACKAGE} from ${old} to ${new}"
