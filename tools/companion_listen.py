#!/usr/bin/env python3
"""Companion listener for Kindle rust_x11_hello semantic activations.

Listens on the USBNetwork interface for the one-line protocol:
    event action=<semantic-id>
and prints each received action. Requires the Mac USBNetwork interface to be
up with the Kindle at 192.168.15.201 and this host routable to it.
"""

import socket
import sys

HOST = "0.0.0.0"
PORT = 5581

KNOWN_ACTIONS = {
    "media.play_pause",
    "media.next",
    "media.previous",
    "terminal.new_window",
    "tmux.work",
    "zoom.toggle_mute",
}


def main() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as srv:
        srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        srv.bind((HOST, PORT))
        srv.listen(1)
        print(f"listening on {HOST}:{PORT}", flush=True)
        while True:
            conn, addr = srv.accept()
            with conn:
                data = conn.recv(1024)
                if not data:
                    continue
                line = data.decode("utf-8", errors="replace").strip()
                print(f"received from {addr[0]}:{addr[1]}: {line}", flush=True)
                action = line.removeprefix("event action=").rstrip(";")
                if action in KNOWN_ACTIONS:
                    print(f"OK semantic action: {action}", flush=True)
                else:
                    print(f"UNKNOWN action: {action!r}", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
