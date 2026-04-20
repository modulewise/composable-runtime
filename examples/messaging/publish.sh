#!/bin/bash

set -e

if ! command -v composable &>/dev/null; then
  echo "Error: composable CLI not found (cargo install composable-runtime)"
  exit 1
fi

GREETER=../hello-world/greeter/target/wasm32-unknown-unknown/release/greeter.wasm

if [ ! -f "$GREETER" ]; then
  ../hello-world/build.sh
fi

composable publish config.toml --channel names --body "${1:-World}" --reply-timeout 1
