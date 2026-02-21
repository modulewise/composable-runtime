# Modulewise Composable Runtime

A Composable Runtime for Wasm Components

## Examples

- [hello-world](examples/hello-world): A simple greeter component invoked by a host application
- [host-extension](examples/host-extension): A greeter component calls a host-provided function to get its greeting
- [otel](examples/otel): A guest component uses wasi:otel backed by either a host-feature or an adapter component

This project also provides a foundation for the
[Modulewise Toolbelt](https://github.com/modulewise/toolbelt).

## License

Copyright (c) 2026 Modulewise Inc and the Modulewise Composable Runtime contributors.

Apache License v2.0: see [LICENSE](./LICENSE) for details.
