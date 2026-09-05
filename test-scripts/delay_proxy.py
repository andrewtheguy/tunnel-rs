#!/usr/bin/env python3
"""A TCP proxy that delays every new connection before forwarding it.

Used to make one relay measurably slower than the other in iroh's net report,
which probes relay latency over a fresh HTTP connection each time. Established
connections (the relay WebSocket) are forwarded byte for byte with no further
delay, so the relay behind the proxy works normally; it is just never the
preferred one while a faster relay answers.

Usage: delay_proxy.py --listen PORT --upstream PORT --delay-ms MS
"""
import argparse
import asyncio


async def pipe(reader, writer):
    try:
        while True:
            data = await reader.read(65536)
            if not data:
                break
            writer.write(data)
            await writer.drain()
    except (ConnectionError, asyncio.CancelledError):
        pass
    finally:
        try:
            writer.close()
        except Exception:
            pass


async def handle(client_reader, client_writer, upstream_port, delay):
    await asyncio.sleep(delay)
    try:
        upstream_reader, upstream_writer = await asyncio.open_connection(
            "127.0.0.1", upstream_port
        )
    except OSError:
        client_writer.close()
        return
    await asyncio.gather(
        pipe(client_reader, upstream_writer),
        pipe(upstream_reader, client_writer),
    )


async def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--listen", type=int, required=True)
    parser.add_argument("--upstream", type=int, required=True)
    parser.add_argument("--delay-ms", type=int, required=True)
    args = parser.parse_args()
    delay = args.delay_ms / 1000

    server = await asyncio.start_server(
        lambda r, w: handle(r, w, args.upstream, delay), "127.0.0.1", args.listen
    )
    print(
        f"READY delay proxy 127.0.0.1:{args.listen} -> 127.0.0.1:{args.upstream} (+{args.delay_ms}ms)",
        flush=True,
    )
    async with server:
        await server.serve_forever()


if __name__ == "__main__":
    asyncio.run(main())
