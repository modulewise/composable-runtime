#!/bin/sh

set -euo pipefail

cd "$(dirname "$0")"

echo "Generating interceptor..."
cargo run --manifest-path ../../Cargo.toml -- \
  --match modulewise:examples/greeter#say-hello \
  --match add \
  --wit ../wit \
  --world multi-world \
  --output lib/interceptor.wasm

echo "Building components..."
cargo build --target wasm32-unknown-unknown --release
for project in calculator greeter logger; do
  wasm-tools component new \
    "target/wasm32-unknown-unknown/release/${project}.wasm" \
    -o "lib/${project}.wasm"
done

if [[ ! -f lib/wasi-logging-to-stderr.wasm ]]; then
  echo "Fetching WASI logging adapter..."
  wkg oci pull -o lib/wasi-logging-to-stderr.wasm ghcr.io/componentized/logging/to-stderr:0.3.1
fi

echo "Composing..."
wac plug -o lib/stderr-logger.wasm \
  --plug lib/wasi-logging-to-stderr.wasm \
  lib/logger.wasm

wac plug -o lib/composed.wasm \
  --plug lib/calculator.wasm \
  --plug lib/greeter.wasm \
  --plug lib/stderr-logger.wasm \
  lib/interceptor.wasm
