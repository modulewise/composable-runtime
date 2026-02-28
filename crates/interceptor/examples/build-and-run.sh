#!/bin/sh

set -euo pipefail

EXAMPLES_DIR="$(cd "$(dirname "$0")" && pwd)"

echo "Fetching WIT dependencies..."
wkg wit fetch

for example in "$EXAMPLES_DIR"/*/; do
    name="$(basename "$example")"

    if [ ! -f "$example/build.sh" ]; then
        continue
    fi

    echo "========================================"
    echo "Example: $name"
    echo "========================================"

    (cd "$example" && sh build.sh)
    (cd "$example" && sh run.sh)

    echo ""
done
