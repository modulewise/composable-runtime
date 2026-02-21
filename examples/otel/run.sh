#!/bin/bash

set -e

if [ ! -f ./target/release/host ]; then
    echo -n "Host binary not found. Run build.sh first? [Y/n] "
    read -r -n 1 answer
    echo
    if [ "$answer" != "n" ] && [ "$answer" != "N" ]; then
        ./build.sh
    else
        exit 1
    fi
fi

./target/release/host config-with-host-feature.toml

./target/release/host config-with-components.toml
