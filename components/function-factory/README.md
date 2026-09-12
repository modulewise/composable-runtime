# function-factory

Build a JSON-based component that targets a function on any other component.

The generated component exports the `composable:runtime/function` interface
which defines the following:

```
metadata: async func() -> result<function-metadata, string>;

call: async func(input: json) -> result<json, json>;
```

The `json` type is an alias for `string`, and the `function-metadata` type is a
record containing name, description, and JSON schemas:

```
record function-metadata {
    name: string,
    description: option<string>,
    input-schema: json,
    output-schema: json,
}
```

The `input-schema` declares a JSON object whose fields match the target's param
names, and the `output-schema` declares the JSON value returned by the target.

The generated component acts as a translator between JSON and WIT for the param
and return types. Those can include composite types such as records, lists, and
variants. The generation is driven by a traversal of the WIT definitions. At
runtime the generated component relies on a `json-mapper` component whose
implementation is in the `composable-factory` repository.

## The `function-factory` World

- exports `composable:factory/factory@0.3.0` which provides the `build()` function
- imports `composable:factory/loader@0.3.0` to read the target's bytes and extract WIT
- imports `wasi:config/store@0.2.0-rc.1` for its own configuration

## Configuration

| Key | |
| --- | --- |
| `target` | Required. Path for the loader to read the target component bytes. |
| `function` | Which function of the target component to call. May be omitted if exactly one. If ambiguous, should be qualified as `interface.function`. |
| `description` | Optional. Sets the description returned in the metadata. |

## Use within Composable Runtime

Reference a `function-factory` component with a `factory:` URI so it provides
bytes for a component definition:

```toml
[component.greeter-function]
uri = "factory:greeter-factory"
imports = ["target", "mapper"]

[component.greeter-factory]
uri = "oci://ghcr.io/modulewise/component/function-factory:0.3.0"
imports = ["loader"]
config.target = "/lib/greeter.wasm"
config.function = "greeter.greet"

[component.target]
uri = "./lib/greeter.wasm"

[component.mapper]
uri = "oci://ghcr.io/modulewise/component/json-mapper:0.3.0"

[component.loader]
uri = "oci://ghcr.io/modulewise/component/filesystem-loader:0.3.0"
imports = ["filesystem"]

[capability.filesystem]
type = "wasi:filesystem"

[[capability.filesystem.preopens]]
host = "./lib"
guest = "/lib"
perms = "read-only"
```

The `target` config path is the loader's view of the target, so it resolves
against the preopened guest path with the permissions configured for that path.
