#!/bin/bash

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
EXAMPLE_DIR="$(dirname "$SCRIPT_DIR")"
OUT_DIR="$SCRIPT_DIR/out"

if [[ ! -f "$OUT_DIR/composed-greeter.wasm" ]]; then
  echo "Error: composed-greeter.wasm not found. Run ./compose.sh first."
  exit 1
fi

# Start the translate API
node "$EXAMPLE_DIR/translate-api.js" &
TRANSLATE_PID=$!
trap "kill $TRANSLATE_PID 2>/dev/null; wait $TRANSLATE_PID 2>/dev/null" EXIT

# Wait for it to be ready
for i in $(seq 1 30); do
  if curl -s -o /dev/null http://localhost:8090/translate 2>/dev/null; then
    break
  fi
  sleep 0.1
done

echo "Invoking greet(\"World\")..."
wasmtime run -S cli -S http -S inherit-network --invoke 'greet("World")' "$OUT_DIR/composed-greeter.wasm"
