#!/bin/bash

set -e

if ! command -v composable-http-gateway &>/dev/null; then
  echo "Error: composable-http-gateway not found (cargo install composable-http-gateway)"
  exit 1
fi

GREETER=../hello-world/greeter/target/wasm32-unknown-unknown/release/greeter.wasm

if [ ! -f "$GREETER" ]; then
  ../hello-world/build.sh
fi

RUST_LOG=info composable-http-gateway config.toml
