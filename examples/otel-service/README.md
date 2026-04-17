# OpenTelemetry Example using OtelService

This example demonstrates end-to-end OpenTelemetry trace propagation using the built-in `otel` capability (from the [composable-otel](../../crates/opentelemetry) sub-crate) and the built-in `http` server (from the [composable-http-server](../../crates/http-server) sub-crate).

An HTTP request arrives with (or without) a `traceparent` header. `HttpService` opens a server span and writes the resulting `traceparent` into the invocation's propagation context. The `otel` capability surfaces that traceparent to the guest through `outer-span-context` on the `wasi:otel/tracing` interface, and the guest emits a child span and log record. `HttpService` and `OtelService` each own their own OTLP exporter.

## Flow

```
   HTTP client
       |  (optional: traceparent header)
       v
   HttpService  --- (server span, OTLP exporter) -----------.
       |   opens server span                                |
       |   adds traceparent to invocation context           v
       v                                              OTLP collector
     guest (component)                                  (external)
           outer-span-context()  --> otel capability        ^
           child span + log      --> otel capability        |
                                           |                |
                                           v                |
                                      OtelService  ---------'
                                    (OTLP exporter)
```

- **HttpService:** Accepts requests on `/test`, extracts the inbound `traceparent` (if any), opens a server span, and writes the span's traceparent into the invocation's propagation context. Exports its server span via its own OTLP pipeline (configured by `otlp-endpoint` on `[server.http]`).
- **otel capability:** Reads the traceparent from the propagation context and returns it to the guest from `outer-span-context`. Forwards guest spans and log records emitted through `wasi:otel` to the `OtelService` exporter.
- **guest:** Calls `outer-span-context` to read the propagated trace id and parent span id, opens its own child span, emits a log record, and ends the span.
- **OtelService:** Owns an independent OTLP pipeline (configured by `[capability.otel]`). Exports guest spans and logs forwarded by the `otel` capability.

## Host Binary

The host registers the two services when building the runtime:

```rust
let runtime = Runtime::builder()
    .from_path(std::path::PathBuf::from("config.toml"))
    .with_service::<OtelService>()
    .with_service::<HttpService>()
    .build()
    .await?;

runtime.run().await
```

Each `Service` implementation registered via `with_service` may contribute config handlers, capability factories, and lifecycle hooks.

## Configuration

```toml
[capability.otel]
type = "otel"
endpoint = "http://localhost:4317"
protocol = "grpc"

[capability.otel.resource]
"service.name" = "otel-service-example"
"service.version" = "0.1.0"

[server.http]
type = "http"
port = 8080
otlp-endpoint = "http://localhost:4317"

[server.http.route.test]
path = "/test"
component = "guest"
function = "run"

[component.guest]
uri = "./target/wasm32-unknown-unknown/release/guest.wasm"
imports = ["otel", "clocks", "random"]

[capability.clocks]
type = "wasi:clocks"

[capability.random]
type = "wasi:random"
```

- `[capability.otel]`: configures the OTLP exporter (endpoint, protocol, resource attributes), and exports `wasi:otel/logs` and `wasi:otel/tracing` for guest components.
- `[server.http]`: runs the HTTP server on port 8080. `otlp-endpoint` configures `HttpService`'s own OTLP exporter for server spans (using the same endpoint URL as `OtelService`).
- `[server.http.route.test]`: dispatches `GET /test` to the guest's `run` function.

## Guest Component

The guest reads the propagated trace context, then creates a child span under it:

```rust
let outer = tracing::outer_span_context();

let span_context = tracing::SpanContext {
    trace_id: outer.trace_id.clone(),
    span_id: /* freshly generated */,
    trace_flags: tracing::TraceFlags::SAMPLED,
    is_remote: false,
    trace_state: vec![],
};
tracing::on_start(&span_context);

// ... emit log with trace_id / span_id from outer ...

tracing::on_end(&tracing::SpanData {
    span_context: span_context.clone(),
    parent_span_id: outer.span_id.clone(),
    // ...
});
```

`outer-span-context` returns the span context the host opened for this invocation. If the inbound HTTP request included a `traceparent`, that trace id flows all the way down to the guest span.

## Building and Running

Prerequisite: an OTLP collector at `localhost:4317`.

```bash
./build.sh   # builds the guest wasm + host binary
./run.sh     # starts the host on :8080
```

In another terminal:

```bash
./curl.sh
```

This sends two requests:

1. `GET /test` with no traceparent: the host creates a root trace.
2. `GET /test` with a traceparent header: the host span becomes a child of that remote parent, and the guest span is a child of the host span.

At the collector you should see, for each request:
- An HTTP server span (from `HttpService`).
- A guest span named `otel-service-example` whose `parent_span_id` is the server span's id.
- A log record correlated to the guest span via `trace_id` / `span_id`.
