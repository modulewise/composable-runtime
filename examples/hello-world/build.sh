#!/bin/bash

set -e

( cd greeter; cargo component build --target wasm32-unknown-unknown --release )
