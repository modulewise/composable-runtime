#!/bin/bash

set -e

cargo component build --target wasm32-unknown-unknown --release

if [[ ! -f wasi-logging-to-stdout.wasm ]]; then
  echo "Fetching WASI logging adapter..."
  wkg oci pull -o wasi-logging-to-stdout.wasm ghcr.io/componentized/logging/to-stdout:v0.2.1
fi

COMPONENTS="target/wasm32-unknown-unknown/release"

wac plug -o logger.wasm \
  --plug wasi-logging-to-stdout.wasm \
  "$COMPONENTS/logger.wasm"
