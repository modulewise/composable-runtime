#!/bin/bash

set -e

cd "$(dirname "$0")"

for project in greeter translator; do
  cargo build -p "$project" --target wasm32-unknown-unknown --release
  wasm-tools component new \
    "target/wasm32-unknown-unknown/release/${project}.wasm" \
    -o "lib/${project}.wasm"
done
