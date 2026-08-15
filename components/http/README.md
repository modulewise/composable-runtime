# composable:http

HTTP client support, including a WIT definition and a Wasm Component.

(for hosting Wasm Components behind an HTTP server, see [`crates/http-server`](../../crates/http-server))

## The `composable:http/client` Interface

Every function is `async` and returns `result<http-response, string>`.
Request and response bodies are both `stream<u8>`.

Functions that accept a request body:

- `request(method, url, headers, body, options)`
- `post(url, headers, body, options)`
- `put(url, headers, body, options)`
- `patch(url, headers, body, options)`
- `query(url, headers, body, options)`

Functions without a request body:

- `get(url, headers, options)`
- `delete(url, headers, options)`
- `head(url, headers, options)`
- `options(url, headers, options)`
- `trace(url, headers, options)`

An `http-response` carries `status` and `headers`. The response `body` streams, and `trailers`
resolve once the body stream is fully consumed.

The `options` arg supplies per-request timeouts (`connect`, `first-byte`, `between-bytes`, all in
milliseconds). Any field left `none` falls through to the host default.

See [`wit/package.wit`](wit/package.wit) for the full type definitions.

## The `http-client` World

- exports `composable:http/client`
- imports `wasi:http/client@0.3.0`

That import can be satisfied by the `wasi:http` Capability which is available in the core runtime.

## The `http-client` Component

Implementation of the `http-client` world, with source code in the [client](client/) sub-directory.
