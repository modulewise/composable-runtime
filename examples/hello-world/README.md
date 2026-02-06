# Hello World Example

A simple greeter component invoked by a host application.

## Structure

```
hello-world/
├── greeter/          # Wasm Component
│   └── src/lib.rs    # Implements the greet(name) function
├── host/             # Host application using composable-runtime
│   └── src/main.rs   # Loads and invokes the component
├── wit/              # WIT interface definitions
│   └── greeter.wit   # Defines the greet(name) function
└── config.toml       # Component configuration for the runtime
```

## Build

```bash
./build.sh
```

Builds the `greeter` Wasm Component and the `host` binary.

## Run

```bash
./run.sh
```

Output:
```
Result: "Hello, World!"
```

## Configuration

Edit `config.toml` to customize the greeting:

```toml
[greeter]
uri = "./target/wasm32-unknown-unknown/release/greeter.wasm"
exposed = true
config.greeting = "Aloha"
```

Output with custom greeting:
```
Result: "Aloha, World!"
```
