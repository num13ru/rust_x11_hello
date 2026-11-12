#!/usr/bin/env python3
"""Restricted HTTP CONNECT proxy for Apple Container guests behind a host VPN."""

from __future__ import annotations

import argparse
import ipaddress
import json
import os
import select
import socket
import socketserver
import subprocess
import sys
import threading
from urllib.parse import urlsplit


MAX_HEADER_BYTES = 64 * 1024
BUFFER_BYTES = 64 * 1024
CONNECT_TIMEOUT_SECONDS = 30
ALLOWED_DESTINATION_PORTS = {80, 443}


def discover_network(name: str) -> tuple[str, ipaddress.IPv4Network]:
    output = subprocess.check_output(
        ["container", "network", "list", "--format", "json"],
        text=True,
    )
    networks = json.loads(output)
    for network in networks:
        configuration = network.get("configuration", {})
        if network.get("id") != name and configuration.get("name") != name:
            continue
        status = network.get("status", {})
        gateway = ipaddress.ip_address(status["ipv4Gateway"])
        subnet = ipaddress.ip_network(status["ipv4Subnet"])
        if not isinstance(gateway, ipaddress.IPv4Address) or not isinstance(
            subnet, ipaddress.IPv4Network
        ):
            raise ValueError(f"Apple Container network {name!r} is not IPv4")
        if not subnet.is_private or gateway not in subnet:
            raise ValueError(f"Apple Container network {name!r} is not a private subnet")
        return str(gateway), subnet
    raise ValueError(f"Apple Container network {name!r} was not found")


def split_authority(authority: str, default_port: int) -> tuple[str, int]:
    parsed = urlsplit(f"//{authority}")
    if parsed.hostname is None:
        raise ValueError("missing destination host")
    try:
        port = parsed.port or default_port
    except ValueError as error:
        raise ValueError("invalid destination port") from error
    if port not in ALLOWED_DESTINATION_PORTS:
        raise ValueError(f"destination port {port} is not allowed")
    return parsed.hostname, port


def connect_public(host: str, port: int) -> socket.socket:
    last_error: OSError | None = None
    addresses = socket.getaddrinfo(
        host,
        port,
        family=socket.AF_INET,
        type=socket.SOCK_STREAM,
    )
    for family, socktype, protocol, _, sockaddr in addresses:
        address = ipaddress.ip_address(sockaddr[0])
        if not address.is_global:
            continue
        upstream = socket.socket(family, socktype, protocol)
        upstream.settimeout(CONNECT_TIMEOUT_SECONDS)
        try:
            upstream.connect(sockaddr)
            upstream.settimeout(None)
            return upstream
        except OSError as error:
            last_error = error
            upstream.close()
    if last_error is not None:
        raise last_error
    raise OSError(f"{host} did not resolve to a public address")


def relay(left: socket.socket, right: socket.socket) -> None:
    sockets = [left, right]
    while sockets:
        readable, _, _ = select.select(sockets, [], [])
        for source in readable:
            target = right if source is left else left
            data = source.recv(BUFFER_BYTES)
            if not data:
                sockets.remove(source)
                try:
                    target.shutdown(socket.SHUT_WR)
                except OSError:
                    pass
                continue
            target.sendall(data)


def read_request(client: socket.socket) -> tuple[bytes, bytes]:
    request = bytearray()
    while b"\r\n\r\n" not in request:
        chunk = client.recv(BUFFER_BYTES)
        if not chunk:
            raise ValueError("client closed before sending HTTP headers")
        request.extend(chunk)
        if len(request) > MAX_HEADER_BYTES:
            raise ValueError("HTTP headers exceed size limit")
    header_end = request.index(b"\r\n\r\n") + 4
    return bytes(request[:header_end]), bytes(request[header_end:])


def rewrite_http_request(header: bytes) -> tuple[str, int, bytes]:
    lines = header.decode("iso-8859-1").split("\r\n")
    method, target, version = lines[0].split(" ", 2)
    parsed = urlsplit(target)
    if parsed.scheme.lower() != "http" or parsed.hostname is None:
        raise ValueError("non-CONNECT requests must use an absolute HTTP URL")
    port = parsed.port or 80
    if port not in ALLOWED_DESTINATION_PORTS:
        raise ValueError(f"destination port {port} is not allowed")
    origin_target = parsed.path or "/"
    if parsed.query:
        origin_target += f"?{parsed.query}"

    forwarded_headers = [f"{method} {origin_target} {version}"]
    for line in lines[1:]:
        if not line:
            continue
        name = line.partition(":")[0].strip().lower()
        if name not in {"connection", "proxy-authorization", "proxy-connection"}:
            forwarded_headers.append(line)
    forwarded_headers.extend(("Connection: close", "", ""))
    rewritten = "\r\n".join(forwarded_headers).encode("iso-8859-1")
    return parsed.hostname, port, rewritten


class ProxyHandler(socketserver.BaseRequestHandler):
    server: "ProxyServer"

    def handle(self) -> None:
        upstream: socket.socket | None = None
        stream_started = False
        try:
            header, buffered_body = read_request(self.request)
            request_line = header.split(b"\r\n", 1)[0].decode("iso-8859-1")
            method, target, _ = request_line.split(" ", 2)
            if method.upper() == "CONNECT":
                host, port = split_authority(target, 443)
                upstream = connect_public(host, port)
                self.request.sendall(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                stream_started = True
            else:
                host, port, rewritten = rewrite_http_request(header)
                upstream = connect_public(host, port)
                upstream.sendall(rewritten)
                if buffered_body:
                    upstream.sendall(buffered_body)
                stream_started = True

            print(f"proxy: {self.client_address[0]} -> {host}:{port}", flush=True)
            relay(self.request, upstream)
        except (OSError, UnicodeError, ValueError) as error:
            outcome = "closed" if stream_started else "rejected"
            print(
                f"proxy: {outcome} {self.client_address[0]}: {error}",
                file=sys.stderr,
                flush=True,
            )
            if not stream_started:
                try:
                    self.request.sendall(
                        b"HTTP/1.1 502 Bad Gateway\r\n"
                        b"Connection: close\r\n"
                        b"Content-Length: 0\r\n\r\n"
                    )
                except OSError:
                    pass
        finally:
            if upstream is not None:
                upstream.close()


class ProxyServer(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True
    request_queue_size = 128

    def verify_request(self, request: socket.socket, client_address: tuple[str, int]) -> bool:
        del request
        return ipaddress.ip_address(client_address[0]) in self.allowed_clients


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--network",
        default="default",
        help="Apple Container network to discover",
    )
    parser.add_argument(
        "command",
        nargs=argparse.REMAINDER,
        help="optional command to run while the proxy is active",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    gateway, subnet = discover_network(args.network)
    with ProxyServer((gateway, 0), ProxyHandler) as server:
        server.allowed_clients = subnet
        _, port = server.server_address
        proxy_url = f"http://{gateway}:{port}"
        print(
            f"proxy: listening on discovered network {args.network!r}; Ctrl-C to stop",
            flush=True,
        )
        if command:
            server_thread = threading.Thread(target=server.serve_forever, daemon=True)
            server_thread.start()
            try:
                command_environment = os.environ.copy()
                command_environment["CONTAINER_PROXY"] = proxy_url
                command_environment["http_proxy"] = proxy_url
                command_environment["https_proxy"] = proxy_url
                result = subprocess.run(command, check=False, env=command_environment)
            finally:
                server.shutdown()
                server_thread.join()
            return result.returncode
        try:
            server.serve_forever()
        except KeyboardInterrupt:
            pass
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
