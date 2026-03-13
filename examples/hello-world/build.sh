#!/bin/bash

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

( cd "$SCRIPT_DIR/greeter"; cargo component build --target wasm32-unknown-unknown --release )
