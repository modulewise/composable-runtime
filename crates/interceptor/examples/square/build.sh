#!/bin/sh

set -euo pipefail

cd "$(dirname "$0")"

echo "Generating interceptor..."
cargo run --manifest-path ../../Cargo.toml -- \
  --wit ../wit \
  --world calculator-world \
  --output interceptor.wasm

echo "Building calculator (Python)..."
(cd calculator && uvx componentize-py \
  --wit-path ../../wit \
  --world calculator-world \
  componentize calculator \
  -o calculator.wasm)

echo "Building squarer (Rust)..."
(cd squarer; cargo component build --target wasm32-unknown-unknown --release)

echo "Composing..."
wac plug \
  --plug calculator/calculator.wasm \
  --plug squarer/target/wasm32-unknown-unknown/release/squarer.wasm \
  interceptor.wasm \
  -o composed.wasm
