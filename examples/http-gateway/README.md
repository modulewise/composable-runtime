# HTTP Gateway Example

This example demonstrates the HTTP gateway with two types of routes:

1. a component route that invokes a function directly
2. a channel route that publishes a message for a subscribed component

Because the HTTP Gateway applies inversion of control, neither component has any built-in HTTP request handling. In fact, both routes target the simple [hello-world](../hello-world/) greeter component.

## Structure

- `config.toml`: gateway, component, and route configuration
- `run-gateway.sh`: starts the Composable Runtime with HTTP Gateway as a service
- `greet.sh`: sends requests to both routes via `curl`

## Building and Running

Change into the `examples/http-gateway` directory.

1. Start the gateway:
```bash
./run-gateway.sh
```

That will build the [hello-world](../hello-world/) example's greeter component if not already built.

Then it starts a long-running Composable Runtime instance with the [HTTP Gateway](../../crates/gateway-http/) enabled, and configures info-level logging.

3. In another terminal, send requests:
```bash
./greet.sh
```

Output:
```
--- GET /hello/world (component route) ---
"Hello, World!"
--- POST /bonjour (channel route -> subscription -> bonjour component) ---
```

In the gateway log, you will see the channel route result logged asynchronously:

```
... invocation complete component=bonjour function=greet result="Bonjour, le Monde!"
```

## How It Works

1. **Component route**: `GET /hello/{name}` maps the `{name}` path segment to the `name` argument of `hello.greet(name)` and returns the result directly.
2. **Channel route**: `POST /bonjour` publishes the request body to the "names" channel. The `bonjour` component subscribes to that channel and is invoked asynchronously.
3. **Configuration**: The `bonjour` component definition includes `config.greeting = "Bonjour"` to differentiate its greeting in the log output.

## Configuration

```toml
[gateway.api]
type = "http"
port = 8888

[gateway.api.route.hello]
path = "/hello/{name}"
component = "hello"
function = "greet"

[gateway.api.route.bonjour]
path = "/bonjour"
channel = "names"

[component.hello]
uri = "../hello-world/greeter/target/wasm32-unknown-unknown/release/greeter.wasm"

[component.bonjour]
uri = "../hello-world/greeter/target/wasm32-unknown-unknown/release/greeter.wasm"
config.greeting = "Bonjour"
subscription = "names"
```
