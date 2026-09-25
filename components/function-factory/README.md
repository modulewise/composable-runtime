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

- exports `composable:factory/factory@0.4.0` which provides the `build()` function
- imports `wasi:config/store@0.2.0-rc.1` for its own configuration

## Configuration

| Key | |
| --- | --- |
| `wit` | Required. The target's WIT. |
| `world` | Optional. The world in `wit` which contains the exported function to be adapted. Defaults to `root`. |
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
uri = "oci://ghcr.io/modulewise/component/function-factory:0.4.0"
config.wit = "${wit(target)}"
config.function = "greeter.greet"

[component.target]
uri = "./lib/greeter.wasm"

[component.mapper]
uri = "oci://ghcr.io/modulewise/component/json-mapper:0.4.0"
```

The `${wit(target)}` expression in a definition extracts the WIT of the named `target` component.

## Use outside Composable Runtime

The `function-adapter` CLI builds the same component directly from a WIT file:

```bash
wasm-tools component wit greeter.wasm > greeter.wit
function-adapter greeter.wit -o greeter-function.wasm --function greeter.greet
```

To run the function-factory component as a standalone component, provide a
`wasi:config/store` import, which can be created with `static-config`:

```bash
static-config -p "wit=$(wasm-tools component wit greeter.wasm)" \
  -p "function=greeter.greet" -o config.wasm
```
