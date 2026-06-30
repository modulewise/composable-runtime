#!/bin/sh

set -euo pipefail

cd "$(dirname "$0")"

echo "Generating interceptor..."
cargo run --manifest-path ../../Cargo.toml -- \
  --wit ../wit \
  --world calculator-world \
  --output lib/interceptor.wasm

echo "Building calculator (Python)..."
(cd calculator && uvx componentize-py \
  --wit-path ../../wit \
  --world calculator-world \
  componentize calculator \
  -o ../lib/calculator.wasm)

echo "Building squarer (Rust)..."
(cd squarer; cargo build --target wasm32-unknown-unknown --release)
wasm-tools component new \
  squarer/target/wasm32-unknown-unknown/release/squarer.wasm \
  -o lib/squarer.wasm

echo "Composing..."
wac plug \
  --plug lib/calculator.wasm \
  --plug lib/squarer.wasm \
  lib/interceptor.wasm \
  -o lib/composed.wasm
