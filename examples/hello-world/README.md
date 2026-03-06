# Hello World Example

A simple greeter component invoked via CLI.

## Structure

```
hello-world/
├── greeter/          # Wasm Component
│   └── src/lib.rs    # Implements the greet(name) function
├── wit/              # WIT interface definitions
│   └── greeter.wit   # Defines the greet(name) function
└── config.toml       # Component configuration for the runtime
```

## Build

```bash
./build.sh
```

Builds the `greeter` Wasm Component.

## Run

```bash
./run.sh
```

Invokes the `greeter` via `composable invoke`, passing "World".

Output:
```
Result: "Hello, World!"
```

## Configuration

Edit `config.toml` to customize the greeting:

```toml
[component.greeter]
uri = "./greeter/target/wasm32-unknown-unknown/release/greeter.wasm"
config.greeting = "Aloha"
```

Output with custom greeting:
```
Result: "Aloha, World!"
```
