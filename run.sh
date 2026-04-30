#!/usr/bin/env bash
set -euo pipefail

docker run --rm \
  -v "$PWD":/work \
  -w /work \
  rust-kindle-armv7hf-builder
