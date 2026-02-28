# Uppercase Example

Demonstrates interception of an **interface export** with string parameters and return values.

## Components

- **Target: greeter** (JavaScript) exports `say-hello` and `say-goodbye` via `modulewise:examples/greeter`
- **Advice: uppercaser** (Rust) uppercases string return values

## WIT

```wit
interface greeter {
  say-hello: func(name: string) -> string;
  say-goodbye: func(name: string) -> string;
}

world greeter-world {
  export greeter;
}
```

## Build

```sh
./build.sh
```

1. Generates the interceptor component from the WIT world using `cargo run` (interceptor CLI)
2. Builds the JavaScript greeter target using `jco componentize`
3. Builds the Rust uppercaser advice using `cargo component`
4. Composes all three into `composed.wasm` using `wac plug`

## Run

```sh
./run.sh
```

Calls `say-hello("world")` — the greeter returns `"Hello world!"`, the uppercaser advice transforms it to **`"HELLO WORLD!"`**.
