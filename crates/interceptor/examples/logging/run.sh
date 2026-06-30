#!/bin/sh

set -euo pipefail

cd "$(dirname "$0")"

if [[ ! -f lib/composed.wasm ]]; then
  echo "Error: lib/composed.wasm not found. Run build.sh first."
  exit 1
fi

echo "say-hello(\"World\")"
RESULT=$(wasmtime run --invoke 'say-hello("World")' lib/composed.wasm)
echo "$RESULT"

echo "say-goodbye(\"World\")"
RESULT=$(wasmtime run --invoke 'say-goodbye("World")' lib/composed.wasm)
echo "$RESULT"

echo "add(3, 4)"
RESULT=$(wasmtime run --invoke 'add(3, 4)' lib/composed.wasm)
echo "$RESULT"

echo "subtract(7, 4)"
RESULT=$(wasmtime run --invoke 'subtract(7, 4)' lib/composed.wasm)
echo "$RESULT"
