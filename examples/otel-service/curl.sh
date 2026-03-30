#!/bin/bash

echo "=== no traceparent (host generates root trace) ==="
curl http://localhost:8080/test

echo ""
echo "=== traceparent (upstream parent of host span) ==="
curl -H "traceparent: 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01" http://localhost:8080/test
