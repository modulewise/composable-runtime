#!/bin/sh

set -euo pipefail

cd "$(dirname "$0")"

echo "Generating interceptor..."
cargo run --manifest-path ../../Cargo.toml -- \
  --wit ../wit \
  --world greeter-world \
  --output lib/interceptor.wasm

echo "Building greeter (JavaScript)..."
(cd greeter; npm install && npm run build)

echo "Building uppercaser (Rust)..."
(cd uppercaser; cargo build --target wasm32-unknown-unknown --release)
wasm-tools component new \
  uppercaser/target/wasm32-unknown-unknown/release/uppercaser.wasm \
  -o lib/uppercaser.wasm

echo "Composing..."
wac plug \
  --plug lib/greeter.wasm \
  --plug lib/uppercaser.wasm \
  lib/interceptor.wasm \
  -o lib/composed.wasm
