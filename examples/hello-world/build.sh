#!/bin/bash

set -e

cargo component build -p greeter --target wasm32-unknown-unknown --release

cargo build -p host --release
