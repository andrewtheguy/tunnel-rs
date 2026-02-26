# tunnel-rs Roadmap

This document outlines planned features and improvements for tunnel-rs.

## Current Status

tunnel-rs currently supports three operational modes:
- **iroh**: Persistent identity with automatic discovery, relay fallback, and client-requested sources
- **nostr**: Full ICE with automated Nostr relay signaling and client-requested sources
- **manual**: Full ICE with manual signaling (single-session)

Port forwarding modes (iroh, nostr, manual) support TCP and UDP tunneling with end-to-end encryption via QUIC/TLS 1.3.

---

## Planned Features

### Medium Priority

#### Multi-Source/Target per Client

**Status:** Idea

Currently, each client connection tunnels a single source to a single target. This feature would allow a single client to tunnel multiple source/target pairs simultaneously, with live updates.

**Proposed Features:**
- **Multiple tunnels per client**: Configure multiple `--source`/`--target` pairs in one client instance
- **Live update**: Add/remove tunnels without restarting the client (via config file reload, API, or CLI command)
- **Config file support**: Define multiple tunnels in TOML config

**Example (proposed config):**
```toml
role = "client"
mode = "iroh"

[iroh]
server_node_id = "..."
auth_token = "..."

[[iroh.tunnels]]
source = "tcp://127.0.0.1:22"
target = "127.0.0.1:2222"

[[iroh.tunnels]]
source = "tcp://127.0.0.1:5432"
target = "127.0.0.1:5432"

[[iroh.tunnels]]
source = "udp://127.0.0.1:53"
target = "127.0.0.1:5353"
```

**Complexity:** High
- Requires refactoring client to manage multiple listener loops
- Live update needs signal handling (SIGHUP) or control socket/API
- State management for adding/removing tunnels without disrupting existing connections
- Error handling per-tunnel (one tunnel failure shouldn't affect others)

**Use Cases:**
- Single client exposing multiple services (SSH + database + DNS)
- Dynamic service discovery and tunnel provisioning
- Reduced overhead vs. running multiple client processes

---

#### Auth Rate Limiting

**Status:** Idea

Rate limiting for token authentication to prevent brute-force attacks. Hybrid approach with per-client limits (for typo handling) and global limits (for distributed attack detection).

Design to be documented in a dedicated proposal file.

---

#### Dynamic Client Whitelisting for Self-Hosted Relay

**Status:** Idea

When self-hosting an iroh relay server, there is currently no easy way to whitelist specific clients at the relay level because clients use ephemeral identities by default.

**Problem:**
- Clients use ephemeral identities (new EndpointId each run)
- Self-hosted relay servers cannot restrict which clients are allowed to connect
- No mechanism to dynamically authorize client identities

**Proposed Solution:**

The iroh-relay server supports dynamic access control via `AccessConfig::Restricted`, which takes a callback function that checks each `EndpointId` and returns `Access::Allow` or `Access::Deny`. The solution involves dynamic coordination between the tunnel-rs server and the self-hosted relay:

1. **Client connects to tunnel-rs server** with ephemeral EndpointId
2. **Tunnel-rs server authenticates client** via auth token (existing mechanism)
3. **Server registers client's EndpointId** with the relay's dynamic whitelist
4. **Client can now use the relay** for NAT traversal

Clients continue to use ephemeral identities - the tunnel-rs server dynamically coordinates with the relay to authorize authenticated clients.

**iroh-relay access control API** (from [iroh-relay](https://github.com/n0-computer/iroh/tree/main/iroh-relay)):
```rust
pub enum AccessConfig {
    Everyone,
    Restricted(Box<dyn Fn(EndpointId) -> Boxed<Access> + Send + Sync + 'static>),
}

pub enum Access {
    Allow,
    Deny,
}
```

**Implementation approach:**
- Relay server exposes an API or shared state for dynamic whitelist updates
- Tunnel-rs server adds client EndpointIds after successful auth token validation
- Tunnel-rs server removes EndpointIds when clients disconnect
- The `AccessConfig::Restricted` callback queries this dynamic whitelist

**Complexity:** Medium-High
- Requires coordination protocol between tunnel-rs server and self-hosted relay
- Relay needs to expose whitelist management API (file watch, HTTP API, or shared memory)
- Cleanup logic for stale EndpointIds when clients disconnect

**Use Cases:**
- Private self-hosted relay infrastructure
- Enterprise deployments requiring relay-level access control
- Additional defense-in-depth beyond tunnel-rs auth tokens

---

#### macOS Localhost Multi-Binding (tunnel-ice only)

**Status:** Idea

**Note:** This issue affects **tunnel-ice only** (nostr and manual modes). The iroh mode is already fixed.

On macOS, third-party apps connecting to `localhost` try IPv6 (`::1`) before IPv4 (`127.0.0.1`). If the tunnel-ice client only binds to one address, connections may fail or experience 250ms delays. The fix is to bind to both addresses when listening on localhost.

See [MACOS_LOCALHOST_PROPOSAL.md](MACOS_LOCALHOST_PROPOSAL.md) for detailed design.

---

#### Relay Fallback for manual/nostr Modes

**Status:** Idea

manual and nostr modes use full ICE but have no relay fallback for symmetric NAT scenarios where direct connectivity fails.

---

#### Automatic Reconnection

**Status:** Partial

| Feature | Status |
|---------|--------|
| QUIC keepalive (15s interval) | **Implemented** |
| Stream retry with backoff | **Implemented** |
| Connection-level auto-reconnect | Idea |

**iroh mode (Moderate complexity):**
- Add client-side connection retry loop with exponential backoff
- Iroh's discovery automatically re-resolves server's new IP/relay address

**nostr mode (Higher complexity):**
- Re-signal via Nostr relays and re-establish ICE/QUIC

---

#### Connection Migration (Resilience to IP Changes)

**Status:** Idea

QUIC natively supports connection migration, allowing sessions to continue when network path changes. Currently, active sessions may drop if a peer's IP changes.

---

#### Performance Metrics

**Status:** Idea

Built-in monitoring for connection latency, throughput, packet loss, and uptime.

---

#### Multi-path Support

**Status:** Idea

Utilize multiple network paths simultaneously for increased throughput or redundancy.

---

#### Web UI

**Status:** Idea

Browser-based interface for configuration, monitoring, and key management.

---

#### Smart Routing (Server Mesh)

**Status:** Idea

A mesh of tunnel-rs servers where clients can connect to any server and be redirected to the optimal server based on routing rules.

**Concept:**
- Multiple tunnel-rs servers form a mesh, each responsible for certain CIDR ranges or services
- Client connects to any server in the mesh
- Server evaluates the requested source against routing rules and either:
  - Handles the connection directly if it owns the route
  - Returns the address of the best server for that destination
  - Proxies the connection through the mesh

**Proposed Routing Criteria:**
- **CIDR-based**: Route `10.0.0.0/8` to Server A, `192.168.0.0/16` to Server B
- **Service-based**: Route database connections to Server A, SSH to Server B
- **Geographic**: Route based on client location for latency optimization
- **Load-based**: Distribute connections across servers based on current load

**Example (proposed config):**
```toml
role = "server"
mode = "iroh"

[mesh]
enabled = true
peers = ["node_id_a", "node_id_b", "node_id_c"]

[[mesh.routes]]
cidr = "10.0.0.0/8"
owner = "self"  # This server handles this range

[[mesh.routes]]
cidr = "192.168.0.0/16"
owner = "node_id_b"  # Redirect to Server B
```

**Complexity:** High
- Requires mesh discovery and health checking between servers
- Routing table synchronization across the mesh
- Decision: redirect client vs. proxy through mesh
- Fallback handling when preferred server is unavailable

**Use Cases:**
- Distributed infrastructure with region-specific access
- High availability with automatic failover
- Load distribution across multiple servers
- Simplified client configuration (connect to any entry point)

---

## Contributing

Feature requests and contributions are welcome. Please open an issue on GitHub to discuss proposed changes before submitting a pull request.

---

## References

- [ARCHITECTURE.md](ARCHITECTURE.md) - Detailed technical architecture
- [README.md](../README.md) - Usage documentation
