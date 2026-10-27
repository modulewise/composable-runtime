#!/bin/bash

set -euo pipefail

DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$DIR"

if ! command -v composable &>/dev/null; then
  echo "Error: composable CLI not found (cargo install composable-runtime)"
  exit 1
fi

INPUT=$(jq -cn --arg name "${1:-World}" '{name: $name}')

echo "metadata:"
composable invoke config.toml -- greet.function.metadata

echo "call:"
composable invoke config.toml -- greet.function.call "$INPUT" | jq 'fromjson'
