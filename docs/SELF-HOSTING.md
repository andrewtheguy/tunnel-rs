# Self-Hosting Iroh Infrastructure

This document covers how to self-host iroh's relay server for fully independent operation in port forwarding mode (`tunnel-rs`).

## Peer Discovery

Peer discovery is **not configurable** — tunnel-rs picks the right behavior automatically based on the relay in use:

- **Default relays** (no `--relay-url`): the default iroh discovery server is used (pkarr publishing + DNS-based lookup). The server (persistent identity) publishes its address; the client (ephemeral identity) only resolves.
- **Custom relay** (`--relay-url`): discovery is **disabled automatically**. A custom relay doubles as the rendezvous point, so the discovery server is unnecessary.

mDNS for local-network discovery is always enabled (unless `--relay-only` clears direct transports).

## Custom Relay Server

Use a custom relay server instead of the public iroh relay infrastructure. When you specify `--relay-url`, the iroh discovery server is disabled automatically — both sides find each other through the shared relay.

```bash
# Both sides must use the same relay (tokens via files — recommended)
tunnel-rs server \
  --relay-url https://relay.example.com \
  --allowed-tcp 127.0.0.0/8 \
  --auth-tokens-file ./auth_tokens.txt

tunnel-rs client \
  --relay-url https://relay.example.com \
  --server-node-id <ID> \
  --source tcp://127.0.0.1:22 \
  --target 127.0.0.1:2222 \
  --auth-token-file ./auth_token.txt

# Force relay-only (no direct P2P) - CLI-only flag (not supported in config files)
tunnel-rs server \
  --relay-url https://relay.example.com \
  --relay-only \
  --allowed-tcp 127.0.0.0/8 \
  --auth-tokens-file ./auth_tokens.txt
```

When a custom relay is in use, clients and server find each other via:
1. **The shared relay server** — Both specify the same `--relay-url`
2. **mDNS** — Automatic discovery on the same local network (always enabled)

> **Tip:** For container deployments, use environment variables instead of files: `TUNNEL_RS_AUTH_TOKENS`, `TUNNEL_RS_SECRET` (server); `TUNNEL_RS_AUTH_TOKEN` (client).

### Running iroh-relay (Quick Start)

```bash
cargo install iroh-relay
iroh-relay --dev  # Local testing on http://localhost:3340

# Or with the bundled dev config (metrics disabled, avoids the port-9090 clash):
iroh-relay --dev -c test-scripts/relay-dev.toml
```

> [!NOTE]
> **No relay-level client whitelisting:** The self-hosted relay server must allow all client IDs (like the public iroh relay) because tunnel-rs clients use ephemeral EndpointIds that change on each run. Rely on tunnel-rs auth tokens for access control instead. See [Dynamic Client Whitelisting](ROADMAP.md#dynamic-client-whitelisting-for-self-hosted-relay) for a planned enhancement.

## Full Self-Hosted Infrastructure

For fully independent operation, self-host an iroh relay. Point both sides at it with `--relay-url`; discovery is handled through the relay automatically (the iroh discovery server is disabled when a custom relay is set).

### Running iroh-relay

```bash
cargo install iroh-relay
iroh-relay --config relay.toml  # production; use --dev (no config) for local testing
```

Example `relay.toml` (production, with TLS):
```toml
# Enable QUIC address discovery
enable_quic_addr_discovery = true

# TLS configuration (required for production)
[tls]
cert_mode = "Manual"
manual_cert_path = "/etc/letsencrypt/live/relay.example.com/fullchain.pem"
manual_key_path = "/etc/letsencrypt/live/relay.example.com/privkey.pem"

# Alternative: use Let's Encrypt automatic certificates
# [tls]
# cert_mode = "LetsEncrypt"
# hostname = "relay.example.com"
```

> **Note (ports, verified against iroh-relay 1.0.2):** `--dev` runs the relay
> over plain HTTP on port **3340** (`http_bind_addr`) and starts a Prometheus
> **metrics** server on **9090** (`metrics_bind_addr`); it does **not** start a
> QUIC endpoint, because QUIC address discovery requires TLS, which `--dev`
> ignores. If port 9090 is already in use (e.g. by Cockpit), the simplest fix is
> to turn the metrics server off with `enable_metrics = false` in the config file
> (the E2E tunnel test does not need metrics); or, to keep metrics, move it with
> `metrics_bind_addr = "127.0.0.1:9099"`. Either works because `--dev` still
> honors non-TLS config fields. For production, configure TLS: the relay serves
> HTTP on **80** and HTTPS on **443** by default, plus QUIC address discovery on
> **7842** (`quic_bind_addr`) when `enable_quic_addr_discovery = true`.

### Using Your Infrastructure

```bash
# Server (tokens via files — recommended)
tunnel-rs server \
  --relay-url https://relay.example.com \
  --secret-file ./server.key \
  --allowed-tcp 127.0.0.0/8 \
  --auth-tokens-file ./auth_tokens.txt

# Client (tokens via files — recommended)
tunnel-rs client \
  --relay-url https://relay.example.com \
  --server-node-id <ID> \
  --source tcp://127.0.0.1:22 \
  --target 127.0.0.1:2222 \
  --auth-token-file ./auth_token.txt
```

> **Tip:** For container deployments, use environment variables (`TUNNEL_RS_AUTH_TOKENS`, `TUNNEL_RS_AUTH_TOKEN`) instead of files.

## Relay Behavior

iroh mode uses the relay for both **signaling/coordination** and as a **data transport fallback**:

1. Initial connection goes through relay for signaling
2. iroh attempts coordinated hole punching (similar to libp2p's DCUtR protocol)
3. If successful (~70%), traffic flows directly between peers
4. If hole punching fails, **traffic continues through relay**

> [!NOTE]
> **Bandwidth Concern:** If you want signaling-only coordination **without** relay fallback (to avoid forwarding any tunnel traffic), iroh mode currently doesn't support this. The relay always acts as fallback when direct connection fails.
