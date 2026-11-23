#!/usr/bin/env bash

# Managed by componentized/build and synced into each repository by `make update-build`. Change it
# in https://github.com/componentized/build, `make check-build` fails when a copy is changed.

# Print the WIT paths each cargo component's bindings are generated from, as make variables, e.g.
# `WIT_BINDGEN_PATHS_greeter := components/greeter/../wit`, for target/wit-paths.mk, see
# build/components.mk.
#
#   build/scripts/wit-paths.sh <component>...
#
# A component's paths are the `path` of each `wit_bindgen::generate!` in its sources and in those of
# the workspace crates it depends on by path, transitively, but for tests, e.g. a crate of bindings
# the component and others share. A path is relative to the manifest of the crate with the
# `generate!`, like wit-bindgen reads it, e.g.
#
#   generate!({ path: "../wit", world: "greeter" })      ../wit
#   generate!({ path: ["wit", "../wit"], world: ... })   wit and ../wit
#   generate!("greeter" in "../wit")                     ../wit
#   generate!("greeter") or generate!({ world: ... })    wit, wit-bindgen's default
#   generate!({ inline: "...", world: ... })             nothing, the WIT is inline
#
# A component without a `generate!`, or that isn't a package of the workspace, has no paths.

set -euo pipefail

cd "$(dirname "$0")/../.."

[[ $# == 0 ]] && exit 0

# for each component, a line of its name, then the manifest directory and the source of each target of
# it and of the crates it depends on, tab separated, relative to the repository, e.g.
# `greeter  components/greeter  components/greeter/src`, a target in the crate's own directory, e.g.
# build.rs, is a file, others are their directory, e.g. src
cargo metadata --no-deps --format-version 1 | jq -r --args '
    .workspace_root as $root
    | (.packages | map({key: .name, value: .}) | from_entries) as $packages
    | def relative: if . == $root then "." else ltrimstr($root + "/") end;
      def dir: sub("/[^/]*$"; "");
      # the workspace packages a package depends on by path, but for tests, cargo forbids cycles
      def path_deps: .dependencies[] | select(.path != null and .kind != "dev") | $packages[.name] | select(. != null);
      def sources:
        (.manifest_path | dir) as $dir
        | .targets[] | select(any(.kind[]; IN("lib", "rlib", "cdylib", "staticlib", "dylib", "bin", "proc-macro", "custom-build")))
        | .src_path | if dir == $dir then . else dir end
        | [($dir | relative), relative] | join("\t");
      $ARGS.positional[] as $name
      | $packages[$name] | select(. != null)
      | [$name, ([recurse(path_deps)] | unique_by(.name) | .[] | sources)] | join("\t")
' "$@" | perl -ne '
    chomp;
    my ($name, @fields) = split /\t/;
    my %paths;
    while (my ($crate, $source) = splice(@fields, 0, 2)) {
        my @files = -d $source ? split /\0/, `find "$source" -name "*.rs" -not -path "*/target/*" -print0` : ($source);
        for my $file (@files) {
            open(my $fh, "<", $file) or next;
            local $/;
            my $text = <$fh>;
            # the comments, which may mention a path or a macro
            $text =~ s{//[^\n]*}{}g;
            $text =~ s{/\*.*?\*/}{}gs;
            # the arguments of each generate!, the parentheses balanced
            while ($text =~ /\bgenerate!\s*(\((?:[^()]++|(?1))*\))/g) {
                my $args = $1;
                my @found;
                if ($args =~ /\bpath\s*:\s*(\[[^\]]*\]|"[^"]*")/) {
                    @found = $1 =~ /"([^"]*)"/g;
                } elsif ($args =~ /^\(\s*"[^"]*"\s+in\s+"([^"]*)"/) {
                    @found = ($1);
                } elsif ($args !~ /\binline\s*:/) {
                    @found = ("wit");
                }
                $paths{$crate eq "." ? $_ : "$crate/$_"} = 1 for @found;
            }
        }
    }
    print "WIT_BINDGEN_PATHS_$name := ", join(" ", sort keys %paths), "\n";
'
