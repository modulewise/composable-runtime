#!/bin/bash

set -e

wkg wit fetch

cargo build -p guest --target wasm32-unknown-unknown --release
wasm-tools component new \
  target/wasm32-unknown-unknown/release/guest.wasm \
  -o lib/guest.wasm

cargo build -p host --release
