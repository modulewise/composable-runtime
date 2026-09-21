#!/bin/bash

cd "$(dirname "$0")"

# `-v` drops the storage volume, so pushed components do not survive a restart.
docker compose down -v
