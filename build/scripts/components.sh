#!/usr/bin/env bash

# Managed by componentized/build and synced into each repository by `make update-build`. Change it
# in https://github.com/componentized/build, `make check-build` fails when a copy is changed.

# Print the components of the repository, each a line of its name and its directory, e.g.
# `example-hello examples/hello`, read by build/build.mk.
#
#   build/scripts/components.sh
#
# Components are found in directories of components: components/, and each directory with a
# components.toml, e.g. examples/components.toml. The file names the prefix of the components in the
# directory, components/ has none unless it has a components.toml too, e.g.
#
#   prefix = "example-"
#
# Each directory in a directory of components is a component named for the prefix and the directory,
# e.g. examples/hello is example-hello, when it has the files of one, named for the directory:
# wit/<dir>.constants.wit, <dir>.properties, <dir>.wac, <dir>.plug, <dir>.wkg or Cargo.toml. A
# component's directory may hold a group of components, those of its directories that are
# components, named for it, e.g. components/gate/gate.wac is gate, the composition of
# components/gate/internal, gate-internal, and components/gate/config, gate-config.

set -euo pipefail

cd "$(dirname "$0")/../.."

# the prefix a components.toml names, e.g. `prefix = "example-"`
prefix_of() {
    [[ -f "$1/components.toml" ]] && sed -n 's/^prefix *= *"\(.*\)" *$/\1/p' "$1/components.toml" | head -n 1
    return 0
}

# whether a directory has the files of a component, named for the directory
is_component() {
    local base
    base=$(basename "$1")
    [[ -f "$1/wit/${base}.constants.wit" || -f "$1/${base}.properties" || -f "$1/${base}.wac" \
        || -f "$1/${base}.plug" || -f "$1/${base}.wkg" || -f "$1/Cargo.toml" ]]
}

# the components of a directory of components with a prefix, and those of each group, a component
# with components of its own. The components of a directory with a components.toml are named for its
# prefix instead, found as a directory of its own.
visit() {
    local dir="$1" prefix="$2" sub name
    for sub in "$dir"/*/; do
        sub="${sub%/}"
        [[ -d "$sub" ]] && is_component "$sub" || continue
        name="${prefix}$(basename "$sub")"
        printf '%s %s\n' "$name" "$sub"
        [[ -f "$sub/components.toml" ]] || visit "$sub" "${name}-"
    done
}

{
    [[ -d components ]] && echo components
    find . -name components.toml -not -path '*/target/*' -not -path '*/wit/deps/*' -not -path './.git/*' \
        | sed 's|^\./||; s|^components.toml$|.|; s|/components.toml$||'
} | sort -u | while read -r dir; do
    visit "$dir" "$(prefix_of "$dir")"
done
