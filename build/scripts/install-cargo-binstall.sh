#!/usr/bin/env bash

# Managed by componentized/build and synced into each repository by `make update-build`. Change it
# in https://github.com/componentized/build, `make check-build` fails when a copy is changed.

# Install the binary of a cargo-binstall release, which can't install itself the way it installs the
# other tools, see build/tools.mk.
#
#   build/scripts/install-cargo-binstall.sh <version> <root> <host>
#
# Downloads the release's archive for the host, a rust host tuple, e.g. aarch64-unknown-linux-gnu,
# and has the binary install itself in the root, `cargo-binstall --self-install`, with its record of
# the installed version alongside those of the tools it installs. Fails for a host the release has no
# archive for, the Makefile builds it with cargo install instead.

set -euo pipefail

version=$1
root=$2
host=$3

case "${host}" in
  *-apple-darwin | *-pc-windows-msvc) format=zip ;;
  *) format=tgz ;;
esac
archive="cargo-binstall-${host}.${format}"

# in the root, the binary moves itself into it, which fails across file systems, e.g. from /tmp
mkdir -p "${root}"
download=$(mktemp -d "${root}/.cargo-binstall.XXXXXX")
trap 'rm -rf "${download}"' EXIT

curl -fsSL --proto '=https' --tlsv1.2 --retry 3 -o "${download}/${archive}" \
  "https://github.com/cargo-bins/cargo-binstall/releases/download/v${version}/${archive}"
case "${format}" in
  zip) unzip -q "${download}/${archive}" -d "${download}" ;;
  tgz) tar xzf "${download}/${archive}" -C "${download}" ;;
esac

CARGO_INSTALL_ROOT=${root} "${download}/cargo-binstall" --self-install
