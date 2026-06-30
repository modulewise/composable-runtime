#!/bin/bash

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

( cd "$SCRIPT_DIR/greeter"; cargo build --target wasm32-unknown-unknown --release )
wasm-tools component new \
  "$SCRIPT_DIR/greeter/target/wasm32-unknown-unknown/release/greeter.wasm" \
  -o "$SCRIPT_DIR/lib/greeter.wasm"
