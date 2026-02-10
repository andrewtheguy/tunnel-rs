# tunnel-rs Architecture

This document summarizes the port-forwarding architecture.

## Binaries

- `tunnel-rs`: iroh signaling and transport for port forwarding
- `tunnel-rs-ice`: manual and nostr signaling variants

## Core crates

- `tunnel-common`: config, signaling codec, network helpers
- `tunnel-iroh`: iroh mode implementation
- `tunnel-ice`: manual and nostr mode implementation

## Flow

1. Client discovers and connects to the server.
2. Client authenticates with a token.
3. Client requests a source endpoint (`tcp://` or `udp://`).
4. Server validates policy and opens forwarding streams.
5. Data is proxied between local socket and QUIC streams.

## Modes

- `iroh`: best connectivity and relay fallback
- `manual`: copy/paste signaling
- `nostr`: nostr relay signaling with static keys
