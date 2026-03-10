# Modulewise Composable Runtime

An Inversion of Control Runtime for Wasm Components

## Examples

- [hello-world](examples/hello-world): A simple greeter component invoked by a host application
- [host-capability](examples/host-capability): A greeter component calls a host-provided function to get its greeting
- [interceptor](examples/interceptor): An interceptor component dynamically generated from generic advice logs before and after greeter function calls
- [otel](examples/otel): A guest component uses wasi:otel backed by either a host-capability or an adapter component
- [service](examples/service): A custom Service provides a ConfigHandler, HostCapability, and its own lifecycle

This project also provides a foundation for the
[Modulewise Toolbelt](https://github.com/modulewise/toolbelt).

## License

Copyright (c) 2026 Modulewise Inc and the Modulewise Composable Runtime contributors.

Apache License v2.0: see [LICENSE](./LICENSE) for details.
