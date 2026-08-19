#!/bin/bash

set -e

if ! command -v composable &>/dev/null; then
  echo "Error: composable CLI not found (cargo install composable-runtime)"
  exit 1
fi

cd "$(dirname "$0")"

echo "== list =="
composable invoke config-readonly.toml -- file-store.list-files

echo "== read from-host.txt =="
composable invoke config-readonly.toml -- file-store.read from-host.txt

echo "== write from-guest.txt =="
composable invoke config-readwrite.toml -- file-store.write from-guest.txt "hello from the guest"

echo "== list again =="
composable invoke config-readonly.toml -- file-store.list-files

echo "== read from-guest.txt back =="
composable invoke config-readonly.toml -- file-store.read from-guest.txt

echo "== write denied under read-only =="
composable invoke config-readonly.toml -- file-store.write denied.txt "nope" || true
