#!/bin/sh

EXT_DIR="${PAPERPAD_EXT_DIR:-/mnt/us/extensions/rust_x11_hello}"
BIN="$EXT_DIR/bin/rust_x11_hello"
LOG="$EXT_DIR/rust_x11_hello.log"

if [ ! -f "$BIN" ]; then
    echo "ERROR: framebuffer probe binary missing: $BIN" >> "$LOG"
    exit 1
fi

{
    echo "========================================"
    echo "PaperPad read-only framebuffer probe"
    echo "Date: $(date)"
    "$BIN" --inspect-framebuffer
    PROBE_STATUS=$?
    echo "Framebuffer probe exit status: $PROBE_STATUS"
    echo "========================================"
} >> "$LOG" 2>&1

exit "$PROBE_STATUS"
