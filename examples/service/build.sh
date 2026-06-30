#!/bin/bash

set -e

cargo build -p greeter --target wasm32-unknown-unknown --release
wasm-tools component new \
  target/wasm32-unknown-unknown/release/greeter.wasm \
  -o lib/greeter.wasm

cargo build -p host --release
