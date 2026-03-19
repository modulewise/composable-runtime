#!/bin/bash

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

(
  cd "$SCRIPT_DIR/components"
  cargo component build --target wasm32-unknown-unknown --release
)

for f in "$SCRIPT_DIR"/components/target/wasm32-unknown-unknown/release/*.wasm; do
  cp "$f" "$SCRIPT_DIR/lib/$(basename "$f" | tr '_' '-')"
done

if [[ ! -f "$SCRIPT_DIR/lib/wasi-logging-to-stdout.wasm" ]]; then
  echo "Fetching WASI logging adapter..."
  wkg oci pull -o "$SCRIPT_DIR/lib/wasi-logging-to-stdout.wasm" ghcr.io/componentized/logging/to-stdout:v0.2.1
fi
