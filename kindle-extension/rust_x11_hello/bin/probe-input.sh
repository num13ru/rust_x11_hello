#!/bin/sh

EXT_DIR="${PAPERPAD_EXT_DIR:-/mnt/us/extensions/rust_x11_hello}"
BIN="$EXT_DIR/bin/rust_x11_hello"
LOG="$EXT_DIR/rust_x11_hello.log"
MODE="${1:-inspect}"

case "$MODE" in
    inspect)
        PROBE_ARG="--inspect-input"
        PROBE_LABEL="read-only input capability probe"
        ;;
    trace)
        PROBE_ARG="--trace-input"
        PROBE_LABEL="bounded nonexclusive input event trace"
        ;;
    *)
        echo "ERROR: input probe mode must be inspect or trace, got: $MODE" >> "$LOG"
        exit 2
        ;;
esac

if [ ! -f "$BIN" ]; then
    echo "ERROR: input probe binary missing: $BIN" >> "$LOG"
    exit 1
fi

{
    echo "========================================"
    echo "PaperPad $PROBE_LABEL"
    echo "Date: $(date)"
    "$BIN" "$PROBE_ARG"
    PROBE_STATUS=$?
    echo "Input probe exit status: $PROBE_STATUS"
    echo "========================================"
} >> "$LOG" 2>&1

exit "$PROBE_STATUS"
