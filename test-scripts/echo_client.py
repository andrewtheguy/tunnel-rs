# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Echo test client for tunnel-rs E2E tests.

Connects to a host:port (the local port exposed by the tunnel-rs client),
sends a message, reads the echoed reply back, and verifies it matches.
Exits 0 on success, 1 on mismatch/timeout.

Run via uv:
    uv run echo_client.py --proto tcp --host 127.0.0.1 --port 2222 --message hello
"""

from __future__ import annotations

import argparse
import socket
import sys


def log(msg: str) -> None:
    print(msg, file=sys.stderr, flush=True)


def run_tcp(host: str, port: int, payload: bytes, timeout: float) -> bool:
    with socket.create_connection((host, port), timeout=timeout) as sock:
        sock.settimeout(timeout)
        sock.sendall(payload)
        received = bytearray()
        while len(received) < len(payload):
            chunk = sock.recv(len(payload) - len(received))
            if not chunk:
                break
            received.extend(chunk)
    return bytes(received) == payload


def run_udp(host: str, port: int, payload: bytes, timeout: float, retries: int) -> bool:
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.settimeout(timeout)
    try:
        # UDP has no handshake; the first datagram can be dropped while the
        # tunnel path is still settling, so retry a few times before failing.
        for attempt in range(retries):
            sock.sendto(payload, (host, port))
            try:
                data, _ = sock.recvfrom(65536)
            except socket.timeout:
                log(f"udp attempt {attempt + 1}/{retries} timed out, retrying")
                continue
            return data == payload
        return False
    finally:
        sock.close()


def main() -> int:
    parser = argparse.ArgumentParser(description="TCP/UDP echo test client")
    parser.add_argument("--proto", choices=["tcp", "udp"], required=True)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--message", default="tunnel-rs-e2e-hello")
    parser.add_argument("--timeout", type=float, default=5.0)
    parser.add_argument("--udp-retries", type=int, default=5)
    args = parser.parse_args()

    payload = args.message.encode()

    try:
        if args.proto == "tcp":
            ok = run_tcp(args.host, args.port, payload, args.timeout)
        else:
            ok = run_udp(args.host, args.port, payload, args.timeout, args.udp_retries)
    except (OSError, socket.timeout) as exc:
        log(f"FAIL {args.proto} {args.host}:{args.port} error: {exc}")
        return 1

    if ok:
        log(f"OK {args.proto} {args.host}:{args.port} echoed {len(payload)} bytes")
        return 0
    log(f"FAIL {args.proto} {args.host}:{args.port} echo mismatch")
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
