# Hello World Example with Host Capability

This example demonstrates a custom host capability being used by a guest component.

## Structure

- `wit/host-greeting.wit`: WIT interface implemented by host capability
- `host/src/main.rs`: Host capability implementation and runtime
- `greeter/`: Wasm component that imports the host interface
- `config.toml`: Component and capability configuration

## Building and Running

Change into the `examples/host-capability` directory.

1. Build the greeter component and host:
```bash
./build.sh
```

2. Run the example:
```bash
./run.sh
```

Output:
```
Result: "Hello, World!"
```

## Flow

1. **Configuration**: Defines `greeting` as a host capability and `greeter` as a component that expects it
2. **Host**: `GreetingCapability` implements `HostCapability` and registers the `host-greeting` interface
3. **Invocation**: Guest calls `get_greeting()`, host returns "Hello", and guest formats "Hello, World!"
