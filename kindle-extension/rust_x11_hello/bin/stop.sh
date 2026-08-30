#!/bin/sh

EXT_DIR="${RUST_X11_HELLO_EXT_DIR:-/mnt/us/extensions/rust_x11_hello}"
BIN="$EXT_DIR/bin/rust_x11_hello"
LOG="$EXT_DIR/rust_x11_hello.log"
PID_FILE="$EXT_DIR/rust_x11_hello.pid"
STATUS_FILE="$EXT_DIR/rust_x11_hello.status"

show_status() {
    DISPLAY_TEXT="$1"

    if command -v eips >/dev/null 2>&1; then
        eips 1 5 "Rust X11 Hello:"
        eips 1 7 "$DISPLAY_TEXT"
    fi
}

pid_is_expected() {
    CHECK_PID="$1"

    case "$CHECK_PID" in
        ''|*[!0-9]*) return 1 ;;
    esac

    [ -d "/proc/$CHECK_PID" ] || return 1

    EXE_PATH="$(readlink "/proc/$CHECK_PID/exe" 2>/dev/null)"
    [ "$EXE_PATH" = "$BIN" ] && return 0

    if [ -r "/proc/$CHECK_PID/cmdline" ]; then
        FIRST_ARG="$(tr '\000' '\n' < "/proc/$CHECK_PID/cmdline" 2>/dev/null | sed -n '1p')"
        [ "$FIRST_ARG" = "$BIN" ] && return 0
    fi

    return 1
}

if [ ! -f "$PID_FILE" ]; then
    show_status "Not running"
    exit 0
fi

RECORDED_PID="$(sed -n '1p' "$PID_FILE" 2>/dev/null | tr -d ' \t\r\n')"

if ! pid_is_expected "$RECORDED_PID"; then
    echo "Stop refused: PID file does not identify $BIN (value: ${RECORDED_PID:-empty})" >> "$LOG"
    printf '%s\n' "ERROR stop refused pid=${RECORDED_PID:-empty}" > "$STATUS_FILE"
    show_status "Stop refused: bad PID"
    exit 0
fi

if kill -TERM "$RECORDED_PID" 2>/dev/null; then
    echo "Stop requested with TERM for verified child PID $RECORDED_PID" >> "$LOG"
    printf '%s\n' "STOP REQUESTED pid=$RECORDED_PID" > "$STATUS_FILE"
    show_status "Stop requested: $RECORDED_PID"
else
    echo "Stop failed: could not signal verified child PID $RECORDED_PID" >> "$LOG"
    printf '%s\n' "ERROR stop signal failed pid=$RECORDED_PID" > "$STATUS_FILE"
    show_status "Stop signal failed"
fi

exit 0
