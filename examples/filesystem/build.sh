#!/bin/bash

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

( cd "$SCRIPT_DIR/file-store"; cargo build --target wasm32-unknown-unknown --release )
wasm-tools component new \
  "$SCRIPT_DIR/file-store/target/wasm32-unknown-unknown/release/file_store.wasm" \
  -o "$SCRIPT_DIR/lib/file-store.wasm"
