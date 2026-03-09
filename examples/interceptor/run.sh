#!/bin/bash

set -e

cargo run -q --manifest-path ../../Cargo.toml -- \
  invoke config.toml -- \
  greeter.greeter.greet World
