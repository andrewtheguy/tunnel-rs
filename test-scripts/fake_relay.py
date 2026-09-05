#!/usr/bin/env python3
"""A relay that answers iroh's net-report probe but refuses relay connections.

Serves `GET /ping` with a fast 200 (what iroh's net report measures relay
latency with) and answers everything else, in particular the `/relay`
WebSocket upgrade, with 404. To iroh's net report this relay looks healthy and
stays the preferred home relay; to the relay actor it can never be connected.
That is the shape of outage the shared home-relay failover exists for, since
iroh's own re-homing only kicks in when the probe fails.

Usage: fake_relay.py --port PORT
"""
import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self):  # noqa: N802
        if self.path == "/ping":
            body = b"pong"
            self.send_response(200)
            self.send_header("Content-Type", "text/plain")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        self.send_response(404)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def log_message(self, fmt, *args):  # quiet
        pass


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, required=True)
    args = parser.parse_args()
    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    server.daemon_threads = True
    print(f"READY fake relay on 127.0.0.1:{args.port}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
