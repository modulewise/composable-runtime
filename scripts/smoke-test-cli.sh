#!/usr/bin/env bash

# Smoke test the composable-runtime cli, run by the CI workflow with the cli unpacked from its
# archive, and by `make test-cli` with the cli installed with the tools.
#
#   scripts/smoke-test-cli.sh <cli>

set -euo pipefail

cli=$1

cd "$(dirname "$0")/.."

fail() {
  echo "::error::$1"
  exit 1
}

# without the carriage returns of windows
run() {
  "$cli" "$@" | tr -d '\r'
}

# TODO add tests to cover core behavior
run --help || fail "expected cli to report help"
