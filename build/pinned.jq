# Managed by componentized/build and synced into each repository by `make update-build`. Change it
# in https://github.com/componentized/build, `make check-build` fails when a copy is changed.

# generates target/tools/pinned.mk from `cargo metadata --no-deps` of build/tools/Cargo.toml, the
# tools the build provides, followed by that of the repository's tools/Cargo.toml when it has one,
# slurped, see build/tools.mk. A tool pinned by the repository overrides the build's.
def pinned($manifest):
  .packages[0] as $tools
  | $tools.dependencies[]
  | {name, version: (.req | ltrimstr("=")),
     bins: ($tools.metadata.bins[.name] // error("\(.name) is missing from [package.metadata.bins] in \($manifest)"))};
([.[0] | pinned("build/tools/Cargo.toml")] + [.[1] // empty | pinned("tools/Cargo.toml")])
| reduce .[] as $tool ({}; .[$tool.name] = $tool)
| [.[]] | sort_by(.name)
| "TOOLS := \(map(.name) | join(" "))",
  "TOOLS_PINNED := \(map("\(.name)@\(.version)") | join(" "))",
  (.[].bins[] | "\(ascii_upcase | gsub("-"; "_")) := $(TOOLS_BIN)/\(.)")
