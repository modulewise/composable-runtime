# shellcheck shell=bash
# Managed by componentized/build and synced into each repository by `make update-build`. Change it
# in https://github.com/componentized/build, `make check-build` fails when a copy is changed.

# Registers the shell completions of the installed tools, sourced by bash or zsh at startup once the
# tools are on the PATH, e.g. in ~/.bashrc:
#
#   PATH="$(make -s tools-path):${PATH}"
#   source /path/to/repository/build/scripts/completions.sh
#
# The tools that have completions, and how each prints them, are listed in
# [package.metadata.completions] of build/tools/Cargo.toml and the repository's tools/Cargo.toml,
# generated into target/tools/completions by `make tools`. Generated as each shell starts, so a tool
# installed or bumped since completes in new shells. A tool that isn't installed is skipped, as is
# one that fails to print its completions.

# the arguments are the ones each line of completions sets
# shellcheck disable=SC2120
__componentized_completions() {
    local script completions shell line
    if [ -n "${ZSH_VERSION:-}" ]; then
        # zsh's path of the sourced script, BASH_SOURCE in bash
        # shellcheck disable=SC2296
        script="${(%):-%x}"
        shell=zsh
    else
        script="${BASH_SOURCE[0]}"
        shell=bash
    fi
    completions="$(cd "$(dirname "$script")/../.." && pwd)/target/tools/completions"
    [ -f "$completions" ] || return 0
    while IFS= read -r line; do
        # the binary and its arguments, quoted by build/completions.jq
        eval "set -- $line"
        command -v "$1" > /dev/null 2>&1 || continue
        # only a script the tool printed, one that fails may print its usage to stdout, which isn't
        # run, and none reads the terminal, which would hang the shell
        line=$("$@" "$shell" < /dev/null 2> /dev/null) || continue
        [ -n "$line" ] && eval "$line"
    done < "$completions"
}
__componentized_completions
unset -f __componentized_completions
