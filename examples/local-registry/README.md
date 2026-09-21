# Local Registry

Pulls a component over `oci://` from a `registry:2` running locally on HTTP.

Registries are HTTPS by default, so this one is declared in `registries.toml`:

```toml
[registry."localhost:5001"]
type = "oci"

[registry."localhost:5001".oci]
protocol = "http"
```

The `COMPOSABLE_OCI_REGISTRY_CONFIG` env var points to that config. If not set,
the runtime reads `wasm-pkg`'s global configuration (the same file `wkg` uses).

## Running

```bash
./start-registry.sh
./push-component.sh
./run.sh
```

The pushed component is from the `hello-world` example, so build that first if
`../hello-world/lib/greeter.wasm` is missing.

## Listing Registry Contents

```bash
./list.sh
```

Lists each repository, its tags, and each tag's layers:

```
example/greeter:0.1.0
    application/wasm  24131 bytes  sha256:997e5e2c70...
```

A tag points to a manifest, which lists the layers holding the content. The
digest above is the layer's, whereas `wkg oci push` reports the manifest
digest. The runtime's cache is keyed on the layer digest.

## Cleanup

```bash
./stop-registry.sh
```

Drops the registry's storage volume, discarding pushed components.

Components already pulled remain in `~/.cache/composable`, so `./run.sh`
still resolves them with the registry stopped. Delete that directory to
force a pull.

## Logging

An insecure pull logs a `warn!`, visible with `RUST_LOG=warn`.
