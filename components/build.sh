#!/bin/sh

PROJECTS=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[].name')

for project in $PROJECTS; do
  echo "Building $project..."

  target=wasm32-unknown-unknown
  cargo build -p "$project" --target $target --release

  cargo_name=$(echo "$project" | tr '-' '_')
  core_wasm="target/${target}/release/${cargo_name}.wasm"
  wasm-tools component new "$core_wasm" -o "lib/${project}.wasm"
  echo "  -> lib/${project}.wasm"
done
