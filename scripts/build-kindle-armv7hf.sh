#!/usr/bin/env bash
set -euo pipefail

TARGET="armv7-unknown-linux-musleabihf"
BIN_NAME="rust_x11_hello"
EXT_DIR="kindle-extension/rust_x11_hello"
DEPLOY_BIN="${EXT_DIR}/bin/${BIN_NAME}"

target_libdir=$(rustc --print target-libdir --target "$TARGET")
shopt -s nullglob
standard_libraries=("$target_libdir"/libstd-*.rlib)
if (( ${#standard_libraries[@]} == 0 )); then
    printf 'ERROR: Rust standard library for %s is missing from %s. Use nix develop or rebuild the container image.\n' \
        "$TARGET" "$target_libdir" >&2
    exit 1
fi

# Cargo reports the actual executable path, including custom target directories.
cargo_output=$(cargo build --locked --release --target "$TARGET" --package "$BIN_NAME" --bin "$BIN_NAME" \
    --message-format=json-render-diagnostics)
built_bin=$(jq --exit-status --raw-output --slurp --arg name "$BIN_NAME" '
        map(select(.reason == "compiler-artifact" and .target.name == $name and
                   .target.kind == ["bin"] and .executable != null))
        | if length == 1 then .[0].executable else error("Expected one Kindle executable from Cargo") end
    ' <<<"$cargo_output")

mkdir -p "${EXT_DIR}/bin"
if [[ -d "$DEPLOY_BIN" ]]; then
    printf 'ERROR: Kindle artifact path is a directory: %s\n' "$DEPLOY_BIN" >&2
    exit 1
fi
staged_bin=$(mktemp "${EXT_DIR}/bin/.${BIN_NAME}.XXXXXX")
trap 'rm -f "$staged_bin"' EXIT
trap 'exit 1' HUP INT TERM
cp "$built_bin" "$staged_bin"
chmod 755 "$staged_bin"
bash "$(dirname "${BASH_SOURCE[0]}")/check-static.sh" "$staged_bin"
file -b "$staged_bin"
mv -f "$staged_bin" "$DEPLOY_BIN"
printf 'Built Kindle extension binary: %s\n' "$DEPLOY_BIN"
