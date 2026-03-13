#!/bin/bash

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

if [[ -z "$1" ]]; then
  echo "Usage: ./run.sh <1-7>"
  echo ""
  echo "  1  Component         — invoke a single component"
  echo "  2  Composition       — compose two components"
  echo "  3  Configuration     — configure a component"
  echo "  4  Capability        — import a host capability"
  echo "  5  Cross-cutting     — apply an interceptor (advice)"
  echo "  6  Channel           — add HTTP gateway and messaging"
  echo "  7  Collaboration     — combine config files from collaborators"
  exit 1
fi

N="$1"

# Examples 4-7 need the translate API (node)
if [[ "$N" =~ ^[4-7]$ ]]; then
  if ! command -v node &>/dev/null; then
    echo "Error: node is required for examples 4-7 (runs translate-api.js)"
    exit 1
  fi
fi

check_port() {
  local port="$1"
  local label="$2"
  if curl -s -o /dev/null "http://localhost:${port}" 2>/dev/null; then
    echo "Error: port ${port} is already in use (needed for ${label})"
    exit 1
  fi
}

PIDS=()
cleanup() {
  # Kill in reverse order: gateway first (lets in-flight work finish
  # against still-running dependencies like translate API), then deps.
  for (( i=${#PIDS[@]}-1; i>=0; i-- )); do
    kill "${PIDS[i]}" 2>/dev/null || true
    wait "${PIDS[i]}" 2>/dev/null || true
  done
}
trap cleanup EXIT

start_translate_api() {
  check_port 8090 "translate API"
  node translate-api.js &
  PIDS+=($!)
  # Wait for the translate API to be ready
  for i in $(seq 1 30); do
    if curl -s -o /dev/null http://localhost:8090/translate 2>/dev/null; then
      break
    fi
    sleep 0.1
  done
}

run_gateway() {
  local port="$1"
  shift
  local configs=("$@")

  check_port "$port" "HTTP gateway"
  echo "Starting gateway on port ${port}..."
  RUST_LOG=info cargo run -q \
    --manifest-path ../../crates/gateway-http/Cargo.toml -- \
    "${configs[@]}" &
  PIDS+=($!)

  # Wait for the gateway to be ready
  for i in $(seq 1 30); do
    if curl -s -o /dev/null "http://localhost:${port}/hello" 2>/dev/null; then
      break
    fi
    sleep 0.2
  done

  echo ""
  echo "POST /hello with body 'World':"
  curl -s -X POST -H "Content-Type: text/plain" -d "World" "localhost:${port}/hello"
  echo ""
}

case "$N" in
  [1-3])
    CONFIG=$(ls configs/${N}-*.toml)
    composable invoke "$CONFIG" -- greeter.greet World
    ;;

  [4-5])
    start_translate_api
    CONFIG=$(ls configs/${N}-*.toml)
    composable invoke "$CONFIG" configs/infra.toml -- greeter.greet World
    ;;

  6)
    start_translate_api
    run_gateway 8080 configs/6-channel.toml configs/infra.toml
    ;;

  7)
    start_translate_api
    run_gateway 8080 \
      configs/7-collaboration-domain.toml \
      configs/7-collaboration-env.toml \
      configs/7-collaboration-ops.toml \
      configs/infra.toml
    ;;

  *)
    echo "Error: argument must be 1-7"
    exit 1
    ;;
esac
