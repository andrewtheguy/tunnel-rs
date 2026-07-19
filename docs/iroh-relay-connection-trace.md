# Iroh Relay Connection Trace

This document traces the API calls made when `endpoint.online()` is called in iroh. Useful for troubleshooting relay connectivity issues.

## Status: Cloudflare Tunnel works

Verified 2026-07-19 with iroh-relay 1.0.2 and cloudflared 2026.7.2 — iroh-relay
behind Cloudflare Tunnel works with **both** quick tunnels (`*.trycloudflare.com`)
and named tunnels (custom domains). The relay-only e2e test passes end to end:

```bash
./test-scripts/run_e2e.sh --relay-url https://relay.example.com --relay-only
```

An earlier version of this document claimed named tunnels fail because HTTP/2
does not support the `Upgrade` header mechanism. That is no longer the case
with the versions above; no paid Cloudflare plan or HTTP/1.1 override is needed.

Note: the relay may still log occasional
`ERROR iroh_relay::server::http_server: failed to handle connection
error=Connection did not reach established state within timeout` lines while
running behind cloudflared. These are harmless — traffic passes and the e2e
test succeeds despite them.

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

## Manual verification

To check that a relay URL accepts the WebSocket upgrade without running the
full e2e test:

```bash
# Should return 101 Switching Protocols
curl -v --http1.1 \
  -H "Connection: Upgrade" \
  -H "Upgrade: websocket" \
  -H "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==" \
  -H "Sec-WebSocket-Version: 13" \
  -H "Sec-WebSocket-Protocol: iroh-relay-v2, iroh-relay-v1" \
  https://relay.example.com/relay
```
