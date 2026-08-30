#!/bin/sh

EXT_DIR="${RUST_X11_HELLO_EXT_DIR:-/mnt/us/extensions/rust_x11_hello}"
BIN="$EXT_DIR/bin/rust_x11_hello"
LOG="$EXT_DIR/rust_x11_hello.log"
PID_FILE="$EXT_DIR/rust_x11_hello.pid"
STATUS_FILE="$EXT_DIR/rust_x11_hello.status"
WATCHDOG_MARKER="$EXT_DIR/rust_x11_hello.watchdog"
LOCK_DIR="$EXT_DIR/rust_x11_hello.lock"
LOCK_OWNER_FILE="$LOCK_DIR/launcher.pid"
WATCHDOG_SECONDS="${RUST_X11_HELLO_WATCHDOG_SECONDS:-90}"
WATCHDOG_TERM_GRACE_SECONDS="${RUST_X11_HELLO_WATCHDOG_TERM_GRACE_SECONDS:-5}"
COMPANION_HOST="${RUST_X11_HELLO_COMPANION:-192.168.15.201}"

CHILD_PID=""
WATCHDOG_PID=""
LOCK_HELD=0

mkdir -p "$EXT_DIR"

case "$WATCHDOG_SECONDS" in
    ''|0|*[!0-9]*) WATCHDOG_SECONDS=90 ;;
esac
case "$WATCHDOG_TERM_GRACE_SECONDS" in
    ''|0|*[!0-9]*) WATCHDOG_TERM_GRACE_SECONDS=5 ;;
esac

read_pid_file() {
    sed -n '1p' "$PID_FILE" 2>/dev/null | tr -d ' \t\r\n'
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

write_status() {
    STATUS_TEXT="$1"
    STATUS_TMP="$STATUS_FILE.tmp.$$"

    if printf '%s\n' "$STATUS_TEXT" > "$STATUS_TMP"; then
        mv -f "$STATUS_TMP" "$STATUS_FILE"
    else
        rm -f "$STATUS_TMP"
    fi
}

release_lock() {
    if [ "$LOCK_HELD" -ne 1 ]; then
        return
    fi

    LOCK_OWNER="$(sed -n '1p' "$LOCK_OWNER_FILE" 2>/dev/null | tr -d ' \t\r\n')"
    if [ "$LOCK_OWNER" = "$$" ]; then
        rm -f "$LOCK_OWNER_FILE"
        rmdir "$LOCK_DIR" 2>/dev/null || true
    else
        echo "WARNING: launcher lock owner changed; leaving lock untouched" >> "$LOG"
    fi
    LOCK_HELD=0
}

acquire_lock() {
    if mkdir "$LOCK_DIR" 2>/dev/null; then
        if printf '%s\n' "$$" > "$LOCK_OWNER_FILE"; then
            LOCK_HELD=1
            return 0
        fi

        rmdir "$LOCK_DIR" 2>/dev/null || true
        return 1
    fi

    LOCK_OWNER="$(sed -n '1p' "$LOCK_OWNER_FILE" 2>/dev/null | tr -d ' \t\r\n')"
    case "$LOCK_OWNER" in
        ''|*[!0-9]*)
            echo "ERROR: launcher lock exists without a verifiable owner; refusing launch" >> "$LOG"
            return 1
            ;;
    esac

    if kill -0 "$LOCK_OWNER" 2>/dev/null; then
        echo "ERROR: launcher already active with PID $LOCK_OWNER" >> "$LOG"
        return 1
    fi

    echo "Removing stale launcher lock owned by dead PID $LOCK_OWNER" >> "$LOG"
    rm -f "$LOCK_OWNER_FILE"
    rmdir "$LOCK_DIR" 2>/dev/null || return 1

    if mkdir "$LOCK_DIR" 2>/dev/null; then
        if printf '%s\n' "$$" > "$LOCK_OWNER_FILE"; then
            LOCK_HELD=1
            return 0
        fi
        rmdir "$LOCK_DIR" 2>/dev/null || true
    fi

    return 1
}

show_status() {
    DISPLAY_TEXT="$1"

    if command -v eips >/dev/null 2>&1; then
        eips 1 5 "Rust X11 Hello:"
        eips 1 7 "$DISPLAY_TEXT"
    fi
}

forward_signal() {
    SIGNAL_NAME="$1"

    if [ -n "$CHILD_PID" ] && pid_is_expected "$CHILD_PID"; then
        echo "Launcher received $SIGNAL_NAME; forwarding to child PID $CHILD_PID" >> "$LOG"
        kill -"$SIGNAL_NAME" "$CHILD_PID" 2>/dev/null || true
    fi
}

trap 'forward_signal HUP' HUP
trap 'forward_signal INT' INT
trap 'forward_signal TERM' TERM

{
    echo "========================================"
    echo "KUAL Rust X11 Hello launcher"
    echo "Date: $(date)"
    echo "PWD: $(pwd)"
    echo "UID/GID: $(id 2>/dev/null || echo 'id unavailable')"
    echo "BIN: $BIN"
    echo "Watchdog: ${WATCHDOG_SECONDS}s plus ${WATCHDOG_TERM_GRACE_SECONDS}s TERM grace"
} >> "$LOG" 2>&1

if [ ! -f "$BIN" ]; then
    echo "ERROR: binary file does not exist: $BIN" >> "$LOG"
    echo "Install the MTP extension at /extensions/rust_x11_hello" >> "$LOG"
    echo "========================================" >> "$LOG"
    write_status "ERROR missing binary"
    show_status "ERROR missing binary"
    exit 0
fi

if ! acquire_lock; then
    echo "========================================" >> "$LOG"
    write_status "ERROR launcher lock unavailable"
    show_status "Launch already active"
    exit 0
fi

if [ -f "$PID_FILE" ]; then
    RECORDED_PID="$(read_pid_file)"
    if pid_is_expected "$RECORDED_PID"; then
        echo "ERROR: expected process already running with PID $RECORDED_PID" >> "$LOG"
        echo "========================================" >> "$LOG"
        write_status "ALREADY RUNNING pid=$RECORDED_PID"
        show_status "Already running: $RECORDED_PID"
        release_lock
        exit 0
    fi

    case "$RECORDED_PID" in
        ''|*[!0-9]*) PID_IS_LIVE=0 ;;
        *)
            if kill -0 "$RECORDED_PID" 2>/dev/null; then
                PID_IS_LIVE=1
            else
                PID_IS_LIVE=0
            fi
            ;;
    esac

    if [ "$PID_IS_LIVE" -eq 1 ]; then
        echo "ERROR: PID $RECORDED_PID is live but its executable cannot be verified; refusing duplicate launch" >> "$LOG"
        echo "========================================" >> "$LOG"
        write_status "ERROR ambiguous live PID=$RECORDED_PID"
        show_status "Refused: ambiguous PID"
        release_lock
        exit 0
    fi

    echo "Removing stale PID file (recorded value: ${RECORDED_PID:-empty})" >> "$LOG"
    if ! rm -f "$PID_FILE"; then
        echo "ERROR: cannot remove stale PID file: $PID_FILE" >> "$LOG"
        echo "========================================" >> "$LOG"
        write_status "ERROR stale PID file"
        show_status "ERROR stale PID file"
        release_lock
        exit 0
    fi
fi

rm -f "$WATCHDOG_MARKER"

if ! chmod +x "$BIN" 2>> "$LOG"; then
    echo "WARNING: chmod +x failed; attempting launch with existing mode" >> "$LOG"
fi
echo "Companion host: $COMPANION_HOST" >> "$LOG"
echo "---- running Rust binary ----" >> "$LOG"
RUST_X11_HELLO_COMPANION="$COMPANION_HOST" "$BIN" >> "$LOG" 2>&1 &
CHILD_PID=$!

if ! printf '%s\n' "$CHILD_PID" > "$PID_FILE"; then
    echo "ERROR: cannot write PID file; terminating child PID $CHILD_PID" >> "$LOG"
    if pid_is_expected "$CHILD_PID"; then
        kill -TERM "$CHILD_PID" 2>/dev/null || true
    fi
    wait "$CHILD_PID" 2>/dev/null || true
    write_status "ERROR cannot write PID file"
    show_status "ERROR cannot write PID"
    release_lock
    exit 0
fi

echo "Child PID: $CHILD_PID" >> "$LOG"
write_status "RUNNING pid=$CHILD_PID watchdog=${WATCHDOG_SECONDS}s"

(
    sleep "$WATCHDOG_SECONDS"

    RECORDED_PID="$(read_pid_file)"
    if [ "$RECORDED_PID" = "$CHILD_PID" ] && pid_is_expected "$CHILD_PID"; then
        printf '%s\n' "$CHILD_PID" > "$WATCHDOG_MARKER"
        echo "Watchdog deadline reached; sending TERM to child PID $CHILD_PID" >> "$LOG"
        kill -TERM "$CHILD_PID" 2>/dev/null || true
        sleep "$WATCHDOG_TERM_GRACE_SECONDS"

        RECORDED_PID="$(read_pid_file)"
        if [ "$RECORDED_PID" = "$CHILD_PID" ] && pid_is_expected "$CHILD_PID"; then
            echo "Watchdog TERM grace expired; sending KILL to verified child PID $CHILD_PID" >> "$LOG"
            kill -KILL "$CHILD_PID" 2>/dev/null || true
        fi
    fi
) &
WATCHDOG_PID=$!

while :; do
    wait "$CHILD_PID" 2>> "$LOG"
    CHILD_STATUS=$?

    if ! kill -0 "$CHILD_PID" 2>/dev/null; then
        break
    fi
done

if [ -n "$WATCHDOG_PID" ]; then
    kill -TERM "$WATCHDOG_PID" 2>/dev/null || true
    wait "$WATCHDOG_PID" 2>/dev/null || true
fi

RECORDED_PID="$(read_pid_file)"
if [ "$RECORDED_PID" = "$CHILD_PID" ]; then
    rm -f "$PID_FILE"
else
    echo "WARNING: PID file changed before cleanup; leaving it untouched" >> "$LOG"
fi

if [ "$(sed -n '1p' "$WATCHDOG_MARKER" 2>/dev/null)" = "$CHILD_PID" ]; then
    END_REASON="watchdog"
else
    END_REASON="process_exit"
fi
rm -f "$WATCHDOG_MARKER"

{
    echo
    echo "Rust binary exit status: $CHILD_STATUS"
    echo "End reason: $END_REASON"
    echo "Ended: $(date)"
    echo "========================================"
} >> "$LOG" 2>&1

write_status "STOPPED status=$CHILD_STATUS reason=$END_REASON"
show_status "Stopped: $CHILD_STATUS ($END_REASON)"
release_lock
exit 0
