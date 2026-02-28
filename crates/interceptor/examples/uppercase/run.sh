#!/bin/sh

set -euo pipefail

cd "$(dirname "$0")"

if [[ ! -f composed.wasm ]]; then
  echo "Error: composed.wasm not found. Run build.sh first."
  exit 1
fi

echo "Running: say-hello(\"world\")"
RESULT=$(wasmtime run --invoke 'say-hello("world")' composed.wasm)
echo "Result: $RESULT"
