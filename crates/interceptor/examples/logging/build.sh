#!/bin/sh

set -euo pipefail

cd "$(dirname "$0")"

echo "Generating interceptor..."
cargo run --manifest-path ../../Cargo.toml -- \
  --match modulewise:examples/greeter#say-hello \
  --match add \
  --wit ../wit \
  --world multi-world \
  --output interceptor.wasm

echo "Building components..."
cargo component build --target wasm32-unknown-unknown --release

if [[ ! -f wasi-logging-to-stdout.wasm ]]; then
  echo "Fetching WASI logging adapter..."
  wkg oci pull -o wasi-logging-to-stdout.wasm ghcr.io/componentized/logging/to-stdout:v0.2.1
fi

COMPONENTS="target/wasm32-unknown-unknown/release"

echo "Composing..."
wac plug -o stdout-logger.wasm \
  --plug wasi-logging-to-stdout.wasm \
  "$COMPONENTS/logger.wasm"

wac plug -o composed.wasm \
  --plug "$COMPONENTS/calculator.wasm" \
  --plug "$COMPONENTS/greeter.wasm" \
  --plug stdout-logger.wasm \
  interceptor.wasm
