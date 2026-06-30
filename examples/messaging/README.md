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
Hello, Alice!
```

You can also run `composable publish` directly (`reply-timeout` ensures a reply Message is returned):
```bash
composable publish config.toml --channel names --body Bob --reply-timeout 1
```

Output:
```
Hello, Bob!
```

## How It Works

1. **Subscription**: A `[subscription.<name>]` block declares that a component should be subscribed to a channel. The `channel` field defaults to the subscription's name. When a message arrives on the channel, the runtime invokes the component's function with the message body as the argument. Optional fields: `function` (required when the component exports more than one), and the four mapping blocks - `param-mapping`, `param-encoding`, `result-decoding`, `result-mapping` - which apply in pipeline order to bridge the Message and the WIT call. See the [mapping module docs](../../src/mapping.rs) for details.
2. **Publishing**: `composable publish` starts the runtime, publishes a single message to the named channel, waits for processing, and exits.

## Configuration

```toml
[component.greeter]
uri = "../hello-world/lib/greeter.wasm"

[subscription.names]
component = "greeter"
```
