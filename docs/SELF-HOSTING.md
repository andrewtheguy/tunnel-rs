# Self-Hosting Iroh Infrastructure

This guide covers custom relay and discovery servers for port-forwarding modes.

## Custom Relay

```bash
tunnel-rs server --relay-url https://relay.example.com --allowed-tcp 127.0.0.0/8 --auth-tokens "$AUTH_TOKEN"
tunnel-rs client --relay-url https://relay.example.com --server-node-id <ID> --source tcp://127.0.0.1:22 --target 127.0.0.1:2222 --auth-token "$AUTH_TOKEN"
```

## Custom Discovery Server

```bash
tunnel-rs server --dns-server https://dns.example.com/pkarr --secret-file ./server.key --allowed-tcp 127.0.0.0/8 --auth-tokens "$AUTH_TOKEN"
tunnel-rs client --dns-server https://dns.example.com/pkarr --server-node-id <ID> --source tcp://127.0.0.1:22 --target 127.0.0.1:2222 --auth-token "$AUTH_TOKEN"
```

## Disable DNS discovery

```bash
tunnel-rs server --dns-server none --relay-url https://relay.example.com --allowed-tcp 127.0.0.0/8 --auth-tokens "$AUTH_TOKEN"
tunnel-rs client --dns-server none --relay-url https://relay.example.com --server-node-id <ID> --source tcp://127.0.0.1:22 --target 127.0.0.1:2222 --auth-token "$AUTH_TOKEN"
```
