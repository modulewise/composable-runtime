# Composable HTTP Gateway

**Expose Wasm Components and messaging channels as HTTP endpoints.**

The HTTP gateway maps incoming HTTP requests to component function invocations or to messages published to a channel based on TOML route configuration. It runs as a `RuntimeService`, starting automatically when `[gateway.*]` definitions with `type = "http"` are present.

---

## Configuration

Gateway routes are defined under `[gateway.<name>]` in TOML config files.

### Component routes

Invoke a component function, mapping path parameters and an optional request body to function arguments:

```toml
[gateway.api]
type = "http"
port = 8080

[gateway.api.route.get-user]
path = "/users/{id}"
component = "user-service"
function = "get-user"

[gateway.api.route.create-user]
path = "/users"
component = "user-service"
function = "create"
body = "user"
```

### Channel routes

Publish the raw request body to a messaging channel:

```toml
[gateway.api.route.events]
path = "/events"
channel = "incoming-events"
```

### Route properties

| Property | Required | Description |
|---|---|---|
| `path` | yes | URL path with optional `{param}` segments |
| `component` | yes* | Component to invoke |
| `function` | yes* | Function name on the component |
| `body` | no | Names a function parameter to populate from the request body |
| `channel` | yes* | Channel to publish to (mutually exclusive with `component`/`function`) |
| `method` | no | HTTP method (see defaults below) |

\* Either `component` + `function` or `channel` is required, never both.

### Method defaults

If `method` is not specified, it is inferred:

- Component routes with `body` default to **POST**
- Component routes without `body` default to **GET**
- Channel routes default to **POST** (the only valid option)

### Content types

Component routes currently support two content types for the request body:

- `application/json` (default): parsed as JSON
- `text/plain`: wrapped as a JSON string value

Other content types are rejected with `415 Unsupported Media Type`.

Channel routes forward the raw request body and content-type header to the channel. The activator handles content-type interpretation when the message is consumed.

All component route responses are JSON-serialized with `content-type: application/json`.

### Validation

The config handler rejects:

- Duplicate routes (same method and path structure)
- `body` param name that collides with a path param name
- `body` with method GET
- `body` on channel routes (since it's the payload, not a named param)
- GET with channel routes

---

## Standalone binary

```sh
composable-http-gateway config.toml [additional-configs...]
```

Multiple config files are merged, allowing separation of concerns (e.g. domain components, infrastructure capabilities, gateway routes in separate files). The default log level is `info`, overridable via the `RUST_LOG` environment variable.

---

## Library usage

Register the gateway service with a `RuntimeBuilder`:

```rust
use composable_runtime::Runtime;
use composable_gateway_http::HttpGatewayService;

let runtime = Runtime::builder()
    .from_paths(&config_paths)
    .with_service::<HttpGatewayService>()
    .build()
    .await?;

runtime.run().await
```

The service claims `[gateway.*]` definitions where `type = "http"`, so other gateway types can coexist under the same `[gateway.*]` category using different type selectors.