#!/bin/bash

set -e

echo "--- GET /hello/World (component route) ---"
curl -s localhost:8888/hello/world; echo

echo "--- POST /bonjour (channel route -> subscription -> bonjour component) ---"
curl -s -X POST -H "Content-Type: text/plain" -d "le monde" localhost:8888/bonjour; echo
