#!/usr/bin/env bash
set -euo pipefail

BIN="${1:-kindle-extension/rust_x11_hello/bin/rust_x11_hello}"

if [ ! -f "$BIN" ]; then
    echo "ERROR: binary not found: $BIN"
    exit 1
fi

for cmd in file readelf arm-linux-gnueabihf-objdump; do
    if ! command -v "$cmd" >/dev/null 2>&1; then
        echo "ERROR: required command not found: $cmd"
        echo
        echo "Run this check inside the Docker builder:"
        echo "  ./verify.sh"
        exit 1
    fi
done

echo "Binary:"
file "$BIN"
echo

echo "Interpreter check:"
INTERPRETER_OUTPUT="$(readelf -l "$BIN" | grep interpreter || true)"

if [ -n "$INTERPRETER_OUTPUT" ]; then
    echo "ERROR: binary has dynamic interpreter"
    echo "$INTERPRETER_OUTPUT"
    exit 1
fi

echo "OK: no dynamic interpreter"
echo

echo "GLIBC symbols check:"
OBJDUMP_ERR="$(mktemp)"
OBJDUMP_OUT="$(mktemp)"

if arm-linux-gnueabihf-objdump -T "$BIN" >"$OBJDUMP_OUT" 2>"$OBJDUMP_ERR"; then
    if grep -q "GLIBC_" "$OBJDUMP_OUT"; then
        echo "ERROR: GLIBC symbols found"
        grep "GLIBC_" "$OBJDUMP_OUT" | sort -u
        rm -f "$OBJDUMP_ERR" "$OBJDUMP_OUT"
        exit 1
    fi

    echo "OK: no GLIBC symbols found"
else
    ERR_CONTENT="$(cat "$OBJDUMP_ERR")"

    if echo "$ERR_CONTENT" | grep -q "not a dynamic object"; then
        echo "OK: not a dynamic object"
    else
        echo "ERROR: objdump failed"
        echo "$ERR_CONTENT"
        rm -f "$OBJDUMP_ERR" "$OBJDUMP_OUT"
        exit 1
    fi
fi

rm -f "$OBJDUMP_ERR" "$OBJDUMP_OUT"

echo
echo "OK: static binary, no dynamic interpreter, no GLIBC symbols"
