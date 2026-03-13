# Composable Runtime Interceptor

**Aspect-Oriented Programming for Wasm Components**

The Composable Runtime Interceptor library creates an *interceptor component* that wraps another component's exported functions, inserting before and after advice hooks around every call without modifying the target component. The generated interceptor component can then be composed with any component that implements the generic advice interface and any component that implements the selected target interface(s).

```
          ┌─────────────┐     ┌─────────────┐
          │ interceptor │────▶│   target    │
caller ──▶│ (generated) │     │  component  │
          │             │◀────│             │
          └──────┬──────┘     └─────────────┘
                 │
         before()│after()
                 ▼
          ┌─────────────┐
          │   advice    │
          │  component  │
          └─────────────┘
```

---

## The Advice Protocol

The interceptor imports `modulewise:interceptor/advice@0.1.0`. Any advice component implementation can export this interface.

```wit
interface advice {
    use types.{arg, value};

    resource invocation {
        constructor(function-name: string, args: list<arg>);
        before: func() -> before-action;
        after: func(ret: option<value>) -> after-action;
    }
}
```

For each intercepted call, the interceptor:

1. Creates an `invocation` with the function name and arguments
2. Calls `before()` so the advice can inspect/modify args, skip the target, or proceed
3. Calls the target function (unless skipped or error)
4. Calls `after()` with the return value so the advice can accept, repeat the call, or error

### before-action

| Variant | Meaning |
|---|---|
| `proceed(list<arg>)` | Call the target with these args (complex-typed values use originals) |
| `skip(option<value>)` | Return this value directly without calling the target |
| `error(string)` | Trap immediately |

### after-action

| Variant | Meaning |
|---|---|
| `accept(option<value>)` | Return this value to the caller |
| `repeat(list<arg>)` | Call the target again with these args |
| `error(string)` | Trap immediately |

### arg and value

```wit
record arg {
    name: string,       // parameter name, e.g. "count"
    type-name: string,  // WIT type name, e.g. "u32"
    value: value,
}

variant value {
    str(string),
    num-s64(s64),
    num-u64(u64),
    num-f32(f32),
    num-f64(f64),
    boolean(bool),
    complex(string),    // opaque (advice sees the type name but not the value)
}
```

Primitive types (`bool`, integers, floats, `char`) and strings are fully readable and writable by advice. Complex types (records, lists, variants, etc.) are passed as `complex("")` which means advice cannot read, modify, or replace them.

> [!NOTE]
> If access to complex types is required within an interceptor implementation, implement a dedicated component that explicitly imports and exports the same interface as exported by the target component (instead of *generic* cross-cutting advice).

---

## CLI

```
interceptor --world <world> [--wit <path>] [--match <pattern>]... --output <file>
```

| Flag | Default | Description |
|---|---|---|
| `--world` | *(required)* | World name whose exports define the interceptor contract |
| `--wit` | `wit/` | Path to WIT file or directory |
| `--match` | *(none => intercept all)* | Pattern for selective interception (repeatable) |
| `--output` / `-o` | *(required)* | Output path for the generated interceptor `.wasm` |

### Examples

Intercept everything exported by `my-world`:
```sh
interceptor --world my-world --output interceptor.wasm
```

Intercept only `say-hello` in the `greeter` interface:
```sh
interceptor --world my-world --match 'modulewise:examples/greeter#say-hello' --output interceptor.wasm
```

Intercept all functions in the `greeter` interface, bypass anything else:
```sh
interceptor --world my-world --match 'modulewise:examples/greeter' --output interceptor.wasm
```

Intercept all functions across all interfaces in the `modulewise` namespace:
```sh
interceptor --world my-world --match 'modulewise:*' --output interceptor.wasm
```

### Pattern Syntax

Patterns select which exported functions to intercept. Functions not matched are *bypassed* as direct aliases. If no `--match` flags are provided, all functions are intercepted.

| Pattern form | Matches |
|---|---|
| `namespace:pkg/iface#func` | Exact function in exact interface |
| `namespace:pkg/iface` | All functions in that interface |
| `namespace:pkg/*` | All functions in all interfaces of that package |
| `namespace:*` | All functions in all packages of that namespace |
| `*` | Everything |
| `func-name` | Direct (world-level) function by exact name |
| `func-*` | Direct functions matching the wildcard |

---

## Library API

The Composable Runtime Interceptor is usable as a library crate for programmatic interceptor generation.

```toml
[dependencies]
interceptor = { package = "composable-interceptor", version = "0.1" }
```

### From a WIT path

```rust
use std::path::Path;

let bytes = interceptor::create_from_wit(
    Path::new("wit/"),
    "my-world",
    &["modulewise:examples/greeter"],
)?;
std::fs::write("interceptor.wasm", bytes)?;
```

### From a Wasm Component binary

When a target component is available, the WIT world can be extracted from the component's embedded type information:

```rust
let target_bytes: &[u8] = /* ... */;

let interceptor_bytes = interceptor::create_from_component(
    target_bytes,
    &[], // empty => intercept all exports
)?;
```

Both functions return validated wasm component bytes ready for composition.

---

## Examples

| Example | Demonstrates |
|---|---|
| [`examples/square`](examples/square/) | Direct function exports, primitive types |
| [`examples/uppercase`](examples/uppercase/) | String parameter and return value |
| [`examples/logging`](examples/logging/) | Multiple interfaces, WASI logging import in advice |

> [!NOTE]
> In each of the examples above, the advice components are implemented in Rust, but wasm composition works at the bytecode level regardless of source language. To demonstrate this, the `square` example uses a target component [implemented in Python](examples/square/calculator/calculator.py), and the `uppercase` example uses a target component [implemented in JavaScript](examples/uppercase/greeter/greeter.js). The logging example uses Rust for all of its components, but its `greeter` and `calculator` components export the same WIT functions as those other two examples.

---

## Limitations

- **Complex types in advice**: Records, lists, variants, etc. are forwarded opaquely. Advice can observe their presence (via `type-name`) but cannot read, modify, or replace them.
- **Error handling**: When advice returns `error(string)`, the interceptor traps. The error string is not currently surfaced to the host, but in the future might be written to an address that is known to the composable-runtime host for better error reporting.

---

## How It Works

Given a WIT world, the interceptor generates three core wasm modules and assembles them into a single wasm component:

- **main module**: marshals arguments into the advice protocol, calls `before()` and `after()`, and handles the returned actions
- **shim module**: provides indirect call stubs so the main module can be instantiated before its real imports are available
- **fixup module**: patches the shim's table with the real lowered function references at instantiation time

The component imports the target interface(s) and `modulewise:interceptor/advice`, then re-exports the target interface(s) with interception applied. Bypassed functions are aliased. The generated component is a standard wasm component with no special runtime support required. Notice that the run scripts in the examples use [wasmtime](https://github.com/bytecodealliance/wasmtime) directly instead of composable-runtime.
