# Self-Hosting Iroh Infrastructure

This document covers how to self-host iroh's relay and Pkarr discovery services for fully independent operation in port forwarding mode (`tunnel-rs`).

## Peer Discovery

The `--discovery` option (or `[iroh].discovery` in a config file) controls
internet address discovery independently from the relay configuration:

- **Omitted (default):** with no custom relays, use iroh's public discovery
  service (the server publishes its home relay and addresses; clients resolve
  them). With custom `--relay-url`s configured, internet discovery is disabled
  automatically — clients reach the server through relay hints, and nothing is
  published to public iroh infrastructure.
- **Custom URL:** publish and resolve through that Pkarr HTTP endpoint, for
  example `--discovery https://dns.example.com/pkarr`. Configure the same URL
  on the server and clients. An explicit URL is honored even when custom
  relays are configured.
- **`none`:** disable internet discovery. The client then relies on relay hints
  supplied by `--relay-url`, or on mDNS when both peers share a local network.

mDNS for local-network discovery remains enabled for all three settings.
`--relay-only` is the exception: it skips Pkarr, DNS, and mDNS discovery and
rendezvous occurs solely through the explicitly configured relay.

Relay hints make discovery unnecessary with custom relays, including more than
one: the client includes every configured relay in the server's address, and
iroh sends the QUIC handshake packets to all of them, so the connection is
established via whichever relay the server is currently homed on. See
[relay-discovery-findings.md](relay-discovery-findings.md) for the full
analysis (iroh internals, failure-mode caveats, and e2e verification).

> [!WARNING]
> Configure clients with the **full** relay list. An iroh endpoint has one home
> relay at a time, and relay servers are stateless and independent: they do not
> synchronize registrations or forward traffic to one another. A client
> configured with only a subset of the server's relays can reach the server only
> while the server's current home relay is in that subset. (After its home relay
> goes offline, the server re-homes onto another configured relay within
> ~30 seconds.) If clients must work with partial relay lists, configure an
> explicit shared discovery server instead.

## Custom Relay Server

Use a custom relay server instead of the public iroh relay infrastructure.
Configuring any custom relay disables internet discovery automatically; set
`--discovery` explicitly only to point at a self-hosted Pkarr service.

```bash
# Both sides must use the same relay(s) (tokens via files — recommended)
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

With custom relays (internet discovery auto-disabled), clients and the server
find each other through the shared relay URLs or through mDNS on the same
local network.

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

For fully independent operation, self-host one or more iroh relays; with custom
relays configured on both sides, internet discovery is disabled automatically
and no discovery service is needed. Optionally self-host a Pkarr discovery
service instead (e.g. to support clients configured with only part of the relay
list).

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

### Simple production setup: relay behind Cloudflare Tunnel (single TCP port)

If you don't want to manage TLS certificates or open inbound ports, run the
relay over **plain HTTP on a single TCP port** and let Cloudflare Tunnel
terminate TLS at the edge and forward decrypted HTTP to it. Only outbound
connectivity is needed on the relay host — no public IP, no 443, no QUIC/UDP.

**How it works:** omitting the `[tls]` section makes iroh-relay serve *all*
services (the `/relay` WebSocket and the `healthz` routes) over plain HTTP on
`http_bind_addr`. QUIC address discovery defaults to off, so no TLS is required
and the relay starts cleanly without `--dev`. This is the non-`--dev`
equivalent of the local dev config — verified against iroh-relay 1.0.2.

Copy the bundled template [`../relay-prod.toml.example`](../relay-prod.toml.example) to `relay-prod.toml` and adjust as needed:

```toml
# Plain-HTTP relay on 3340 (the non-dev default is port 80). Must match the
# cloudflared ingress service and the --relay-url the clients use.
http_bind_addr = "[::]:3340"

# Metrics server defaults to port 9090 and often collides with other services;
# turn it off (or move it to a private address you do NOT expose via the tunnel).
enable_metrics = false
# metrics_bind_addr = "127.0.0.1:9099"

# Recommended for a publicly reachable relay: require a bearer token. Clients
# then use --relay-url https://relay.example.com/?token=<secret>
# access.shared_token = ["change-me-to-a-long-random-secret"]
```

**1. Run the relay** (no `--dev`):

```bash
cp relay-prod.toml.example relay-prod.toml
iroh-relay -c relay-prod.toml
```

**2. Point cloudflared at it.** The tunnel's ingress must forward the hostname
to `http://localhost:3340`. With a token-based (dashboard-managed) tunnel this
is one line in the dashboard; for a locally-managed tunnel, `config.yml`:

```yaml
tunnel: <tunnel-uuid>
credentials-file: /root/.cloudflared/<tunnel-uuid>.json

ingress:
  - hostname: relay.example.com
    service: http://localhost:3340
  - service: http_status:404
```

```bash
# Locally-managed tunnel:
cloudflared tunnel run <tunnel-name>
# Or dashboard/token-managed tunnel:
cloudflared tunnel run --token <token>
```

**3. Verify** end to end:

```bash
./test-scripts/run_e2e.sh --relay-url https://relay.example.com --relay-only
```

> [!NOTE]
> No paid Cloudflare plan or HTTP/1.1 override is needed — the iroh relay
> client sends no TLS ALPN, so Cloudflare's edge negotiates HTTP/1.1 and the
> WebSocket upgrade works through both quick and named tunnels. A bare
> `curl https://relay.example.com/relay` returning `400` is expected (the relay
> answers 400 to any non-WebSocket request); it is not a tunnel problem. See
> [`iroh-relay-connection-trace.md`](iroh-relay-connection-trace.md) for the
> full trace.

> **Trade-off:** this routes all relayed traffic through Cloudflare and, because
> there is no QUIC endpoint, disables QUIC address discovery (one of the signals
> iroh uses to help peers hole-punch to a direct connection). For a relay whose
> job is a pure relay-only fallback this is fine; if you want to maximize direct
> P2P success, use the TLS + QUIC production config above instead.

### Using Your Infrastructure

```bash
# Server (tokens via files — recommended)
tunnel-rs server \
  --relay-url https://relay.example.com \
  --discovery https://dns.example.com/pkarr \
  --secret-file ./server.key \
  --allowed-tcp 127.0.0.0/8 \
  --auth-tokens-file ./auth_tokens.txt

# Client (tokens via files — recommended)
tunnel-rs client \
  --relay-url https://relay.example.com \
  --discovery https://dns.example.com/pkarr \
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
