#!/usr/bin/env bash

# Managed by componentized/build and synced into each repository by `make update-build`. Change it
# in https://github.com/componentized/build, `make check-build` fails when a copy is changed.

# Encode each WIT package of a wit/deps directory where `wac compose --deps-dir` looks for it, so a
# composition can name the interfaces of a package it does not instantiate, e.g.
# `import stderr: wasi:cli/stderr@0.3.0;`.
#
#   WASM_TOOLS=<wasm-tools> scripts/wac-deps.sh <wit/deps> <deps-dir>
#
# wac looks for a versioned package at `<namespace>/<name>/<version>.wasm`, e.g.
#
#   wit/deps/wasi-cli-0.3.0  -> <deps-dir>/wasi/cli/0.3.0.wasm
#
# Each package is encoded with the other packages as its dependencies.

set -euo pipefail

deps=$1
out=$2
wasm_tools=${WASM_TOOLS:-wasm-tools}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

rm -rf "$out"
mkdir -p "$out"

# a missing directory, e.g. one without a wkg.toml to fetch it, has no packages
for package in "$deps"/*/; do
  [[ -d "$package" ]] || continue
  package=${package%/}
  rm -rf "$work/package"
  mkdir -p "$work/package/deps"
  cp "$package"/*.wit "$work/package/"
  for dep in "$deps"/*/; do
    dep=${dep%/}
    [[ "$dep" == "$package" ]] || cp -R "$dep" "$work/package/deps/"
  done
  "$wasm_tools" component wit "$work/package" --wasm -o "$work/package.wasm"

  # e.g. `package wasi:cli@0.3.0;`
  id=$("$wasm_tools" component wit "$work/package.wasm" | sed -n 's/^package \(.*\);$/\1/p' | head -n 1)
  namespace=${id%%:*}
  name=${id#*:}
  name=${name%%@*}
  version=${id#*@}
  mkdir -p "$out/$namespace/$name"
  cp "$work/package.wasm" "$out/$namespace/$name/$version.wasm"
done
