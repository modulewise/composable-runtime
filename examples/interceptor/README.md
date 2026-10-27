# Interceptor Example

This example demonstrates an interceptor that wraps a component with cross-cutting logging advice.

## Structure

- `wit/package.wit`: WIT interfaces for the greeter and logger worlds
- `greeter/`: Wasm component that exports a `greet` function
- `logger/`: Wasm component that exports `modulewise:interceptor/advice` and implements generic logging before and after each invocation
- `config.toml`: Component and capability configuration, including the WASI logging adapter that writes to stderr, which the runtime pulls from an OCI registry

## Building and Running

Change into the `examples/interceptor` directory.

1. Build the components:
```bash
./build.sh
```

2. Run the example:
```bash
./run.sh
```

Output:
```
... INFO  [interceptor]: Before greet(name: "World")
... INFO  [interceptor]: After greet -> "Hello, World!"
"Hello, World!"
```

## Configuration

```toml
[component.greeter]
uri = "./lib/greeter.wasm"
interceptors = ["logging-advice"]

[component.logging-advice]
uri = "./lib/logger.wasm"
imports = ["stderr-logger"]

[component.stderr-logger]
uri = "oci://ghcr.io/componentized/logging/to-stderr:0.3.1"
imports = ["cli", "clocks"]

[capability.cli]
type = "wasi:cli"
inherit-stderr = true

[capability.clocks]
type = "wasi:clocks"
```

## How It Works

The `interceptors` property on the greeter refers to generic advice, so the runtime generates an interceptor component at startup. The generated interceptor composes the logging-advice component with the greeter component, so that the advice is applied to each exported function of the greeter.
