#!/bin/bash

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TARGET=wasm32-unknown-unknown

(
  cd "$SCRIPT_DIR/components"

  for project in $(cargo metadata --no-deps --format-version 1 | jq -r '.packages[].name'); do
    echo "Building $project..."
    cargo build -p "$project" --target "$TARGET" --release

    cargo_name=$(echo "$project" | tr '-' '_')
    core="target/${TARGET}/release/${cargo_name}.wasm"
    wasm-tools component new "$core" -o "$SCRIPT_DIR/lib/${project}.wasm"
    echo "  -> lib/${project}.wasm"
  done
)

if [[ ! -f "$SCRIPT_DIR/lib/wasi-logging-to-stdout.wasm" ]]; then
  echo "Fetching WASI logging adapter..."
  wkg oci pull -o "$SCRIPT_DIR/lib/wasi-logging-to-stdout.wasm" ghcr.io/componentized/logging/to-stdout:v0.2.1
fi
