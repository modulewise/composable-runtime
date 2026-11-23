# Managed by componentized/build and synced into each repository by `make update-build`. Change it
# in https://github.com/componentized/build, `make check-build` fails when a copy is changed.

# generates target/tools/local.mk from `cargo metadata --no-deps` of the workspace, for the packages
# named in $tools, LOCAL_TOOLS, see build/tools.mk
.workspace_root as $root
| (.packages | map({key: .name, value: .}) | from_entries) as $packages
| def relative: if . == $root then "." else ltrimstr($root + "/") end;
  def dir: sub("/[^/]*$"; "");
  # the workspace packages a package depends on by path, but for tests, cargo forbids cycles
  def path_deps: .dependencies[] | select(.path != null and .kind != "dev") | $packages[.name] | select(. != null);
  # the manifest and the sources of a package's targets, but its tests, benches and examples. A
  # target in the package's own directory, e.g. build.rs, is a file, others are their directory,
  # e.g. src, a root package's directory holds the whole repository.
  def sources:
    (.manifest_path | dir) as $dir
    | (.manifest_path | relative),
      (.targets[] | select(any(.kind[]; IN("lib", "rlib", "cdylib", "staticlib", "dylib", "bin", "proc-macro", "custom-build")))
        | .src_path | if dir == $dir then relative else dir | relative end);
  ($tools | split(" ") | map(select(. != "")))[] as $name
  | ($packages[$name] // error("\($name) in LOCAL_TOOLS is not a package of the workspace")) as $package
  | "LOCAL_TOOL_PATH_\($name) := \($package.manifest_path | dir | relative)",
    "LOCAL_TOOL_SOURCES_\($name) := \([$package | recurse(path_deps) | sources] | unique | join(" "))",
    ($package.targets[] | select(.kind | index("bin")) | .name | "\(ascii_upcase | gsub("-"; "_")) := $(TOOLS_BIN)/\(.)")
