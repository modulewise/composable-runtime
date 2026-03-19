#!/bin/bash

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
EXAMPLE_DIR="$(dirname "$SCRIPT_DIR")"
LIB_DIR="$EXAMPLE_DIR/lib"
OUT_DIR="$SCRIPT_DIR/out"

for f in configured-greeter.wasm capable-translator.wasm logging-advice.wasm wasi-logging-to-stdout.wasm; do
  if [[ ! -f "$LIB_DIR/$f" ]]; then
    echo "Error: $LIB_DIR/$f not found. Run ../build.sh first."
    exit 1
  fi
done

mkdir -p "$OUT_DIR"

# Step 1: Generate static config component (locale=de-DE)
echo "Generating config component..."
static-config -p locale=de-DE -o "$OUT_DIR/config.wasm"

# Step 2: Generate interceptor wrapper for the translator interface
echo "Generating interceptor wrapper..."
waspect --world capable-translator --wit "$EXAMPLE_DIR/wit/" -o "$OUT_DIR/translator-interceptor.wasm"

# Step 3: Compose logging advice into the interceptor wrapper
echo "Composing logging-translator..."
wac plug --plug "$LIB_DIR/logging-advice.wasm" "$OUT_DIR/translator-interceptor.wasm" -o "$OUT_DIR/logging-translator.wasm"

# Step 4: Compose all components using the WAC file
echo "Composing final component..."
wac compose "$SCRIPT_DIR/compose.wac" \
  --dep "modulewise:capable-translator=$LIB_DIR/capable-translator.wasm" \
  --dep "modulewise:logging-to-stdout=$LIB_DIR/wasi-logging-to-stdout.wasm" \
  --dep "modulewise:logging-translator=$OUT_DIR/logging-translator.wasm" \
  --dep "modulewise:config=$OUT_DIR/config.wasm" \
  --dep "modulewise:greeter=$LIB_DIR/configured-greeter.wasm" \
  -o "$OUT_DIR/composed-greeter.wasm"

echo ""
echo "Composition completed. Run ./invoke.sh to invoke composed-greeter.wasm"
