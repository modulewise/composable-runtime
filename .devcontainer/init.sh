#!/usr/bin/env bash

# Setup script run once for new devcontainers to init the environment.

set -euo pipefail

echo "PATH=\"\$(make -s tools-path):\${PATH}\"" | tee -a ~/.zshrc ~/.bashrc >/dev/null
echo "source \"$(pwd)/build/scripts/completions.sh\"" | tee -a ~/.zshrc ~/.bashrc >/dev/null

make tools wit

cargo check
