# tunnel-rs

Cross-platform secure peer-to-peer TCP/UDP port forwarding with NAT traversal.

`tunnel-rs` forwards specific TCP or UDP services between peers without opening inbound ports.

> [!WARNING]
> Pre-1.0 project: compatibility between minor versions is not guaranteed.

## Binaries

- `tunnel-rs`: iroh-based port forwarding
- `tunnel-rs-ice`: manual and nostr signaling modes

## Quick Start

Generate a persistent server key and auth token:

```bash
tunnel-rs generate-server-key --output ./server.key
AUTH_TOKEN=$(tunnel-rs generate-token)
```

Start server:

```bash
tunnel-rs server \
  --secret-file ./server.key \
  --allowed-tcp 127.0.0.0/8 \
  --auth-tokens "$AUTH_TOKEN"
```

Start client:

```bash
tunnel-rs client \
  --server-node-id <SERVER_NODE_ID> \
  --source tcp://127.0.0.1:22 \
  --target 127.0.0.1:2222 \
  --auth-token "$AUTH_TOKEN"
```

## Installation

Linux/macOS:

```bash
curl -sSL https://andrewtheguy.github.io/tunnel-rs/install.sh | bash
```

Windows:

```powershell
irm https://andrewtheguy.github.io/tunnel-rs/install.ps1 | iex
```

## From Source

```bash
cargo install --path . -p tunnel-rs
cargo install --path . -p tunnel-rs-ice
```

## Documentation

- `docs/ARCHITECTURE.md`
- `docs/ARCHITECTURE-PORT-FORWARDING.md`
- `docs/ALTERNATIVE-MODES.md`
- `docs/SELF-HOSTING.md`
