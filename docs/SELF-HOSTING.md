# Self-Hosting Iroh Infrastructure

This document covers how to self-host iroh's relay and DNS servers for fully independent operation in port forwarding mode (`tunnel-rs`).

## Custom Relay Server

Use a custom relay server instead of the public iroh relay infrastructure.

> **Note:** When using `--relay-url`, you only need a custom relay server. The `--dns-server` option is **not required** — DNS discovery is only needed if you also want to avoid the public iroh DNS infrastructure (see [Self-Hosted DNS Discovery](#self-hosted-dns-discovery)).

> **Note:** The public iroh DNS endpoint is now dual-stack (IPv4 + IPv6). IPv6-only environments no longer need a custom DNS server just to reach the default discovery service.

```bash
# Both sides must use the same relay
tunnel-rs server --relay-url https://relay.example.com --allowed-tcp 127.0.0.0/8 --auth-tokens "$AUTH_TOKEN"
tunnel-rs client --relay-url https://relay.example.com --server-node-id <ID> --source tcp://127.0.0.1:22 --target 127.0.0.1:2222 --auth-token "$AUTH_TOKEN"

# Force relay-only (no direct P2P) - CLI-only flag (not supported in config files)
tunnel-rs server --relay-url https://relay.example.com --relay-only --allowed-tcp 127.0.0.0/8 --auth-tokens "$AUTH_TOKEN"
```

### Running iroh-relay (Quick Start)

```bash
cargo install iroh-relay
iroh-relay --dev  # Local testing on http://localhost:3340
```

> [!NOTE]
> **No relay-level client whitelisting:** The self-hosted relay server must allow all client IDs (like the public iroh relay) because tunnel-rs clients use ephemeral EndpointIds that change on each run. Rely on tunnel-rs auth tokens for access control instead. See [Dynamic Client Whitelisting](ROADMAP.md#dynamic-client-whitelisting-for-self-hosted-relay) for a planned enhancement.

The `--dns-server` flag (e.g., `https://dns.example.com/pkarr`) is a **pkarr peer-discovery service**, not a general-purpose DNS resolver.

## Self-Hosted DNS Discovery

For fully independent operation without public infrastructure. Note that `--dns-server` is for iroh node discovery via pkarr and does **not** provide ordinary DNS name resolution:

```bash
# Both sides use custom DNS server
tunnel-rs server --dns-server https://dns.example.com/pkarr --secret-file ./server.key --allowed-tcp 127.0.0.0/8 --auth-tokens "$AUTH_TOKEN"
tunnel-rs client --dns-server https://dns.example.com/pkarr --server-node-id <ID> --source tcp://127.0.0.1:22 --target 127.0.0.1:2222 --auth-token "$AUTH_TOKEN"
```

## Disabling DNS Discovery

You can disable DNS-based peer discovery entirely by setting `--dns-server none`:

> **Note:** This used to be a common workaround for IPv6-only networks when the public iroh DNS endpoint was IPv4-only. Since it is now dual-stack, only use `--dns-server none` if you intentionally want to disable DNS discovery.

```bash
# Both sides disable DNS discovery
tunnel-rs server --dns-server none --relay-url https://relay.example.com --allowed-tcp 127.0.0.0/8 --auth-tokens "$AUTH_TOKEN"
tunnel-rs client --dns-server none --relay-url https://relay.example.com --server-node-id <ID> --source tcp://127.0.0.1:22 --target 127.0.0.1:2222 --auth-token "$AUTH_TOKEN"
```

When DNS discovery is disabled, clients and server must connect using one of these methods:
1. **Common relay server** — Both specify the same `--relay-url`
2. **mDNS** — Automatic discovery on the same local network (always enabled)

> **Note:** mDNS discovery is unaffected by the `--dns-server none` setting and remains active for local network discovery.

## Full Self-Hosted Infrastructure

For fully independent operation, you can self-host both iroh's relay and DNS servers.

### Running iroh-relay

```bash
cargo install iroh-relay
iroh-relay --config relay.toml --dev  # --dev for local testing
```

Example `relay.toml`:
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

> **Note:** With `--dev`, the relay runs HTTP on port 3340 and QUIC on port 7824. For production, configure TLS and use a reverse proxy or direct HTTPS binding.

### Running iroh-dns-server

```bash
cargo install iroh-dns-server
iroh-dns-server --config dns.toml
```

Example `dns.toml`:
```toml
# Rate limiting for pkarr PUT requests
pkarr_put_rate_limit = "smart"

# HTTP server for pkarr API (development)
[http]
port = 8080
bind_addr = "0.0.0.0"

# HTTPS server (production)
[https]
port = 443
domains = ["dns.example.com"]
cert_mode = "lets_encrypt"
letsencrypt_prod = true

# DNS server configuration
[dns]
port = 53
default_ttl = 30
origins = ["dns.example.com", "."]
rr_a = "203.0.113.10"  # Your server's public IP
rr_ns = "ns1.dns.example.com."
default_soa = "ns1.dns.example.com hostmaster.dns.example.com 0 10800 3600 604800 3600"

# Mainline DHT fallback (optional)
[mainline]
enabled = false
```

> **Note:** The iroh-dns-server provides the `/pkarr` HTTP endpoint used by tunnel-rs for peer discovery. Refer to the [iroh-dns-server source](https://github.com/n0-computer/iroh/tree/main/iroh-dns-server) for the latest configuration options.

### Using Your Infrastructure

```bash
# Server
tunnel-rs server \
  --relay-url https://relay.example.com \
  --dns-server https://dns.example.com/pkarr \
  --secret-file ./server.key \
  --allowed-tcp 127.0.0.0/8 \
  --auth-tokens "$AUTH_TOKEN"

# Client
tunnel-rs client \
  --relay-url https://relay.example.com \
  --dns-server https://dns.example.com/pkarr \
  --server-node-id <ID> \
  --source tcp://127.0.0.1:22 \
  --target 127.0.0.1:2222 \
  --auth-token "$AUTH_TOKEN"
```

## Relay Behavior

iroh mode uses the relay for both **signaling/coordination** and as a **data transport fallback**:

1. Initial connection goes through relay for signaling
2. iroh attempts coordinated hole punching (similar to libp2p's DCUtR protocol)
3. If successful (~70%), traffic flows directly between peers
4. If hole punching fails, **traffic continues through relay**

> [!NOTE]
> **Bandwidth Concern:** If you want signaling-only coordination **without** relay fallback (to avoid forwarding any tunnel traffic), iroh mode currently doesn't support this. The relay always acts as fallback when direct connection fails.
>
> **Alternative for signaling-only:** Use [nostr mode](ALTERNATIVE-MODES.md#nostr-mode) with self-hosted Nostr relays. Nostr relays only handle signaling (small encrypted messages), never tunnel traffic. If hole punching fails, the connection fails — no traffic is ever forwarded through the relay.
