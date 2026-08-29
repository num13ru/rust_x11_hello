#!/bin/sh

LOG="/mnt/us/extensions/rust_x11_hello/rust_x11_hello.log"

if command -v eips >/dev/null 2>&1; then
    eips 1 5 "Rust X11 Hello log:"
    eips 1 7 "$(tail -n 1 "$LOG" 2>/dev/null || echo 'no log yet')"
fi

exit 0
