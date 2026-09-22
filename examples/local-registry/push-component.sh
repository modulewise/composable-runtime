#!/bin/bash

set -e

cd "$(dirname "$0")"

REGISTRY="localhost:5001"
REFERENCE="$REGISTRY/example/greeter:0.1.0"
COMPONENT="../hello-world/lib/greeter.wasm"

if ! command -v wkg &>/dev/null; then
  echo "Error: wkg not found (cargo install wkg)"
  exit 1
fi

if [[ ! -f "$COMPONENT" ]]; then
  echo "Error: $COMPONENT not found. Build the hello-world example first."
  exit 1
fi

wkg oci push --insecure "$REGISTRY" "$REFERENCE" "$COMPONENT"
