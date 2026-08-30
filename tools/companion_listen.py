#!/usr/bin/env python3
"""Companion listener for Kindle rust_x11_hello semantic activations.

Listens on a TCP port for the one-line protocol:
    event action=<semantic-id>
and prints each received action, validating it against the six known ids.

Defaults to 0.0.0.0:5581 (the USBNetwork companion port). For a Wi-Fi run,
bind explicitly and/or select a specific interface:
    python3 tools/companion_listen.py --host 0.0.0.0 --port 5581
"""

import argparse
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


def main(argv) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", default=HOST, help=f"bind address (default {HOST})")
    parser.add_argument("--port", type=int, default=PORT, help=f"bind port (default {PORT})")
    args = parser.parse_args(argv)

    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as srv:
        srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        srv.bind((args.host, args.port))
        srv.listen(1)
        print(f"listening on {args.host}:{args.port}", flush=True)
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
    sys.exit(main(sys.argv[1:]))
