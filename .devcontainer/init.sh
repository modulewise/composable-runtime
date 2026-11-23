#!/usr/bin/env bash

# Setup script run once for new devcontainers to init the environment.

set -euo pipefail

curl -L --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh | bash

echo "PATH=\"\$(make -s tools-path):\${PATH}\"" | tee -a ~/.zshrc ~/.bashrc >/dev/null
echo "source \"$(pwd)/build/scripts/completions.sh\"" | tee -a ~/.zshrc ~/.bashrc >/dev/null

make tools wit

cargo check
