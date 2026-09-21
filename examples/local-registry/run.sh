#!/bin/bash

set -e

if ! command -v composable &>/dev/null; then
  echo "Error: composable CLI not found (cargo install composable-runtime)"
  exit 1
fi

cd "$(dirname "$0")"

export COMPOSABLE_OCI_REGISTRY_CONFIG=registries.toml

composable invoke config.toml -- greeter.greet World
