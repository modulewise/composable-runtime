# Square Example

Demonstrates interception of **direct function exports** with primitive integer types.

## Components

- **Target: calculator** (Python) exports `add` and `subtract`
- **Advice: squarer** (Rust) squares the return value of every intercepted call

## WIT

```wit
world calculator-world {
  export add: func(a: s32, b: s32) -> s32;
  export subtract: func(a: s32, b: s32) -> s32;
}
```

## Build

```sh
./build.sh
```

1. Generates the interceptor component from the WIT world using `cargo run` (interceptor CLI)
2. Builds the Python calculator target using `componentize-py`
3. Builds the Rust squarer advice using `cargo component`
4. Composes all three into `composed.wasm` using `wac plug`

## Run

```sh
./run.sh
```

Calls `add(3, 4)` — the calculator returns 7, the squarer advice squares it to **49**.