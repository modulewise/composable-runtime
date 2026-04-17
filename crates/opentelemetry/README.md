# Composable OpenTelemetry

**Host capability providing `wasi:otel` to guest components.**

Guest components can emit spans via `wasi:otel/tracing` and log records via `wasi:otel/logs` (metrics will be added soon). The host capability batches and forwards to an OTLP endpoint.

---

## Configuration

Capability instances are defined under `[capability.<name>]` with `type="otel"` in TOML config files.

```toml
[capability.otel]
type = "otel"
endpoint = "http://localhost:4317"
protocol = "grpc"

[capability.otel.resource]
"service.name" = "my-service"
"service.version" = "0.1.0"
```

### Properties

| Property | Default | Description |
|---|---|---|
| `endpoint` | `http://localhost:4317` | OTLP endpoint URL |
| `protocol` | `grpc` | OTLP protocol: `grpc` or `http/protobuf` |
| `resource` | `{}` | Table of resource attributes applied to spans and to log records that carry no inline resource |

### Multiple instances

Multiple `[capability.*]` blocks with `type = "otel"` create independent instances. Each has its own exporter pipeline (endpoint, protocol, resource). Components import a specific instance by the capability name under `[capability.<name>]`.

---

## Trace Context Propagation

The capability exposes `outer-span-context` on `wasi:otel/tracing`, returning the `SpanContext` the host opened for the current invocation. This is parsed from a `traceparent` (and optional `tracestate`) entry on the runtime's `PROPAGATION_CONTEXT` task-local, populated by whichever entry point accepted the inbound request. For example, `composable-http-server` writes the traceparent before invoking a component.

When no traceparent is in scope, `outer-span-context` returns an empty span context.

---

## Log Record Resources

Log records may optionally carry an inline `resource` field:

- Records **without** a resource route through the capability's default `SdkLoggerProvider`, which carries the capability's configured `[capability.*.resource]` attributes.
- Records **with** a resource go through a per-resource `SdkLoggerProvider`, keyed by sorted attribute pairs and cached for reuse..

---

## Library usage

Register the service with a `RuntimeBuilder`:

```rust
use composable_runtime::Runtime;
use composable_otel::OtelService;

let runtime = Runtime::builder()
    .from_paths(&config_paths)
    .with_service::<OtelService>()
    .build()
    .await?;

runtime.run().await
```

The service claims `[capability.*]` definitions where `type = "otel"`. On `start()` it initializes each instance's exporters and processors. On `shutdown()` it flushes and stops the batch processors for each instance.

---

## WIT

Bindings target the [wasi:otel](https://github.com/WebAssembly/wasi-otel) `tracing` and `logs` interfaces (the `wasi:otel@0.2.0-rc.2+patch` package has updated transitive wasi dependency versions to 0.2.6 to align with the core runtime). The `metrics` interface is not yet implemented.

See [examples/otel-service](../../examples/otel-service) for an end-to-end example including HTTP trace propagation.
