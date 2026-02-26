#!/bin/sh

set -euo pipefail

cd "$(dirname "$0")"

echo "Generating interceptor..."
cargo run --manifest-path ../../Cargo.toml -- \
  --wit ../wit \
  --world greeter-world \
  --output interceptor.wasm

echo "Building greeter (JavaScript)..."
(cd greeter; npm install && npm run build)

echo "Building uppercaser (Rust)..."
(cd uppercaser; cargo component build --target wasm32-unknown-unknown --release)

echo "Composing..."
wac plug \
  --plug greeter/greeter.wasm \
  --plug uppercaser/target/wasm32-unknown-unknown/release/uppercaser.wasm \
  interceptor.wasm \
  -o composed.wasm
