# tunnel-rs Roadmap

This document outlines planned features and improvements for tunnel-rs.

## Current Status

tunnel-rs uses iroh mode with persistent identity, automatic discovery, relay fallback, and client-requested sources. It supports TCP and UDP tunneling with end-to-end encryption via QUIC/TLS 1.3.

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
private_key_file = "~/.config/tunnel-rs/client.key"

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

#### External Authorized-Key Source

**Status:** Idea

Allow the server to fetch authorized Ed25519 public keys from an external HTTP REST service at runtime instead of loading a static file only at startup. This enables centralized key management where clients can be added or revoked without restarting the server.

**Proposed Features:**
- **Remote key endpoint**: Server periodically queries a configurable HTTP endpoint to retrieve authorized public keys
- **Polling interval**: Configurable refresh interval (default: 60s)
- **Caching with fallback**: Cache the last successful response so the server continues operating if the external service is temporarily unavailable
- **Startup behavior**: Fetch keys on startup; fail fast if the endpoint is unreachable and no fallback keys are configured

**Example:**
```bash
tunnel-rs server \
  --secret-file ./server.key \
  --allowed-tcp 127.0.0.0/8 \
  --authorized-keys-url https://auth.example.com/api/authorized-keys
```

**Expected response format:**
```json
{
  "authorized_keys": [
    "ed25519 AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA= alice",
    "ed25519 BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB= bob"
  ]
}
```

**Use Cases:**
- Centralized key authorization across multiple tunnel-rs servers
- Revoking a compromised client key without restarting any servers
- Integration with existing identity providers, admin dashboards, or secret managers
- Container orchestration systems that manage secrets externally (e.g., Vault, AWS Secrets Manager)

**Complexity:** Medium
- Requires atomically replaceable authorized-key state
- Background task for periodic polling (tokio interval)
- HTTP client dependency (reqwest)
- Decision: whether existing sessions with revoked keys should be terminated or only future connections denied

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
2. **Tunnel-rs server authenticates client** via Ed25519 challenge-response
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
- Tunnel-rs server adds client EndpointIds after successful public-key authentication
- Tunnel-rs server removes EndpointIds when clients disconnect
- The `AccessConfig::Restricted` callback queries this dynamic whitelist

**Complexity:** Medium-High
- Requires coordination protocol between tunnel-rs server and self-hosted relay
- Relay needs to expose whitelist management API (file watch, HTTP API, or shared memory)
- Cleanup logic for stale EndpointIds when clients disconnect

**Use Cases:**
- Private self-hosted relay infrastructure
- Enterprise deployments requiring relay-level access control
- Additional defense-in-depth beyond tunnel-rs application authentication

**Simpler alternative since iroh 0.98 — endpoint-side `EndpointHooks`:**

If relay-level filtering isn't required (i.e. you accept that an unauthorized client can *reach* the relay but you want to reject them at your tunnel-rs server), iroh 0.98 added `iroh::endpoint::EndpointHooks` which avoids the relay-coordination complexity above:

```rust
pub trait EndpointHooks {
    async fn after_handshake(&self, conn: &ConnectionInfo) -> AfterHandshakeOutcome;
    // -> Accept | Reject { error_code: VarInt, reason: Vec<u8> }
}
```

Installed on the tunnel-rs server's own endpoint via `Endpoint::Builder::hooks(...)`. Runs after the QUIC TLS handshake, so the remote's verified `EndpointId` is known. Rejecting here closes the connection with a QUIC close frame *before* it consumes an `accept_bi()` slot or reaches the application public-key check in `multi_source.rs`.

This does not replace the Ed25519 challenge-response in `src/auth.rs` (the hook only sees transport metadata, not stream bytes), but it could be a second factor. An `--allowed-endpoint-ids` flag would require stable client transport identities, however, losing the current ephemeral-identity benefit. It is therefore only suitable as opt-in hardening.

---

#### External Address Hint for Kubernetes / NAT Environments

**Status:** Idea

When tunnel-rs runs inside Kubernetes (or behind any symmetric NAT), iroh's STUN-based hole-punching fails because the overlay network's conntrack-based NAT assigns different external ports per destination. All connections fall back to relay. The `hostNetwork: true` workaround bypasses K8s networking entirely but has tradeoffs (port conflicts, no network policies).

This feature adds an `--external-address` flag so the server can advertise a known externally-reachable socket address in its published `EndpointAddr`, enabling direct connections without `hostNetwork`.

**Proposed Features:**
- **CLI flag**: `--external-address <IP:PORT>` (repeatable for multiple addresses)
- **Config file support**: `external_addresses = ["203.0.113.5:30000"]`
- **iroh integration**: Inject addresses as `TransportAddr::Ip(SocketAddr)` into the server's published `EndpointAddr` via Pkarr/DNS
- Clients automatically discover and use these addresses alongside relay

**Example with Kubernetes NodePort:**
```bash
# Create a NodePort service for QUIC (UDP)
kubectl apply -f - <<EOF
apiVersion: v1
kind: Service
metadata:
  name: tunnel-rs-nodeport
spec:
  type: NodePort
  ports:
    - port: 12345
      targetPort: 12345
      nodePort: 30000
      protocol: UDP
  selector:
    app: tunnel-server
EOF

# Server advertises the NodePort address
tunnel-rs server \
  --secret-file ./server.key \
  --allowed-tcp 10.0.0.0/8 \
  --authorized-keys-file ./authorized_keys \
  --external-address 203.0.113.5:30000
```

**Example with Cloud LoadBalancer:**
```bash
# AWS NLB or GCP LoadBalancer with static IP
tunnel-rs server \
  --external-address <LOAD_BALANCER_IP>:12345
```

**Example config:**
```toml
[iroh]
external_addresses = ["203.0.113.5:30000"]
```

**Complexity:** Medium
- Add CLI/config parsing for external addresses
- Modify `create_server_endpoint()` in `endpoint.rs` to include external addresses in the published `EndpointAddr`
- iroh's `EndpointAddr` already supports `TransportAddr::Ip(SocketAddr)` — need to ensure these get published via Pkarr/DNS alongside auto-discovered addresses

**Use Cases:**
- K8s pods behind overlay NAT with NodePort or LoadBalancer services
- VMs behind cloud NAT with static port mappings
- Any environment where the server's externally-reachable address differs from what STUN discovers

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
