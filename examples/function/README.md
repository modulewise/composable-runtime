# Function Example

A `[function]` definition that exposes a component's function as
`composable:runtime/function`, with JSON in and out.

## Configuration

```toml
[function.greet]
target = "greeter"
function = "greeter.greet"
description = "Greet someone by name"

[component.greeter]
uri = "oci://ghcr.io/modulewise/demo/hello:0.2.0"
```

The `[function]` handler generates the `greet` component with a
function-factory over the WIT of its `target`, and composes it with the
target and a shared JSON mapper. All three components are pulled from OCI, so
there is nothing to build.

| Property | |
| --- | --- |
| `target` | Required. The component whose function is called. |
| `function` | Which function of the target to call. May be omitted if exactly one. If ambiguous, should be qualified as `interface.function`. |
| `world` | Optional. The world in the target's WIT which contains the function. Defaults to `root`. |
| `description` | Optional. Sets the description returned in the metadata. |
| `labels` | Optional. Labels for the generated component. |

## Run

```bash
./run.sh [NAME]
```

Invokes `metadata` and then `call` with `{"name":"NAME"}`, where `NAME`
defaults to `World`.

Output:
```
metadata:
{
  "name": "greeter.greet",
  "description": "Greet someone by name",
  "input-schema": "{\"type\":\"object\",\"properties\":{\"name\":{\"type\":\"string\"}},\"required\":[\"name\"],\"additionalProperties\":false}",
  "output-schema": "{\"type\":\"string\"}"
}
call:
"Hello World!"
```
