#!/bin/bash

set -e

if ! command -v composable &>/dev/null; then
  echo "Error: composable CLI not found (cargo install composable-runtime)"
  exit 1
fi

composable invoke config.toml -- greeter.greet World
