#!/bin/bash

set -e

for project in greeter logger; do
  cargo build -p "$project" --target wasm32-unknown-unknown --release
  wasm-tools component new \
    "target/wasm32-unknown-unknown/release/${project}.wasm" \
    -o "lib/${project}.wasm"
done

if [[ ! -f lib/wasi-logging-to-stdout.wasm ]]; then
  echo "Fetching WASI logging adapter..."
  wkg oci pull -o lib/wasi-logging-to-stdout.wasm ghcr.io/componentized/logging/to-stdout:v0.2.1
fi
