# Messaging Example

This example demonstrates a component that subscribes to a channel and is invoked when messages are published.

## Structure

- `config.toml`: Component configuration with a `subscription` property
- `publish.sh`: Publishes a message via `composable publish`

This example reuses the greeter component from the [hello-world](../hello-world/) example.

## Running the Example

Change into the `examples/messaging` directory.

Publish a name via `publish.sh` (it will build the greeter component if not already built):
```bash
./publish.sh Alice
```

Output:
```
... invocation complete component=greeter function=greet result="Hello, Alice!"
```

If you have already run `cargo install --path .` in this repo's root, you can use `composable publish` directly:
```
composable publish config.toml --channel names --body Bob
```

You will notice that is much faster than the `cargo run` in `publish.sh`.

## How It Works

1. **Subscription**: The `subscription` property on a component tells the runtime to create a channel and subscribe the component to it. When a message arrives, the runtime invokes the component's exported function with the message body as the argument.
2. **Publishing**: `composable publish` starts the runtime, publishes a single message to the named channel, waits for processing, and exits.

> [!NOTE]
> At this time, the component must export exactly one function, but more configuration options will be available for subscriptions soon, including the target function.

## Configuration

```toml
[component.greeter]
uri = "../hello-world/greeter/target/wasm32-unknown-unknown/release/greeter.wasm"
subscription = "names"
```
