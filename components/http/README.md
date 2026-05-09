# composable:http

HTTP client support, including a WIT definition and a Wasm Component.

(for hosting Wasm Components behind an HTTP server, see [`crates/http-server`](../../crates/http-server))

## The `composable:http/client` Interface

Request functions (each returns `result<http-response, string>`):

- `request(method, url, headers, body, options)`
- `get(url, headers, options)`
- `post(url, headers, body, options)`
- `put(url, headers, body, options)`
- `delete(url, headers, options)`
- `patch(url, headers, body, options)`
- `head(url, headers, options)`
- `options(url, headers, options)`

The `options` arg includes per-request timeouts and response-size limits. If not provided, default timeouts will be used, and there will be no cap on the response size.

See [`wit/package.wit`](wit/package.wit) for the full type definitions.

## The `http-client` World

- exports `composable:http/client`
- imports `wasi:http/outgoing-handler`

That import can be satisfied by the `wasi:http` Capability which is available in the core runtime.

## The `http-client` Component

Implementation of the `http-client` world, with source code in the [client](client/) sub-directory.
