#!/bin/bash

set -e

echo "--- GET /hello/world (component route) ---"
curl -s localhost:8888/hello/World; echo

echo "--- POST /bonjour (channel route -> subscription -> bonjour component) ---"
curl -s -X POST -H "Content-Type: text/plain" -d "le Monde" localhost:8888/bonjour; echo
