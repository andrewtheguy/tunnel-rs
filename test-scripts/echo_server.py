# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Minimal TCP/UDP echo server used as the backend service for tunnel-rs E2E tests.

It binds a single host:port and echoes back whatever it receives. When the socket
is bound it prints a `READY <proto> <host>:<port>` line to stderr so an orchestrator
can wait for it deterministically instead of sleeping.

Run via uv:
    uv run echo_server.py --proto tcp --host 127.0.0.1 --port 9000
    uv run echo_server.py --proto udp --host 127.0.0.1 --port 9001
"""

from __future__ import annotations

import argparse
import signal
import socket
import sys
import threading


def log(msg: str) -> None:
    print(msg, file=sys.stderr, flush=True)


def serve_tcp(host: str, port: int) -> None:
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind((host, port))
    srv.listen(128)
    log(f"READY tcp {host}:{port}")

    def handle(conn: socket.socket, peer) -> None:
        with conn:
            while True:
                data = conn.recv(65536)
                if not data:
                    break
                conn.sendall(data)

    while True:
        conn, peer = srv.accept()
        threading.Thread(target=handle, args=(conn, peer), daemon=True).start()


def serve_udp(host: str, port: int) -> None:
    srv = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind((host, port))
    log(f"READY udp {host}:{port}")

    while True:
        data, peer = srv.recvfrom(65536)
        srv.sendto(data, peer)


def main() -> int:
    parser = argparse.ArgumentParser(description="TCP/UDP echo server")
    parser.add_argument("--proto", choices=["tcp", "udp"], required=True)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, required=True)
    args = parser.parse_args()

    # Exit cleanly on SIGTERM so the orchestrator's teardown is quiet.
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))

    try:
        if args.proto == "tcp":
            serve_tcp(args.host, args.port)
        else:
            serve_udp(args.host, args.port)
    except KeyboardInterrupt:
        return 0
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
