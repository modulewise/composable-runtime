#!/usr/bin/env bash

# Managed by componentized/build and synced into each repository by `make update-build`. Change it
# in https://github.com/componentized/build, `make check-build` fails when a copy is changed.

# Print the WIT of a component in a canonical form, to compare the WIT of two components, e.g. the
# release and debug build of a component.
#
#   WASM_TOOLS=<wasm-tools> scripts/wit-canonical.sh <component.wasm>
#
# `wasm-tools component wit` prints the items of a component's WIT in the order the component
# defines them, which differs between builds of the same sources, e.g. a build optimized by
# wasm-opt. Each line is printed with the items it's nested in, e.g.
#
#   package wasi:http@0.3.0 > interface types > resource request > get-method: func() -> method;
#
# the names a `use` imports are sorted, and the lines are sorted.

set -euo pipefail

component=$1
wasm_tools=${WASM_TOOLS:-wasm-tools}

"$wasm_tools" component wit "$component" | perl -ne '
  s/^\s+|\s+$//g;
  next unless length;
  if ($_ eq "}") { pop @items; next }
  s/\{([^{}]*)\}/"{" . join(", ", sort split(m{,\s*}, $1)) . "}"/e;
  print join(" > ", @items, $_), "\n";
  push @items, $1 if /^(.*?)\s*\{$/;
' | LC_ALL=C sort
