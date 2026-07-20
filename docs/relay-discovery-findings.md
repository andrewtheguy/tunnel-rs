# Why internet discovery is disabled automatically with custom relays

Analysis behind the change that auto-disables iroh internet discovery (n0 DNS
lookup + pkarr publishing) whenever custom `relay_urls` are configured
(`src/iroh_mode/endpoint.rs`, `create_endpoint_builder`). Verified against
iroh **1.0.2** source and the e2e suites in `test-scripts/` (2026-07-20).

## Question

When both client and server are configured with the same custom relays, does
the iroh discovery server make any real difference — or can it be disabled
automatically?

## Conclusion

It can be disabled. With custom relays on both sides, discovery adds nothing
to connection establishment, and leaving the default (n0) discovery on was
actively undesirable: the server published its custom relay URL and addresses
to iroh's public DNS — an internet dependency and an information leak for
otherwise self-contained deployments.

## Mechanism (iroh 1.0.2 internals)

Discovery exists to answer one question: "at which addresses / on which relay
can this EndpointId be reached?" With custom relays, the answer is already in
the config:

1. **The client passes every configured relay as a hint.** `connect_to_server`
   (`src/iroh_mode/endpoint.rs`) builds the server's `EndpointAddr` with all
   configured relay URLs (non-relay-only mode), or tries each relay in turn
   (relay-only mode).

2. **iroh sends the QUIC handshake to all hinted paths at once.** Before a
   path is selected, QUIC Initial packets are broadcast to *every* known
   transport address, including every relay hint — see
   `iroh-1.0.2/src/socket/remote_map/remote_state.rs`,
   `RemoteStateMessage::SendDatagram`: "Sends a datagram to all known paths.
   Used to send QUIC Initial packets." The handshake therefore succeeds via
   whichever relay the server is currently homed on. This is why the old
   warning ("`discovery = "none"` does not work reliably with more than one
   custom relay") no longer applies on iroh 1.0.

3. **Direct-path upgrade does not need discovery either.** Hole punching is
   negotiated over the established relay path (NAT traversal candidates are
   exchanged on the connection itself), so the relay hint is sufficient to
   bootstrap a direct P2P connection.

### Home-relay semantics (the one caveat)

- An iroh endpoint has **one home relay at a time**, chosen as the
  fastest-probing relay in its `RelayMap` by net_report, and keeps a
  persistent connection only to it (`socket/transports/relay/actor.rs`;
  non-home relay connections close after an inactivity timeout).
- Relay servers are stateless and independent: a relay only delivers packets
  to endpoints currently connected to it. Traffic sent via a relay the server
  is not connected to is dropped.
- net_report re-probes every **20–26 s** (`new_re_stun_timer` in
  `socket.rs`), so after its home relay dies the server re-homes onto another
  configured relay within roughly 30 s.

Consequence: a client configured with only a **subset** of the server's
relays can reach the server only while the server's current home relay is in
that subset. Public discovery was the mechanism that could rescue that case
(the client would learn the server's current home relay). Hence the guidance
in [SELF-HOSTING.md](SELF-HOSTING.md): configure clients with the full relay
list, or set an explicit shared `--discovery` server if partial lists are
required. An explicitly configured discovery URL is still honored even with
custom relays.

## Empirical verification

`test-scripts/run_relay_failover_e2e.sh` (fully offline: two local
`iroh-relay --dev` instances, relay-only, server always configured with both
relays, internet discovery auto-disabled). All scenarios pass:

| # | Scenario | Result |
|---|----------|--------|
| A0 | Both relays down before start → server fails to come online | pass |
| A1 | Relay1 down before start → client with both relays connects via relay2 | pass |
| A2 | Client with only the live relay connects | pass |
| A3 | Client with only the dead relay fails to connect | pass |
| B1 | Both relays up → connect + echo traffic | pass |
| B2 | Relay carrying the connection killed → restarted client fails over once the server re-homes (observed: success on attempt 3, ≈ the 20–26 s re-probe cycle) | pass |
| B3 | Surviving relay killed too → new client fails | pass |
| B4 | Both relays restarted → server recovers, new client connects | pass |

`test-scripts/run_e2e.sh --relay-url <r1> --relay-url <r2>` (TCP + UDP) also
passes against two local relays with no internet discovery, in both normal
and `--relay-only` mode — previously the multi-relay configuration required
public discovery.

Log line confirming the new behavior on endpoint creation:

```
INFO tunnel_rs::iroh_mode::endpoint] Internet discovery disabled (custom relays configured)
```

mDNS local-network discovery remains enabled (unchanged); `--relay-only`
still skips all discovery including mDNS (unchanged).
