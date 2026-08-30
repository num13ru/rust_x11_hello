#!/bin/sh

EXT_DIR="${RUST_X11_HELLO_EXT_DIR:-/mnt/us/extensions/rust_x11_hello}"
STATUS_FILE="$EXT_DIR/rust_x11_hello.status"

if [ -r "$STATUS_FILE" ]; then
    STATUS_TEXT="$(sed -n '1p' "$STATUS_FILE")"
else
    STATUS_TEXT="no status yet"
fi

if command -v eips >/dev/null 2>&1; then
    eips 1 5 "Rust X11 Hello:"
    eips 1 7 "$STATUS_TEXT"
fi

exit 0
