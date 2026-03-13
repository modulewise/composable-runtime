# Interceptor Example

This example demonstrates an interceptor that wraps a component with cross-cutting logging advice.

## Structure

- `wit/package.wit`: WIT interfaces for the greeter and logger worlds
- `greeter/`: Wasm component that exports a `greet` function
- `logger/`: Wasm component that exports `modulewise:interceptor/advice` and implements generic logging before and after each invocation
- `config.toml`: Component and capability configuration
- `wasi-logging-to-stdout.wasm`: WASI logging adapter (fetched from an OCI registry on first build)

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
uri = "./target/wasm32-unknown-unknown/release/greeter.wasm"
interceptors = ["logging-advice"]

[component.logging-advice]
uri = "./target/wasm32-unknown-unknown/release/logger.wasm"
imports = ["stdout-logger"]

[component.stdout-logger]
uri = "./wasi-logging-to-stdout.wasm"
imports = ["stdio", "wasip2"]

[capability.stdio]
uri = "wasmtime:inherit-stdio"

[capability.wasip2]
uri = "wasmtime:wasip2"
```

## How It Works

The `interceptors` property on the greeter refers to generic advice, so the runtime generates an interceptor component at startup. The generated interceptor composes the logging-advice component with the greeter component, so that the advice is applied to each exported function of the greeter.
