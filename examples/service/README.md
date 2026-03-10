# Service Example

This example demonstrates a `Service` that owns a custom config category, provides a host capability, and has lifecycle hooks.

## Structure

- `wit/greeter.wit`: WIT interface implemented by the host capability
- `host/src/main.rs`: Service, ConfigHandler, and HostCapability implementation
- `greeter/`: Wasm component that imports the host interface
- `config.toml`: Component, capability, and service configuration

## Building and Running

Change into the `examples/service` directory.

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
[GreetingService] started
Result: "HOWDY, World!"
[GreetingService] shutdown
```

## How It Works

`GreetingService` implements `Service` with three responsibilities:

1. **Config handling**: A `ConfigHandler` claims the `[greeting]` category, parsing the `message` property. Config is shared with the service via `Arc<Mutex<...>>`.
2. **Capability provision**: After config is parsed, `capabilities()` uses `.take()` to pull the message out of the mutex and creates a `GreetingCapability` factory. The factory also reads `uppercase` from the capability's `config.*` sub-table.
3. **Lifecycle**: `start()` and `shutdown()` are called around invocation.

## Flow

1. **Configuration**: `[greeting.default]` sets `message = "Howdy"`. `[capability.greeting]` sets `config.uppercase = true`.
2. **Config phase**: `GreetingConfigHandler` parses the greeting category and stores the message.
3. **Capability phase**: `GreetingService.capabilities()` takes the stored message, creates a factory that applies the `uppercase` transform.
4. **Invocation**: Guest calls `get_greeting()`, host returns "HOWDY", guest formats "HOWDY, World!".
