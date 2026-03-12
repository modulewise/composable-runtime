#!/bin/bash

set -e

(
  cd components
  cargo component build --target wasm32-unknown-unknown --release
)

for f in components/target/wasm32-unknown-unknown/release/*.wasm; do
  cp "$f" "./lib/$(basename "$f" | tr '_' '-')"
done

if [[ ! -f ./lib/wasi-logging-to-stdout.wasm ]]; then
  echo "Fetching WASI logging adapter..."
  wkg oci pull -o ./lib/wasi-logging-to-stdout.wasm ghcr.io/componentized/logging/to-stdout:v0.2.1
fi
