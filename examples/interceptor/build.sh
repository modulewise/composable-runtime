#!/bin/bash

set -e

for project in greeter logger; do
  cargo build -p "$project" --target wasm32-unknown-unknown --release
  wasm-tools component new \
    "target/wasm32-unknown-unknown/release/${project}.wasm" \
    -o "lib/${project}.wasm"
done
