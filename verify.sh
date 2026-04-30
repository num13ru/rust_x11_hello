#!/usr/bin/env bash
set -euo pipefail

docker run --rm \
  -v "$PWD":/work \
  -w /work \
  rust-kindle-armv7hf-builder \
  bash -lc 'arm-linux-gnueabihf-objdump -T kindle-extension/rust_hello/bin/rust_hello | grep GLIBC_ | sort -u'
