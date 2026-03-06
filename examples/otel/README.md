# OpenTelemetry Example

This example demonstrates sending OpenTelemetry logs from a guest wasm component to an OTLP-compatible collector via the [wasi:otel](https://github.com/WebAssembly/wasi-otel) WIT interface and a gRPC endpoint interface. Tracing and metrics will be added to the demo in a future update.

## Flow

There are two alternative configuration files. One includes a host capability, and the other relies on a `grpc-to-http` adapter component.

The capability-based path is:

```
   guest   ->   otel-to-grpc   ->   grpc-capability   ->   OTLP collector
(component)     (component)             (host)               (external)
```

The component-based path is:

```
   guest   ->   otel-to-grpc   ->   grpc-to-http   ->   wasi:http   ->   OTLP collector
(component)     (component)          (component)        (runtime)          (external)
```

The common flow is:

- **guest:** A user component that emits OpenTelemetry log records via `wasi:otel/logs`.
- **otel-to-grpc:** A wasm component that converts `wasi:otel` log records into OTLP protobuf and sends through `modulewise:grpc/endpoint`.
- **grpc endpoint:** Either `grpc-capability` or `grpc-to-http` delivers the serialized protobuf to a gRPC service. The two interchangeable implementations are described below.

> [!NOTE]
> The `guest` and `otel-to-grpc` components are identical in both configurations. Only the endpoint provider differs, and that is a config (not code) change.

## The gRPC Endpoint Interface

Both endpoint implementations use the same WIT interface:

```wit
package modulewise:grpc@0.1.0;

interface endpoint {
  send: func(path: string, data: list<u8>) -> result<_, string>;
}
```

The endpoint is generic gRPC (OTLP-agnostic, hence the `otel-to-grpc` component). It just sends raw bytes to a named path. Path keys like `logs` and `traces` map to gRPC service paths via config, either in the `host:grpc` capability definition, or the `grpc-to-http` component definition:

```toml
config.url = "http://localhost:4317"
config.paths.logs = "/opentelemetry.proto.collector.logs.v1.LogsService/Export"
config.paths.traces = "/opentelemetry.proto.collector.trace.v1.TraceService/Export"
```

## Two Endpoint Options

### Option 1: Host Capability (`grpc-capability`)

This endpoint is implemented as a [host extension](grpc-capability). The host maintains a persistent gRPC channel that is shared across component invocations. Batching support will also be added at this layer in a future update (it will eventually move from examples to a feature crate).

```toml
# config-with-host-capability.toml

[component.guest]
uri = "./target/wasm32-unknown-unknown/release/guest.wasm"
imports = ["otel"]

[component.otel]
uri = "./target/wasm32-wasip2/release/otel_to_grpc.wasm"
imports = ["grpc", "wasip2"]

[capability.grpc]
uri = "host:grpc"
config.url = "http://localhost:4317"
config.paths.logs = "/opentelemetry.proto.collector.logs.v1.LogsService/Export"
config.paths.traces = "/opentelemetry.proto.collector.trace.v1.TraceService/Export"

[capability.wasip2]
uri = "wasmtime:wasip2"
```

The "grpc" definition with `uri = "host:grpc"` creates an instance of the host capability, which the host binary has registered as an extension:

```rust
let runtime = Runtime::builder(&graph)
    .with_host_extension::<GrpcCapability>("grpc")
    .build()
    .await?;
```

**Advantage:** The underlying channel persists across invocations. If the guest component is instantiated multiple times, all reuse the same connection. If other components rely on the same capability instance, they also share the connection.

### Option 2: Component as Adapter (`grpc-to-http`)

This endpoint is a wasm component that translates gRPC calls into `wasi:http` requests. The runtime's built-in HTTP support (using h2c for `application/grpc`) handles the actual connection.

```toml
# config-with-components.toml

[component.guest]
uri = "./target/wasm32-unknown-unknown/release/guest.wasm"
imports = ["otel"]

[component.otel]
uri = "./target/wasm32-wasip2/release/otel_to_grpc.wasm"
imports = ["grpc", "wasip2"]

[component.grpc]
uri = "./target/wasm32-unknown-unknown/release/grpc_to_http.wasm"
imports = ["http", "io"]
config.url = "http://localhost:4317"
config.paths.logs = "/opentelemetry.proto.collector.logs.v1.LogsService/Export"
config.paths.traces = "/opentelemetry.proto.collector.trace.v1.TraceService/Export"

[capability.http]
uri = "wasmtime:http"

[capability.io]
uri = "wasmtime:io"

[capability.wasip2]
uri = "wasmtime:wasip2"
```

The `grpc-to-http` component constructs a gRPC-framed request and sends it via `wasi:http/outgoing-handler`.

**Advantage:** Pure wasm component model, bottoms out at `wasi:http`. No host capability needed.

**Trade-off:** Each component invocation creates a new HTTP/2 connection. Under load, the host capability's connection reuse becomes significant.

## WIT Worlds

Each component has a WIT world defining its imports and exports:

```wit
// Guest: uses wasi:otel to emit logs
world guest {
    import wasi:otel/logs@0.2.0-rc.2+patch;
    export test;
}

// Adapter: converts wasi:otel to OTLP protobuf, sends via grpc endpoint
world otel-to-grpc {
    export wasi:otel/logs@0.2.0-rc.2+patch;
    import modulewise:grpc/endpoint@0.1.0;
    import wasi:clocks/wall-clock@0.2.6;
}

// Component Endpoint: translates grpc to wasi:http
world grpc-to-http {
    export modulewise:grpc/endpoint@0.1.0;
    import wasi:http/outgoing-handler@0.2.6;
    import wasi:config/store@0.2.0-rc.1;
}
```

The composable-runtime composes these together based on the config graph: the guest's `import wasi:otel/logs` is satisfied by otel-to-grpc's `export wasi:otel/logs`, and otel-to-grpc's `import modulewise:grpc/endpoint` is satisfied by either the host capability or the grpc-to-http component.

## Building and Running

Prerequisite: an OTLP collector at localhost:4317

```bash
./build.sh   # builds all wasm components + host binary
./run.sh     # runs both configs against an OTLP collector at localhost:4317
```

At the collector, you should see two log entries with the following log-record bodies:
- "testing config-with-host-capability.toml"
- "testing config-with-components.toml"
