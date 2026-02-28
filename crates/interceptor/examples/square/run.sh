#!/bin/sh

set -euo pipefail

cd "$(dirname "$0")"

if [[ ! -f composed.wasm ]]; then
  echo "Error: composed.wasm not found. Run build.sh first."
  exit 1
fi

echo "Running: add(3, 4)"
RESULT=$(wasmtime run --invoke 'add(3, 4)' composed.wasm)
echo "Result: $RESULT"
