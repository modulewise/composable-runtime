#!/bin/bash

set -e

cargo build -p guest --target wasm32-unknown-unknown --release
wasm-tools component new \
  target/wasm32-unknown-unknown/release/guest.wasm \
  -o lib/guest.wasm

cargo build -p otel-to-grpc --target wasm32-wasip2 --release
cp target/wasm32-wasip2/release/otel_to_grpc.wasm lib/otel-to-grpc.wasm

cargo build -p grpc-to-http --target wasm32-unknown-unknown --release
wasm-tools component new \
  target/wasm32-unknown-unknown/release/grpc_to_http.wasm \
  -o lib/grpc-to-http.wasm

cargo build -p grpc-capability --release

cargo build -p host --release
