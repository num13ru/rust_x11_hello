#!/bin/sh

EXT_DIR="/mnt/us/extensions/rust_hello"
BIN="$EXT_DIR/bin/rust_hello"
LOG="$EXT_DIR/hello.log"

mkdir -p "$EXT_DIR"

{
    echo "========================================"
    echo "KUAL Rust Hello launcher"
    echo "Date: $(date)"
    echo "PWD: $(pwd)"
    echo "UID/GID: $(id 2>/dev/null || echo 'id unavailable')"
    echo "BIN: $BIN"
    echo

    if [ ! -f "$BIN" ]; then
        echo "ERROR: binary file does not exist: $BIN"
        echo "Did you copy kindle-extension/rust_hello to /mnt/us/extensions/rust_hello?"
        echo "========================================"
        exit 0
    fi

    chmod +x "$BIN" 2>/dev/null || true

    echo "---- running Rust binary ----"
    "$BIN"
    STATUS=$?

    echo
    echo "Rust binary exit status: $STATUS"
    echo "========================================"
} >> "$LOG" 2>&1

if command -v eips >/dev/null 2>&1; then
    eips 1 5 "Rust Hello done"
    eips 1 7 "$LOG"
fi

exit 0
