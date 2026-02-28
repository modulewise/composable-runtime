# Logging Example

Demonstrates **selective interception** across multiple interfaces and direct exports, with a WASI-enabled advice component that logs before and after each call.

## Components

All three components are Rust:

- **Target: calculator** exports `add` and `subtract` as direct functions
- **Target: greeter** exports `say-hello` and `say-goodbye` via `modulewise:examples/greeter`
- **Advice: logger** logs function name, args, and return values via `wasi:logging/logging`, then modifies results (uppercases strings, squares integers)

## WIT

```wit
world multi-world {
  export add: func(a: s32, b: s32) -> s32;
  export subtract: func(a: s32, b: s32) -> s32;
  export greeter;
}
```

The interceptor is generated with `--match` patterns to selectively intercept only `say-hello` and `add`, bypassing `say-goodbye` and `subtract`:

```sh
--match modulewise:examples/greeter#say-hello \
--match add
```

## Build

```sh
./build.sh
```

1. Generates the interceptor with selective `--match` patterns
2. Builds all three Rust components using `cargo component`
3. Fetches a WASI logging-to-stdout adapter via `wkg oci pull`
4. Composes the logger with the WASI adapter using `wac plug`
5. Composes the interceptor with both targets and the adapted logger into `composed.wasm`

## Run

```sh
./run.sh
```

Calls all four functions. Intercepted calls (`say-hello`, `add`) show before/after log output and have their results modified. Bypassed calls (`say-goodbye`, `subtract`) pass through unchanged with no logging.
