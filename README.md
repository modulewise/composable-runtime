# Composable Runtime

An Inversion of Control Runtime for Wasm Components

```
$ composable invoke config.toml -- greeter.greet World
"Bonjour, le Monde!"
```

<table><tr>
<td width="40%" valign="top">

![composition-example](examples/7cs/diagrams/4-capability.svg)

</td>
<td>

```toml
[component.greeter]
uri = "./lib/greeter.wasm"
imports = ["translator"]
config.locale = "fr-FR"

[component.translator]
uri = "./lib/translator.wasm"
imports = ["http", "io"]

[capability.http]
uri = "wasmtime:http"

[capability.io]
uri = "wasmtime:io"
```

</td>
</tr></table>

## Core Concepts

The best way to learn the core concepts of Composable Runtime is to go through the step-by-step overview in the [Sailing the 7 Cs with Composable Runtime](examples/7cs/README.md) example.

That incrementally introduces each concept along the way:
1. **Component:** the Wasm Component that will run in an isolated sandbox, with reference to a portable artifact
2. **Composition:** declarative dependency injection for components, with late-binding
3. **Configuration:** composition with a configuration-providing component, with encapsulation
4. **Capability:** access to external systems for components, with the least-privilege principle
5. **Cross-Cutting Concerns:** declarative aspect-oriented programming for components, with dynamic generation
6. **Channel:** messaging subscriptions for components, with transport/protocol decoupling
7. **Collaboration:** separation of concerns in configuration, with environment-aware binding of domain-centric components

## Other Examples

- [hello-world](examples/hello-world): A simple greeter component invoked by a host application
- [host-capability](examples/host-capability): A greeter component calls a host-provided function to get its greeting
- [http-gateway](examples/http-gateway): The greeter component invoked by the runtime when HTTP requests arrive, either directly or via a messaging channel depending on the route
- [interceptor](examples/interceptor): An interceptor component, dynamically generated from generic advice, logs before and after greeter function calls
- [messsaging](examples/messaging): The greeter component invoked when messages arrive via `composable publish`
- [otel](examples/otel): A guest component uses wasi:otel backed by either a host-capability or an adapter component
- [service](examples/service): A custom Service provides a ConfigHandler, HostCapability, and its own lifecycle

This project also provides a foundation for the
[Modulewise Toolbelt](https://github.com/modulewise/toolbelt).

## License

Copyright (c) 2026 Modulewise Inc and the Composable Runtime contributors.

Apache License v2.0: see [LICENSE](./LICENSE) for details.
