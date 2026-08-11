# tunnel-rs

**Cross-platform Secure Peer-to-Peer TCP/UDP port forwarding with NAT traversal.**

Tunnel-rs enables you to forward TCP and UDP traffic between machines without requiring public IP addresses, open ports, or VPN infrastructure. It establishes direct encrypted connections between peers using modern P2P networking techniques.

> [!IMPORTANT]
> **Project Goal:** This tool provides a convenient way to connect to different networks for **development or homelab purposes** without the hassle and security risk of opening a port. It is **not** meant for production setups or designed to be performant at scale.

> [!WARNING]
> **No Backward Compatibility (Pre-1.0):** During initial development before version 1.0, no backward compatibility or migration path is provided between minor versions (e.g., 0.1.x to 0.2.x). Expect to regenerate server keys and rebuild client/server configurations when upgrading in between minor versions.

**Features:**
- **No account or registration required** — Just download and run
- **No publicly accessible IPs or port forwarding required** — Automatic NAT hole punching
- **Full TCP and UDP support** — Seamlessly tunnel any TCP or UDP traffic
- **Cross-platform** — Works on Linux, macOS, and Windows
- **No root required** — Runs as unprivileged user
- **End-to-end encryption** via QUIC/TLS 1.3
- **NAT traversal** with automatic NAT hole punching and relay fallback

**Use Cases:**
- **SSH access** to machines behind NAT/firewalls
- **UDP Tunneling** — A key advantage over AWS SSM and `kubectl port-forward` which typically lack UDP support. Ideal for:
  - WireGuard/OpenVPN over P2P
  - Game servers (Valheim, Minecraft Bedrock, etc.)
  - VoIP applications and WebRTC
  - Accessing UDP services in Kubernetes (bypassing the [7+ year old limitation in `kubectl`](https://github.com/kubernetes/kubernetes/issues/47862) without complex sidecar workarounds)
- **Simpler Alternative to SSM For Staging Environment Access Purposes** — Great for ad-hoc access without configuring AWS agents or IAM users. **Note:** Not intended for production; it is not battle-tested for enterprise use and lacks integration with cloud security policies (IAM, auditing).
- **Remote Desktop** access (RDP/VNC over TCP) without port forwarding
- **Secure Service Exposure** (HTTP servers, databases, etc.) without public infrastructure
- **Development and Testing** of TCP/UDP services across network boundaries
- **Homelab Networking** — Connecting distributed homelab nodes or accessing local services remotely without complex VPN setups or public IP requirements
- **Cross-platform Tunneling** for both TCP and UDP workflows (including Windows endpoints)

## How It Works

tunnel-rs uses iroh for establishing tunnels, providing NAT traversal with relay fallback, automatic discovery, and client authentication. Clients keep ephemeral iroh identities while proving possession of separately authorized Ed25519 keys, so transport identity and application access control remain independent.

```
+-----------------+        +-----------------+        +-----------------+        +-----------------+
| SSH Client      |  TCP   | client          |  iroh  | server          |  TCP   | SSH Server      |
|                 |<------>| (local:2222)    |<======>|                 |<------>| (client req)    |
|                 |        |                 |  QUIC  |                 |        |                 |
+-----------------+        +-----------------+        +-----------------+        +-----------------+
     Client Side                                            Server Side
```

1. **Transport:** Client and server find each other and connect over iroh — see
   [relays and address lookup](https://github.com/flexaccessdev/iroh-common-architecture/blob/main/relays-and-address-lookup.md)
   for discovery, relays, and NAT traversal
2. **QUIC handshake:** Connection uses the fixed ALPN `mf/4` shared by all peers
3. **Authentication phase:** Client opens the dedicated auth stream; the server sends a fresh random challenge
4. **Client proves key possession:** Client signs the domain-separated challenge and sends its public key and signature; the server checks the key against `authorized_keys` and verifies the signature (10s timeout)
   - *If authentication fails, the connection is closed and steps 5–7 do not occur*
5. **Source request phase:** Client opens source stream with `SourceRequest`
6. Server validates source against allowed networks and responds
7. If accepted, traffic forwarding begins

> See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for detailed diagrams and technical deep-dives.

## Installation

You only need the binary in your PATH; no runtime dependencies or package managers are required.

**Linux & macOS:**
```bash
curl -sSL https://andrewtheguy.github.io/tunnel-rs/install.sh | bash
```

**Windows:**
```powershell
irm https://andrewtheguy.github.io/tunnel-rs/install.ps1 | iex
```

This installs `tunnel-rs`.

<details>
<summary>Advanced installation options</summary>

Install with custom release tag:
```bash
# Linux/macOS
curl -sSL https://andrewtheguy.github.io/tunnel-rs/install.sh | bash -s <RELEASE_TAG>
```

```powershell
# Windows
& ([scriptblock]::Create((irm https://andrewtheguy.github.io/tunnel-rs/install.ps1))) <RELEASE_TAG>
```

By default the installer pulls the latest **stable** release. Use `--prerelease` for the newest prerelease, or pass an explicit tag to pin to a specific build:

```bash
# Linux/macOS - latest prerelease
curl -sSL https://andrewtheguy.github.io/tunnel-rs/install.sh | bash -s -- --prerelease

# Linux/macOS - pin to specific tag
curl -sSL https://andrewtheguy.github.io/tunnel-rs/install.sh | bash -s 20251210172710
```

```powershell
# Windows - latest prerelease
& ([scriptblock]::Create((irm https://andrewtheguy.github.io/tunnel-rs/install.ps1))) -PreRelease

# Windows - pin to specific tag
& ([scriptblock]::Create((irm https://andrewtheguy.github.io/tunnel-rs/install.ps1))) 20251210172710
```

> **Note:** Prerelease artifacts may not include Windows binaries. If unavailable, use a stable release tag or build from source.

</details>

### From Source

```bash
cargo install --path .
```

### Supported Platforms

tunnel-rs works on Linux, macOS, and Windows.

Official prebuilt release artifacts currently include:
- **Linux** (x86_64, ARM64)
- **macOS** (Apple Silicon)
- **Windows** (x86_64, stable releases)

Intel macOS is supported when building from source.

### Docker & Kubernetes

Container images are available at `ghcr.io/andrewtheguy/tunnel-rs`.

Access services running in Docker or Kubernetes remotely — without opening ports, configuring ingress, or requiring `kubectl`. See [container-deploy/](container-deploy/) for Docker Compose and Kubernetes configurations.

---

# Quick Start

End to end in three steps. [Setup](#setup) below explains what each key is for
and where it lives.

## 1. Generate the keys (one-time)

Client key first: the server will not accept a connection until it already holds
that client's public entry.

```bash
# 1. On the client machine; send the printed public entry to the server admin
tunnel-rs generate-auth-key --output ./client.key --comment "alice laptop" \
  > client.authorized_key

# 2. On the server machine - authorize that entry
cat client.authorized_key >> authorized_keys

# 3. On the server machine - generate the persistent identity to dial
tunnel-rs generate-server-key --output ./server.key
# Output: EndpointId: 2xnbkpbc7izsilvewd7c62w7wnwziacmpfwvhcrya5nt76dqkpga
```

## 2. TCP Tunnel (e.g., SSH)

**Server** (on server — waits for client connections):
```bash
tunnel-rs server \
  --secret-file ./server.key \
  --allowed-tcp 127.0.0.0/8 \
  --authorized-keys-file ./authorized_keys
```

Output:
```
EndpointId: 2xnbkpbc7izsilvewd7c62w7wnwziacmpfwvhcrya5nt76dqkpga
Authorized client keys: 1
Waiting for clients to connect...
```

**Client** (on client — requests source from server):
```bash
tunnel-rs client \
  --server-node-id <SERVER_ENDPOINT_ID> \
  --source tcp://127.0.0.1:22 \
  --target 127.0.0.1:2222 \
  --private-key-file ./client.key
```

Then connect: `ssh -p 2222 user@127.0.0.1`

## 3. UDP Tunnel (e.g., WireGuard/Game/DNS)

**Server**:
```bash
tunnel-rs server \
  --secret-file ./server.key \
  --allowed-udp 127.0.0.0/8 \
  --authorized-keys-file ./authorized_keys
```

**Client**:
```bash
tunnel-rs client \
  --server-node-id <SERVER_ENDPOINT_ID> \
  --source udp://127.0.0.1:51820 \
  --target 0.0.0.0:51820 \
  --private-key-file ./client.key
```

---

# Setup

The reference behind [Quick Start](#quick-start). Two unrelated keys are
involved: the **client authentication key**, which proves who is connecting, and
the **server identity key**, which gives the server a stable EndpointId to dial.
The client's comes first, since the server will not accept a connection until it
already holds that client's public entry.

## Client Authentication Key

Clients authenticate after the iroh connection is established by signing a fresh
server challenge with an Ed25519 key. This authentication identity is
independent of the client's ephemeral iroh EndpointId.

```bash
# On the client: write a compact private key and print its public entry
tunnel-rs generate-auth-key \
  --output ~/.config/tunnel-rs/client.key \
  --comment "alice laptop"
# Output: tunnelrsv1authpub:<urlsafe-base64-public-key> alice laptop
```

The private key file is compact and self-describing:

```text
# created: 2024-09-13T22:22:33Z
# public key: tunnelrsv1authpub:<urlsafe-base64-public-key> alice laptop
tunnelrsv1authsecret:<urlsafe-base64-private-seed>
```

Both tokens carry unpadded [URL-safe base64](https://datatracker.ietf.org/doc/html/rfc4648#section-5)
of 32 raw key bytes, so a key is a single copy-pasteable word.

Add the generated public entry to the server's `authorized_keys` file. A single
space separates the key from its comment, and the comment runs to end of line;
it identifies the client in successful-authentication logs:

```text
# Blank lines and comment lines are ignored.
tunnelrsv1authpub:1bhGIken5UAXTkC7cABRzM4cE98xZl3tilGyYZsoyP8 alice laptop
tunnelrsv1authpub:sHMOUCikL2-gX4UbwMCRjOSmdgjhQWCYIqCcP86tGHQ bob workstation
```

### Configuration File

**Server** (`server.toml`):
```toml
[iroh]
authorized_keys_file = "/etc/tunnel-rs/authorized_keys"
```

**Client** (`client.toml`):
```toml
[iroh]
private_key_file = "~/.config/tunnel-rs/client.key"
```

## Server Identity

Required. Only the server needs a persistent key — clients use ephemeral iroh
identities — and it is what keeps the server's EndpointId stable across restarts
so clients can reconnect reliably:

```bash
# Generate key and output EndpointId
tunnel-rs generate-server-key --output ./server.key

# Show EndpointId for existing key
tunnel-rs show-server-id --secret-file ./server.key
```

Then point the server at it — `--secret-file ./server.key` on the CLI, or:

**Config file** (`server.toml`):
```toml
[iroh]
secret_file = "./server.key"
```

---

# Usage

## CLI Options

### server

| Option | Default | Description |
|--------|---------|-------------|
| `--config`, `-c` | - | Path to TOML config file |
| `--default-config` | false | Load config from `~/.config/tunnel-rs/server.toml` |
| `--config-stdin` | false | Read JSON config from stdin for automation/IPC (use `-c` for normal usage) |
| `--allowed-tcp` | - | Allowed TCP networks in CIDR notation (repeatable) |
| `--allowed-udp` | - | Allowed UDP networks in CIDR notation (repeatable) |
| `--authorized-keys-file` | required | Path to SSH-like file containing authorized Ed25519 public keys |
| `--max-sessions` | 100 | Maximum concurrent sessions |
| `--secret-file` | - | Path to secret key file for persistent server identity |
| `--relay-url` | public | Custom relay server URL(s), repeatable. Every one must be reachable at startup |
| `--relay-auth-token` | - | Shared bearer token for the custom relay(s); requires `--relay-url` |
| `--relay-only` | false | Force all traffic through relay (CLI-only; not supported in config files) |

**Environment variables** (for containers and automation scripts):

| Env Var | Description |
|---------|-------------|
| `TUNNEL_RS_AUTHORIZED_KEYS_FILE` | Path to the server authorized-keys file |
| `TUNNEL_RS_SECRET` | Base64-encoded secret key for persistent server identity (use this or `--secret-file`) |
| `TUNNEL_RS_RELAY_AUTH_TOKEN` | Shared bearer token for the custom relay(s) (use this instead of `--relay-auth-token` to keep it out of the process list) |

### client

| Option | Default | Description |
|--------|---------|-------------|
| `--config`, `-c` | - | Path to TOML config file |
| `--default-config` | false | Load config from `~/.config/tunnel-rs/client.toml` |
| `--config-stdin` | false | Read JSON config from stdin for automation/IPC (use `-c` for normal usage) |
| `--server-node-id`, `-n` | required | EndpointId of the server |
| `--source`, `-s` | required | Source address to request from server (tcp://host:port or udp://host:port) |
| `--target`, `-t` | required | Local address to listen on |
| `--private-key-file` | required | Path to compact Ed25519 authentication private key |
| `--relay-url` | public | Custom relay server URL(s), repeatable. Every one must be reachable at startup |
| `--relay-auth-token` | - | Shared bearer token for the custom relay(s); requires `--relay-url` |
| `--relay-only` | false | Force all traffic through relay (CLI-only; not supported in config files) |

**Environment variables** (for containers and automation scripts):

| Env Var | Description |
|---------|-------------|
| `TUNNEL_RS_PRIVATE_KEY_FILE` | Path to the compact Ed25519 authentication private key |
| `TUNNEL_RS_RELAY_AUTH_TOKEN` | Shared bearer token for the custom relay(s) (use this instead of `--relay-auth-token` to keep it out of the process list) |

## Configuration Files

Use `--default-config` to load from the default location, or `-c <path>` for a custom path (both TOML). For normal usage, prefer config files so your settings are saved and reusable. The third form, [`--config-stdin`](#json-config-via-stdin), is for automation. Only one of the three may be used at a time. Whichever form you use, `role` (`"server"` or `"client"`) is a required **top-level** field that is checked against the subcommand; every other setting goes under the `[iroh]` section.

> **Security:** Authentication private keys are referenced by path and are not embedded in TOML. The server endpoint `secret` is also rejected in TOML; use `secret_file` instead. Inline server endpoint secrets remain available only through `TUNNEL_RS_SECRET` or JSON `--config-stdin` automation.

**Default locations:**
- Server: `~/.config/tunnel-rs/server.toml`
- Client: `~/.config/tunnel-rs/client.toml`

> **Note:** `--relay-only` — which forces connections through the relays instead of attempting direct ones — is intentionally **CLI-only** and is not supported in config files to avoid accidental activation.

### Server Config Example

```toml
# Example server configuration

# Required: validates config matches CLI command
role = "server"

[iroh]
secret_file = "./server.key"
# relay_urls = ["https://relay.example.com"]
max_sessions = 100

# Ed25519 public keys, one per line with optional trailing comments
authorized_keys_file = "/etc/tunnel-rs/authorized_keys"

[iroh.allowed_sources]
tcp = ["127.0.0.0/8", "192.168.0.0/16"]
udp = ["10.0.0.0/8"]
```

> [!NOTE]
> See [`server.toml.example`](server.toml.example) for the full example.

```bash
# Load from default location
tunnel-rs server --default-config

# Load from custom path
tunnel-rs server -c ./my-server.toml
```

### Client Config Example

```toml
# Example client configuration

# Required: validates config matches CLI command
role = "client"

[iroh]
server_node_id = "2xnbkpbc7izsilvewd7c62w7wnwziacmpfwvhcrya5nt76dqkpga"
request_source = "tcp://127.0.0.1:22"
target = "127.0.0.1:2222"
# relay_urls = ["https://relay.example.com"]

# Compact Ed25519 authentication private key
private_key_file = "~/.config/tunnel-rs/client.key"
```

> [!NOTE]
> See [`client.toml.example`](client.toml.example) for the full example.

```bash
# Load from default location
tunnel-rs client --default-config

# Load from custom path
tunnel-rs client -c ./my-client.toml
```

### JSON Config via stdin

For automation and IPC, `--config-stdin` takes the same structure as JSON. It is
self-delimiting, so the parent process can keep stdin open after writing it:

```python
import json, socket, subprocess, time

config = {
    "role": "client",
    "iroh": {
        "server_node_id": "<SERVER_NODE_ID>",
        "private_key_file": "/run/secrets/tunnel-rs-client.key",
        "request_source": "tcp://127.0.0.1:22",
        "target": "127.0.0.1:2222",
    }
}

proc = subprocess.Popen(
    ["tunnel-rs", "client", "--config-stdin"],
    stdin=subprocess.PIPE,
)
proc.stdin.write(json.dumps(config).encode())
proc.stdin.flush()  # config is parsed immediately, no need to close stdin

# wait for the forwarded port to be ready
for attempt in range(10):
    try:
        with socket.create_connection(("127.0.0.1", 2222), timeout=2):
            print("tunnel is up")
            break
    except OSError:
        time.sleep(1)
else:
    raise RuntimeError("tunnel failed to start")

input("press enter to quit..")
proc.terminate()
```

### Overriding Config Values

CLI arguments take precedence over config file values. Use `--default-config` with CLI arguments to override specific fields:

```bash
# Use config but override source and target
tunnel-rs client --default-config \
  --source tcp://localhost:3000 \
  --target 127.0.0.1:8080

# Use config but override allowed networks
tunnel-rs server --default-config \
  --allowed-tcp 10.0.0.0/8
```

This lets you keep common settings (keys, relay URLs) in the config file while varying per-session options on the command line. You can also omit fields like `source` and `target` from the config entirely and provide them only via CLI.

### Transport Tuning

QUIC transport parameters can be tuned via an optional `[iroh.transport]` section in either config file. These are **config-only** (no CLI flags) and all have sensible defaults — only set them if you need to.

```toml
[iroh.transport]
# Congestion controller: "cubic" (default), "bbr", or "newreno"
congestion_controller = "cubic"
# QUIC per-stream receive window in bytes (default: 67108864 = 64MB; range 1024-67108864)
receive_window = 67108864
# QUIC send window in bytes (default: 67108864 = 64MB; range 1024-67108864)
send_window = 67108864
# QUIC ACK-eliciting threshold (default: unset = iroh/quinn default cadence; range 0-65535)
# Leave unset unless you have measured a benefit. 0 requests ACKs for every packet.
# ack_eliciting_threshold = 2
```

The connection-level receive window uses iroh's default. If `send_window` is omitted but `receive_window` is set, the send window defaults to twice the stream receive window, capped at the 64MB default. See [`server.toml.example`](server.toml.example) and [`client.toml.example`](client.toml.example) for the annotated reference.

## Utility Commands

### generate-auth-key

Generate a compact Ed25519 client authentication key and print the matching
server authorized-key entry:

```bash
tunnel-rs generate-auth-key \
  --output ~/.config/tunnel-rs/client.key \
  --comment "alice laptop"

# Overwrite an existing key file
tunnel-rs generate-auth-key --output ./client.key --force

# Without --output the key file goes to stdout (the authorized-key entry to stderr)
tunnel-rs generate-auth-key --comment "alice laptop" > ./client.key
```

With `--output` the private key file is created with `0600` permissions on Unix
and the authorized-key entry is printed to stdout, so
`generate-auth-key --output client.key > authorized_keys` captures the entry.
Without `--output` (or with `--output -`) the roles swap: the key file goes to
stdout and the authorized-key entry to stderr — remember to restrict
permissions yourself when redirecting to a file. Either way, copy the printed
`tunnelrsv1authpub:...` line into the server's `authorized_keys` file.

### generate-server-key

```bash
tunnel-rs generate-server-key --output ./server.key

# Emit a new keypair as JSON without writing a file
tunnel-rs generate-server-key --json
# Output: {"public_key":"...","private_key":"..."}

# Write the key to stdout instead of a file (e.g. to capture it in a script)
tunnel-rs generate-server-key --output -

# Overwrite an existing key file
tunnel-rs generate-server-key --output ./server.key --force
```

Without `--json`, the secret key is written to the required `--output` target (created with `0600` permissions on Unix), and the EndpointId is printed to stdout. Use `-` as the output to write the key to stdout instead — in that case the EndpointId is printed to stderr so it stays off the key stream. Existing files are not overwritten unless `--force` is passed. With `--json`, no file is written and both keys are emitted to stdout.

### show-server-id

```bash
tunnel-rs show-server-id --secret-file ./server.key
```

## Security

- All traffic is encrypted using QUIC/TLS 1.3
- The EndpointId is a public key that identifies the server
- The QUIC ALPN is a fixed value (`mf/4`) shared by all peers; access control is handled by Ed25519 public-key authentication.
- Clients authenticate immediately after the QUIC connection by signing a fresh random challenge on a dedicated auth stream. The authentication key is independent of the ephemeral iroh client identity.
- Secret key files are created with `0600` permissions (Unix) and appropriate permissions on Windows
- Treat endpoint and authentication private-key files like passwords

## Exit Codes (Client Mode)

The client process uses categorized exit codes so wrapper scripts can distinguish transient failures (retry) from permanent errors (stop).

| Exit Code | Meaning | Retry? |
|-----------|---------|--------|
| 0 | Success | N/A |
| 1 | General/unexpected error | Use judgment |
| 2 | Configuration error (invalid arguments, bad key format, missing fields) | No — fix configuration |
| 3 | Authentication failure (signature rejected, auth timeout) | No — fix credentials |
| 10 | Connection establishment failed (timeout, relay failure, server unreachable) | Only if it worked before |
| 11 | Connection lost after tunnel was established | Yes — always retry |

Example retry wrapper script:

```bash
#!/bin/bash
succeeded_before=false
while true; do
    tunnel-rs client --default-config
    code=$?
    case $code in
        0)   echo "Clean exit"; break ;;
        2|3) echo "Unrecoverable error (exit $code), not retrying"; exit $code ;;
        10)
            if [ "$succeeded_before" = true ]; then
                echo "Connection failed (previously connected), retrying in 5s..."
                sleep 5
            else
                echo "Never connected successfully (exit 10), not retrying"
                exit $code
            fi
            ;;
        11)  succeeded_before=true
             echo "Connection lost, retrying in 5s..."
             sleep 5 ;;
        *)   echo "Unexpected error (exit $code), retrying in 10s..."; sleep 10 ;;
    esac
done
```

---

# Self-Hosting

Optional. Out of the box tunnel-rs uses n0's public relays and needs no
infrastructure of your own — this section is for when you want none of theirs
either.

For custom relay servers and fully independent operation without public
infrastructure, see
**[iroh-common-architecture](https://github.com/flexaccessdev/iroh-common-architecture)** —
the iroh transport docs shared with
[ezvpn](https://github.com/flexaccessdev/ezvpn) and
[flextunnel](https://github.com/flexaccessdev/flextunnel):
[self-hosting](https://github.com/flexaccessdev/iroh-common-architecture/blob/main/self-hosting.md)
(running your own relay, including the single-port Cloudflare Tunnel setup) and
[relays and address lookup](https://github.com/flexaccessdev/iroh-common-architecture/blob/main/relays-and-address-lookup.md)
(the design). tunnel-rs is the **reference program for relay-only setups** — use
its relay-only e2e script to validate a freshly deployed relay.

Two behaviors are worth reading up on before configuring relays, both covered in
[relays and address lookup](https://github.com/flexaccessdev/iroh-common-architecture/blob/main/relays-and-address-lookup.md):
custom relays disable internet discovery (configure **both** sides with the full
relay list), and every configured relay must come online at startup or the
process refuses to start.

The one tunnel-rs-specific knob is `--relay-only`, which forces every byte
through the relays instead of attempting direct paths. It is **CLI-only** and not
accepted in config files, to avoid accidental activation.
