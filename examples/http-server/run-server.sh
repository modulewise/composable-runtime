#!/bin/bash

set -e

if ! command -v composable-http-server &>/dev/null; then
  echo "Error: composable-http-server not found (cargo install composable-http-server)"
  exit 1
fi

GREETER=../hello-world/greeter/target/wasm32-unknown-unknown/release/greeter.wasm

if [ ! -f "$GREETER" ]; then
  ../hello-world/build.sh
fi

RUST_LOG=info composable-http-server config.toml
