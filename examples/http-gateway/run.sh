#!/bin/bash

set -e

GREETER=../hello-world/greeter/target/wasm32-unknown-unknown/release/greeter.wasm

if [ ! -f "$GREETER" ]; then
  ../hello-world/build.sh
fi

RUST_LOG=info cargo run -q --manifest-path ../../crates/gateway-http/Cargo.toml -- \
  config.toml
