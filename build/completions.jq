# Managed by componentized/build and synced into each repository by `make update-build`. Change it
# in https://github.com/componentized/build, `make check-build` fails when a copy is changed.

# generates target/tools/completions from `cargo metadata --no-deps` of build/tools/Cargo.toml,
# followed by that of the repository's tools/Cargo.toml when it has one, slurped, see build/tools.mk.
# Each line is a binary and the arguments that print its completions, quoted for the shell, e.g.
# `'wasm-tools' 'completion'`, read by build/scripts/completions.sh. A binary the repository lists
# overrides the build's, one listed with no arguments has none.
def completions($manifest):
  (.packages[0].metadata.completions // {})
  | to_entries[]
  | if (.value | type) == "array" and all(.value[]; type == "string") then .
    else error("\(.key) in [package.metadata.completions] in \($manifest) is not a list of arguments") end;
([.[0] | completions("build/tools/Cargo.toml")] + [.[1] // empty | completions("tools/Cargo.toml")])
| reduce .[] as $binary ({}; .[$binary.key] = $binary.value)
| to_entries | sort_by(.key)[]
| select(.value != [])
| [.key] + .value | @sh
