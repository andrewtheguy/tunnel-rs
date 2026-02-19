# macOS Localhost Multi-Binding for tunnel-ice

**Status:** Proposal (not yet implemented)

**Scope:** This proposal affects **tunnel-ice only** (nostr and manual modes). The tunnel-iroh crate is already fixed.

## Problem

On macOS, when a tunnel-ice client listens on `localhost`, third-party applications may fail to connect. This happens because:

1. macOS resolves `localhost` to `::1` (IPv6) before `127.0.0.1` (IPv4)
2. The tunnel-ice client binds to only one address (typically `127.0.0.1`)
3. Third-party apps try `::1` first, get "connection refused", then either fail or wait 250ms for IPv4 fallback

## Current State

**tunnel-iroh:** Already fixed - uses `resolve_listen_addrs` (plural) and creates multiple TCP listeners.

**tunnel-ice (this proposal):** Not fixed - still uses single-address binding:

| File | Function | Issue |
|------|----------|-------|
| `nostr/client.rs` | `run_nostr_tcp_client` | Uses `resolve_listen_addr`, binds TCP to single address |
| `nostr/client.rs` | `run_nostr_udp_client` | Uses `resolve_listen_addr`, binds UDP to single address |
| `custom/tunnel.rs` | `run_manual_client` | Takes single `SocketAddr`, binds to single address |
| `main.rs` | manual client entry | Calls `resolve_listen_addr` (singular) |

## Why Dual-Stack Sockets Don't Work

Dual-stack sockets (binding to `[::]`) do NOT work for loopback:
- `[::]` listens on all interfaces for external traffic
- `127.0.0.1` and `::1` are distinct loopback addresses
- A socket bound to `[::]` does NOT receive connections to `127.0.0.1` or `::1`

**Solution:** Bind separate listeners to both `127.0.0.1` and `::1`.

## Proposed Solution

### For TCP (nostr and manual modes)

1. Use `resolve_listen_addrs` (plural) which returns both IPv4 and IPv6 for localhost
2. Create a `TcpListener` for each address
3. Use tokio channel pattern to accept connections from any listener

```rust
// Example pattern (already implemented in tunnel-iroh)
async fn run_tcp_client_multi_listen(
    conn: Arc<Connection>,
    listen_addrs: &[SocketAddr],
) -> Result<()> {
    use tokio::sync::mpsc;

    // Create listeners for all addresses
    let mut listeners = Vec::with_capacity(listen_addrs.len());
    for addr in listen_addrs {
        let listener = TcpListener::bind(addr).await?;
        log::info!("Listening on TCP {}", addr);
        listeners.push(listener);
    }

    // Channel to receive accepted connections from any listener
    let (tx, mut rx) = mpsc::channel::<(TcpStream, SocketAddr)>(32);

    // Spawn accept task for each listener
    for listener in listeners {
        let tx = tx.clone();
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, peer)) => {
                        if tx.send((stream, peer)).await.is_err() {
                            break; // Channel closed
                        }
                    }
                    Err(e) => log::warn!("Accept error: {}", e),
                }
            }
        });
    }
    drop(tx); // Drop original sender

    // Process connections from any listener
    while let Some((tcp_stream, peer_addr)) = rx.recv().await {
        // Handle connection...
    }

    Ok(())
}
```

### For UDP (nostr mode)

UDP is more complex because:
- Need multiple `UdpSocket` instances bound to different addresses
- Must track which socket received data to send responses correctly
- Client state management becomes more complex

**Options:**
1. **Simple:** Keep single UDP socket, document limitation
2. **Full fix:** Multiple UDP sockets with shared client state tracking

Recommend starting with option 1 (document limitation) and implementing option 2 if users report issues.

## Implementation Plan

### Phase 1: TCP Multi-Binding

#### Files to Modify

1. **`crates/tunnel-ice/src/nostr/client.rs`**
   - Update `run_nostr_tcp_client` to use `resolve_listen_addrs`
   - Implement multi-listener pattern with channel
   - Keep existing connection handling logic

2. **`crates/tunnel-ice/src/custom/tunnel.rs`**
   - Change `run_manual_client` signature: `listen: SocketAddr` → `listen_addrs: &[SocketAddr]`
   - Implement multi-listener pattern for TCP branch

3. **`crates/tunnel-rs-ice/src/main.rs`**
   - Update manual client to call `resolve_listen_addrs` (plural)
   - Pass `&listen_addrs` to `run_manual_client`

### Phase 2: UDP Multi-Binding (Optional)

More complex, defer unless users report issues. Would require:
- Multiple `UdpSocket` instances
- Shared client state across sockets
- Response routing based on which socket received the request

**Feedback Tracking:** User feedback for Phase 2 will be gathered via GitHub Issues. If users encounter UDP connectivity issues on macOS with localhost bindings, they should open an issue with the label `udp-multi-bind`. We will prioritize Phase 2 implementation based on reported issues.

## Breaking Changes

This proposal introduces breaking changes to function signatures. Since the project is pre-release, backwards compatibility is not a concern.

| Function | Current Signature | New Signature | Impact |
|----------|------------------|---------------|--------|
| `run_manual_client` | `listen: SocketAddr` | `listen_addrs: &[SocketAddr]` | Callers must pass a slice instead of single address |
| `run_nostr_tcp_client` | Uses `resolve_listen_addr` (singular) internally | Uses `resolve_listen_addrs` (plural) internally | No external API change; internal behavior change |
| `run_nostr_udp_client` | Uses `resolve_listen_addr` (singular) internally | No change in Phase 1 | Unchanged until Phase 2 |

**Dependent Code:**
- `crates/tunnel-rs-ice/src/main.rs`: Must update manual client call site to use `resolve_listen_addrs` and pass `&listen_addrs`
- Any external consumers of `run_manual_client`: Must update to pass `&[SocketAddr]`

## Testing

- Verify on macOS: third-party apps can connect via both `localhost` and `127.0.0.1`
- Verify on Linux: behavior unchanged (Linux typically returns IPv4 first)
- Verify explicit IP binding still works (`127.0.0.1:2222` binds to single address)

## Platform Behavior Reference

| Platform | `localhost` Resolution Order | Dual-Stack Default |
|----------|------------------------------|-------------------|
| macOS    | `::1` first, then `127.0.0.1` | `IPV6_V6ONLY` forced ON |
| Linux    | Configurable via `/etc/gai.conf`, typically IPv4 first | Dual-stack enabled |
| Windows  | Varies by version | `IPV6_V6ONLY` ON by default |

## Related

- tunnel-iroh implementation: `crates/tunnel-iroh/src/iroh_mode/multi_source.rs`
- Shared helper: `resolve_listen_addrs` in `tunnel_common.rs`
