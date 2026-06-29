# Iroh Relay Connection Trace

This document traces the API calls made when `endpoint.online()` is called in iroh. Useful for troubleshooting relay connectivity issues.

## Problem Context

When using iroh-relay behind Cloudflare Tunnel:
- **Quick tunnels** (`*.trycloudflare.com`) work correctly
- **Named tunnels** (custom domains) may fail due to HTTP/2 not supporting WebSocket upgrades

## Connection Flow

1. `endpoint.online()` waits for `home_relay()` to be initialized
2. `home_relay()` watches `local_addrs_watch` for relay addresses
3. Relay transport watches `my_relay` which gets set when network probes determine the preferred relay
4. `dial_relay()` calls `ClientBuilder::connect()` with a 10s timeout

### ClientBuilder::connect()

```rust
pub async fn connect(&self) -> Result<Client, ConnectError> {
    // 1. Convert URL scheme (https -> wss, http -> ws)
    let mut dial_url = (*self.url).clone();
    dial_url.set_path("/relay");

    // 2. Establish TCP + TLS connection
    let stream = MaybeTlsStreamBuilder::new(dial_url.clone(), self.dns_resolver.clone())
        .connect().await?;

    // 3. WebSocket upgrade with iroh-relay protocol negotiation
    let (conn, response) = tokio_websockets::ClientBuilder::new()
        .uri(dial_url.as_str())?
        .add_header(SEC_WEBSOCKET_PROTOCOL, "iroh-relay-v2, iroh-relay-v1")?
        .connect_on(stream).await?;

    // 4. Verify 101 Switching Protocols response
    ensure!(response.status() == StatusCode::SWITCHING_PROTOCOLS, ...);

    // 5. Complete iroh-relay handshake
    Ok(Client { conn: Conn::new(conn, ...).await?, ... })
}
```

## Key Timeouts

| Timeout | Value | Description |
|---------|-------|-------------|
| Connect | 10s | Overall dial timeout |
| DNS | 1s | DNS resolution |
| TCP Dial | 1.5s | TCP connect |

## HTTP Request/Response

### WebSocket Upgrade Request

```http
GET /relay HTTP/1.1
Host: your-relay-server.com
Connection: Upgrade
Upgrade: websocket
Sec-WebSocket-Key: <random-base64>
Sec-WebSocket-Version: 13
Sec-WebSocket-Protocol: iroh-relay-v2, iroh-relay-v1
```

### Expected Response

```http
HTTP/1.1 101 Switching Protocols
Upgrade: websocket
Connection: Upgrade
Sec-WebSocket-Accept: <computed-hash>
Sec-WebSocket-Protocol: iroh-relay-v2
```

## Cloudflare Tunnel Issue

**Quick tunnels work** because they automatically detect WebSocket traffic and use HTTP/1.1.

**Named tunnels may fail** because they use HTTP/2 by default, which doesn't support the `Upgrade` header mechanism.

**Recommendation:** For self-hosted iroh-relay, open a port directly instead of using Cloudflare Tunnel. The HTTP/2 issue with named tunnels requires a paid Cloudflare plan to force HTTP/1.1 to resolve.

### Verification

```bash
# HTTP/2 (may fail) - returns 400 Bad Request
curl -v https://your-named-tunnel.example.com/relay

# HTTP/1.1 (should work) - returns 101 Switching Protocols
curl -v --http1.1 \
  -H "Connection: Upgrade" \
  -H "Upgrade: websocket" \
  -H "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==" \
  -H "Sec-WebSocket-Version: 13" \
  -H "Sec-WebSocket-Protocol: iroh-relay-v2, iroh-relay-v1" \
  https://your-named-tunnel.example.com/relay
```
