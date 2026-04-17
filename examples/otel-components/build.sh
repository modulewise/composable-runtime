#!/bin/bash

set -e

cargo component build -p guest --target wasm32-unknown-unknown --release

cargo build -p otel-to-grpc --target wasm32-wasip2 --release

cargo component build -p grpc-to-http --target wasm32-unknown-unknown --release

cargo build -p grpc-capability --release

cargo build -p host --release
