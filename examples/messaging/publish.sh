#!/bin/bash

set -e

GREETER=../hello-world/greeter/target/wasm32-unknown-unknown/release/greeter.wasm

if [ ! -f "$GREETER" ]; then
  ../hello-world/build.sh
fi

cargo run -q --manifest-path ../../Cargo.toml -- \
  publish config.toml --channel names --body "${1:-World}"
