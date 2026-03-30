#!/bin/bash

set -e

wkg wit fetch

cargo component build -p guest --target wasm32-unknown-unknown --release

cargo build -p host --release
