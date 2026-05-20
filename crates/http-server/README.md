# Composable HTTP Server

**Expose Wasm Components and messaging channels as HTTP endpoints.**

The HTTP server maps incoming HTTP requests to component function invocations or to messages published to a channel based on TOML route configuration. It runs as a runtime `Service`, starting automatically when `[server.*]` definitions with `type = "http"` are present.

---

## Configuration

Server routes are defined under `[server.<name>]` in TOML config files.

Both route types (component and channel) follow the same model: the inbound request is converted into a `Message` (body + headers), path/query captures are merged into the body and/or headers, and dispatch proceeds either through direct component invocation or by publishing to the channel.

### Component routes

Invoke a component function. Path captures merge into the message, which `MessageMapper` then translates into WIT-typed args either matching by name (default) or via an explicit `param-mapping`:

```toml
[server.api]
type = "http"
port = 8080

[server.api.route.get-user]
method = "GET"
path = "/users/{id}"
component = "user-service"
function = "get-user"

[server.api.route.create-user]
method = "POST"
path = "/users"
component = "user-service"
function = "create"
```

### Channel routes

Publish the request to a messaging channel. The route does NOT carry mapping as that's the responsibility of the consumer subscription, decoupled by the channel:

```toml
[server.api.route.events]
method = "POST"
path = "/events"
channel = "incoming-events"
```

Optionally enable request-reply by setting `reply-timeout-ms`; the route then waits for a reply Message from the channel before responding.

### Captures merged into the request body

Path captures (e.g. `{id}` in the path) and declared query-param captures are merged into the top level of the Message body produced from the request.

- If the request has no body, the Message body is `{ <captures> }`.
- If the request has a JSON object body, captures merge into its top level. A collision with an existing field is rejected as 400.
- If the request has a non-object body (array, scalar) and captures are present, the request is rejected as 400.
- `text/plain` bodies are passed through unchanged (unless captures are present, in which case it is rejected as 400).

### Route properties

| Property | Required | Applies to | Description |
|---|---|---|---|
| `path` | yes | both | URL path with `{name}` named captures or `{}` anonymous segments |
| `method` | yes | both | HTTP method (must be declared, no default) |
| `content-type` | no | both | Inbound request body content-type: `application/json` (default for body-accepting methods) or `text/plain`. Must be absent for methods that do not accept a body (GET, HEAD, OPTIONS, TRACE) |
| `component` | yes\* | component | Component to invoke |
| `function` | yes\* | component | Function name on the component |
| `channel` | yes\* | channel | Channel to publish to |
| `param-mapping` | no | component | Per-arg templates that build WIT args from Message body/headers |
| `param-encoding` | no | component | Per-arg content-type specs that encode assembled args as bytes |
| `result-decoding` | no | component | Per-field content-type specs that decode WIT-result byte fields |
| `result-mapping` | no | component | Structural `body` / `headers` slots that shape the reply Message |
| `response-schema` | no | both | Explicit JSON Schema for the response body |
| `propagate-request-headers` | no | both | HTTP request headers to lift into inbound Message headers (with optional `as` rename) |
| `propagate-response-headers` | no | both | Reply Message headers to emit on the HTTP response (with optional `as` rename) |
| `query-params` | no | both | Query-parameter specifications (see grammar below) |
| `reply-timeout-ms` | no | channel | If set, use request-reply with this timeout (else fire-and-forget) |

\* Either `component` + `function` or `channel` is required, never both.

### Mapping pipeline

Component routes invoke a WIT function. Bridging between a Message
(JSON body + headers) and WIT (typed args, typed result) involves a Message
Mapper driven by four optional config blocks.

**Inbound** (Message -> WIT call):

1. `param-mapping`: per-arg templates that build WIT args by reading paths
   into the inbound Message (`{body.<path>}`, `{headers.<path>}`). Without an
   entry for a given WIT param, the param name is looked up as a top-level
   field on the Message body, and that field's value becomes the arg.
2. `param-encoding`: for any WIT arg typed as a byte array (`list<u8>`),
   the associated value is encoded based on a content-type, either provied as a
   literal value or via path-match against the body or headers.

**Outbound** (WIT result -> reply Message):

3. `result-decoding`: for any byte-array field on the WIT result, the value
   is decoded based on a content-type, either provided as a literal value or
   via path-match against the result. The decoded value replaces the bytes
   before `result-mapping` runs.
4. `result-mapping`: structural `body` / `headers` slots that shape the
   reply Message: `body` becomes the HTTP response body, and mapped headers
   are available for response header propagation (see below).

With no blocks declared, direct name-matching drives the inbound side and the
WIT result becomes the response body verbatim.

### Template path syntax

Paths use a uniform dotted grammar across every config that accepts a `{path}` template:

- `body.user.email`: dot-separated names for normal keys.
- `headers["foo.bar"]`: bracket-quoted-string segment for keys whose characters are outside `[A-Za-z0-9_-]`.
- `body.items[3].name`: bracket-integer for array indices.

When the source is a Message (inbound side), paths must start with `body` or `headers`. When the source is the WIT result (outbound side), paths walk directly into the result. When the source is the assembled WIT args (`param-encoding`'s content-type paths), the first segment names a WIT param and subsequent segments walk into it.

### `result-mapping` shape

The block has two structural sub-keys: `body` and `headers`. Each is independent.

- `body` absent (or `null`, or `""`) -> reply Message body is zero bytes.
- `body` set to a single string path -> that path is bulk-lifted as the body.
- `body` set to a sub-table -> cherry-pick: each sub-key is a body field, each value a source path.
- `headers` follows the same shape (cherry-picked entries become reply Message headers).

When `result-mapping` is absent entirely, the WIT result becomes the reply body verbatim and no headers are written (other than the auto-merged tracing context).

### `result-decoding` and `param-encoding`

Each entry's value is a content-type spec, in one of two forms:

- A literal content-type: `payload = "application/json"`.
- A path (in `{...}`) that resolves at runtime to a content-type string: `payload = "{headers.content-type}"`.

Supported content-types: `application/json` (decodes as JSON / encodes via JSON serialization) and `text/plain` (decodes as UTF-8 string / encodes from a string).

For `result-decoding`, path references resolve against the WIT result. For `param-encoding`, they resolve against the assembled WIT args (first segment is a param name).

Startup validation rejects entries referencing non-existent fields/params, fields/params whose WIT type isn't a byte array, paths that don't exist in the source schema, and unsupported literal content-types.

### `propagate-request-headers` and `propagate-response-headers`

Each is a list of entries. Each entry is either a source name or `"source as target"` (rename).

`propagate-request-headers` reads named HTTP request headers and writes them onto the inbound Message under the target name. Source-side names are HTTP header names. Target-side names are Message header names.

`propagate-response-headers` reads named reply Message headers and emits them on the HTTP response. Source-side names are Message header names. Target-side names are HTTP header names.

Validation rejects entries whose HTTP-side name isn't a valid HTTP token. Validation also rejects a `propagate-response-headers` entry whose target collides with a name that `result-mapping.headers` writes from a different source (which would silently override the mapped header).

### Query-param grammar

Each entry in `query-params` is a string with one of the following syntax options:

- `key`: required, any value, captured
- `key=value`: required, must equal `value`, captured
- `key?`: optional, captured if present
- `key?=value`: optional, captured if present, must equal `value` when present
- `~key`, `~key=value`, `~key?=value`: match but do NOT capture
- `!key`: forbidden (must be absent)

Required, value-mismatched, and forbidden-present cases all make the route NOT match; the router tries the next route, and a request that matches no route returns 404.

### Response schemas

For component routes, the response schema is derived automatically:

- If `result-mapping` is declared, the schema is derived from the body template, walking against the WIT result type (with any `result-decoding` swap applied to byte-array fields).
- Otherwise, the schema is the WIT return type itself, with `result-decoding` swaps applied. A `text/plain` decoded field becomes `{ "type": "string" }`; an `application/json` decoded field or a runtime-path-resolved decoded field becomes `{}` (any).
- If `response-schema` is also declared explicitly, it is structurally validated against the derived schema (and may add metadata like descriptions or validation constraints).
- If `result-mapping` declares no body (slot absent, `null`, or `""`), no response schema is advertised.

For channel routes, no derivation is possible (the consumer's WIT signature is not visible to the route). The explicit `response-schema`, if declared, is taken as-is.

In all cases, the reply Message body is coerced against the response schema as a tolerant-reader pass (e.g. a non-string value at a string-typed position is stringified) before serializing the HTTP response.

### Content types

Each route declares its inbound body content-type via `content-type`. Supported values:

- `application/json` (default for body-accepting methods): parsed as JSON, captures merge into the parsed object
- `text/plain`: passed through as raw text, no captures or JSON mapping/encoding config for request to params

Methods that do not accept a body (GET, HEAD, OPTIONS, TRACE) must not declare `content-type`. Any inbound body bytes and `Content-Type` header are ignored. The Message body is built from captures only (as a JSON object).

Otherwise, when a request's `Content-Type` header is present and does not match the route's declared type, the response is `415 Unsupported Media Type`.

Two routes sharing the same `(method, path, query-params)` may coexist if they declare different `content-type` values. Two routes sharing all four are treated as a config-time conflict.

On `text/plain` routes, the config-time rules also reject:

- `param-mapping` (the inbound body is a raw string, not a JSON shape to traverse)
- `param-encoding` (encoding to the WIT arg is handled automatically)
- Path captures and capturing query-params (no JSON object to merge into)

`result-mapping`, `result-decoding`, and `response-schema` are independent of inbound content-type and remain available.

### Validation

Config-time rejections:

- Duplicate routes (same method, path structure, content-type, with non-disjoint query-param specs)
- Duplicate path capture names within a single path
- A capturing query-param that shares a name with a path capture on the same route
- `param-mapping`, `param-encoding`, `result-decoding`, or `result-mapping` on a channel route (those apply to component routes only; for channel routes, the downstream subscription owns its mappings)
- `reply-timeout-ms` on a component route
- Both `component`/`function` and `channel` on the same route
- An unsupported `content-type` value
- `content-type` declared on a method that does not accept a body
- On `text/plain` routes: `param-mapping`, `param-encoding`, path captures, or capturing query-params
- Mismatch between an explicit `response-schema` and the derived schema
- `result-decoding` / `param-encoding` entries that reference non-existent WIT fields/params, fields/params whose type isn't a byte array, content-type paths that don't exist in the source schema, or unsupported literal content-types
- `propagate-request-headers` / `propagate-response-headers` entries whose HTTP-side names aren't valid RFC 9110 tokens
- A `propagate-response-headers` entry whose target collides with a `result-mapping.headers` target from a different source (which would silently override the mapped value)

Request-time validation:

- An inbound `Content-Type` header that does not match the route's declared `content-type` is rejected with `415 Unsupported Media Type`.
- For component routes, the assembled Message body is validated against the schema derived from `param-mapping` / `param-encoding`. Failure is `400 Bad Request` naming the offending field.
- The response body is validated against the route's effective response schema (derived from the WIT result and `result-mapping`, optionally enriched by an explicit `response-schema`). Failure is `500 Internal Server Error` (the component returned content that doesn't satisfy the advertised contract).

---

## Standalone binary

```sh
composable-http-server config.toml [additional-configs...]
```

Multiple config files are merged, allowing separation of concerns (e.g. domain components, infrastructure capabilities, server routes in separate files). The default log level is `info`, overridable via the `RUST_LOG` environment variable.

---

## Library usage

Register the HTTP service with a `RuntimeBuilder`:

```rust
use composable_runtime::Runtime;
use composable_http_server::HttpService;

let runtime = Runtime::builder()
    .from_paths(&config_paths)
    .with_service::<HttpService>()
    .build()
    .await?;

runtime.run().await
```

The service claims `[server.*]` definitions where `type = "http"`, so other server types can coexist under the same `[server.*]` category using different type selectors.
