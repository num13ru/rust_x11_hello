#!/usr/bin/env bash
set -euo pipefail

TARGET="armv7-unknown-linux-gnueabihf"
BIN_NAME="rust_hello"
EXT_DIR="kindle-extension/rust_hello"
OUT_BIN="target/${TARGET}/release/${BIN_NAME}"

rustup target add "${TARGET}"

cargo build --release --target "${TARGET}"

mkdir -p "${EXT_DIR}/bin"

cp "${OUT_BIN}" "${EXT_DIR}/bin/${BIN_NAME}"
chmod +x "${EXT_DIR}/bin/${BIN_NAME}"
chmod +x "${EXT_DIR}/bin/"*.sh 2>/dev/null || true

echo
echo "Built Kindle extension:"
echo "${EXT_DIR}"
echo

find "${EXT_DIR}" -maxdepth 3 -type f -print

echo
echo "Binary info:"
file "${EXT_DIR}/bin/${BIN_NAME}" || true

echo
echo "Interpreter:"
if command -v arm-linux-gnueabihf-readelf >/dev/null 2>&1; then
    arm-linux-gnueabihf-readelf -l "${EXT_DIR}/bin/${BIN_NAME}" | grep interpreter || true
elif command -v readelf >/dev/null 2>&1; then
    readelf -l "${EXT_DIR}/bin/${BIN_NAME}" | grep interpreter || true
else
    echo "readelf not found"
fi
