# Filesystem Example

A component that reads and writes files through `wasi:filesystem`, showing how
the runtime configuration grants filesystem access with explicit permissions.

## Structure

```
filesystem/
├── file-store/            # Wasm Component
│   └── src/lib.rs         # Implements read, write, and list-files
├── wit/                   # WIT interface definitions
│   └── file-store.wit     # Imports wasi:filesystem, exports the three functions
├── data/                  # The directory made available to the component
├── config-readwrite.toml  # Grants read-write access
└── config-readonly.toml   # Grants read-only access
```

## Build

```sh
./build.sh
```

## Run

```sh
./run.sh
```

## Configuration

A component may only use the directories a capability preopens for it, and only
with the permissions each preopen grants:

```toml
[component.file-store]
uri = "./lib/file-store.wasm"
imports = ["filesystem"]

[capability.filesystem]
type = "wasi:filesystem"

[[capability.filesystem.preopens]]
host = "./data"       # directory on the host
guest = "/data"       # where the component sees it
perms = "read-write"  # or "read-only"
```

The `perms` configuration is **required** (no default). It covers file *and*
directory permissions for the preopen ("read-write" grants directory mutation).

The `host` resolves relative to the working directory where the runtime starts.

## Sample Commands

Reading works under either config:

```
$ composable invoke config-readonly.toml -- file-store.read from-host.txt
"hello from the host\n"
```

Writing works only when the preopen grants `read-write`:

```
$ composable invoke config-readwrite.toml -- file-store.write from-guest.txt "hello from the guest"
20
```

Under `config-readonly.toml`, the same call is denied:

```
$ composable invoke config-readonly.toml -- file-store.write denied.txt "nope"
Error: Component returned error: "cannot open 'denied.txt': ErrorCode::NotPermitted"
```

A component cannot escape its preopen with `..` or an absolute path. Both are denied.
