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
does not support the `Upgrade` header mechanism. Inspection of the iroh-relay
client source (checked at v0.95.1, v0.98.0, v1.0.0, v1.0.2) shows that claim
never applied to the iroh client: the relay client builds its rustls config
without any `alpn_protocols`, so no ALPN extension is sent, Cloudflare's edge
falls back to HTTP/1.1, and the connection is a plain HTTP/1.1 WebSocket
upgrade (`hyper::client::conn::http1` + `tokio_websockets`) in every version.
The HTTP/2 failure mode belongs to `curl` (which offers `h2` via ALPN by
default) — hence the misleading `400 Bad Request` from a bare
`curl -v https://.../relay`. No paid Cloudflare plan or HTTP/1.1 override is
needed.

Related iroh changes that improved relay-only operation behind an HTTP-only
proxy between 0.95.x and 1.0.x:

- 0.97.0 ([#3926](https://github.com/n0-computer/iroh/pull/3926)): QAD (QUIC
  address discovery) probes are skipped when no IP transports are configured,
  instead of failing repeatedly.
- 0.98.0 ([#3955](https://github.com/n0-computer/iroh/pull/3955)): relay
  protocol updated to `iroh-relay-v2`, still negotiated via the standard
  `Sec-WebSocket-Protocol` header (proxy-friendly).
- 0.98.0 ([#4115](https://github.com/n0-computer/iroh/pull/4115)):
  `Endpoint::online()` now returns only once actually connected to the home
  relay, instead of when net_report merely selected it.

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
full e2e test. `--no-alpn` is a curl flag that disables the TLS ALPN extension
on the HTTPS connection, matching what the iroh relay client sends. This TLS
ALPN (h2 vs http/1.1 negotiation with the proxy/edge) is unrelated to
tunnel-rs's fixed QUIC ALPN `mf/4`, which identifies the peer-to-peer protocol
inside the end-to-end encrypted QUIC connection and is never seen by the relay
or Cloudflare:

```bash
# Should return 101 Switching Protocols
curl -v --no-alpn \
  -H "Connection: Upgrade" \
  -H "Upgrade: websocket" \
  -H "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==" \
  -H "Sec-WebSocket-Version: 13" \
  -H "Sec-WebSocket-Protocol: iroh-relay-v2, iroh-relay-v1" \
  https://relay.example.com/relay
```

Caveats confirmed against a live named tunnel (2026-07-19):

- A bare `curl -v https://relay.example.com/relay` returns `400 Bad Request`.
  This is NOT a Cloudflare or HTTP/2 problem — the relay server itself returns
  400 for any non-WebSocket request (the same 400 comes back from
  `curl http://localhost:3340/relay` with no proxy involved). Only the full
  upgrade request above is a meaningful health check.
- With the upgrade headers, the request succeeds (101) over HTTP/1.1 both with
  `--http1.1` and with `--no-alpn`; Cloudflare's edge falls back to HTTP/1.1
  whenever the client doesn't offer `h2` via ALPN, which the iroh client never
  does.
