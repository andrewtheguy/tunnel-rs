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

## Overview

tunnel-rs uses iroh for establishing tunnels, providing NAT traversal with relay fallback, automatic discovery, and client authentication. Clients keep ephemeral iroh identities while proving possession of separately authorized Ed25519 keys, so transport identity and application access control remain independent.

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

### Feature Flags

Relay-only is a **CLI-only** flag that forces connections through relay servers instead of attempting direct connections. It is intended for testing or special scenarios and is **not supported in config files** to avoid accidental activation. See `tunnel-rs --help` for usage.

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

# Configuration

## Persistent Server Identity

Server identity is required. Configure a persistent identity for the **server** so clients can reconnect reliably:

```bash
# Generate key and output EndpointId
tunnel-rs generate-server-key --output ./server.key

# Show EndpointId for existing key
tunnel-rs show-server-id --secret-file ./server.key
```

Then reference the key in your server config or CLI:

**CLI**:
```bash
tunnel-rs server \
  --secret-file ./server.key \
  --allowed-tcp 127.0.0.0/8 \
  --authorized-keys-file ./authorized_keys
```

**Config file** (`server.toml`):
```toml
[iroh]
secret_file = "./server.key"
```

> **Note:** Clients use ephemeral identities by default. Only the server needs a persistent key to maintain a stable EndpointId that clients can connect to.

## Authentication

Clients authenticate after the iroh connection is established by signing a
fresh server challenge with an Ed25519 key. This authentication identity is
independent of the client's ephemeral iroh EndpointId.

```bash
# On the client: write a compact private key and print its public entry
tunnel-rs generate-auth-key \
  --output ~/.config/tunnel-rs/client.key \
  --comment "alice laptop"
# Output: ed25519 <base64-public-key> alice laptop
```

The private key file is compact and self-describing:

```text
# public key: ed25519 <base64-public-key> alice laptop
tunnel-rs-ed25519-private-key-v1:<base64-private-seed>
```

Add the generated public entry to the server's `authorized_keys` file. Comments
at the end identify clients and are included in successful-authentication logs:

```text
# Blank lines and comment lines are ignored.
ed25519 AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA= alice laptop
ed25519 BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB= bob workstation
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

## Self-Hosting

For custom relay servers and fully independent operation without public infrastructure, see [docs/SELF-HOSTING.md](docs/SELF-HOSTING.md). Configuring custom relays disables internet discovery automatically.

---

# Usage

## Architecture

### TCP Tunneling

```
+-----------------+        +-----------------+        +-----------------+        +-----------------+
| SSH Client      |  TCP   | client          |  iroh  | server          |  TCP   | SSH Server      |
|                 |<------>| (local:2222)    |<======>|                 |<------>| (client req)    |
|                 |        |                 |  QUIC  |                 |        |                 |
+-----------------+        +-----------------+        +-----------------+        +-----------------+
     Client Side                                            Server Side
```

For deeper architecture diagrams and protocol flows, see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Quick Start

### 1. Setup (One-Time)

Generate the server identity and a client authentication key:

```bash
# On server machine - generate persistent identity
tunnel-rs generate-server-key --output ./server.key
# Output: EndpointId: 2xnbkpbc7izsilvewd7c62w7wnwziacmpfwvhcrya5nt76dqkpga

# On the client machine; send the printed public entry to the server admin
tunnel-rs generate-auth-key --output ./client.key --comment "alice laptop" \
  > client.authorized_key

# On the server machine
cat client.authorized_key >> authorized_keys
```

### 2. TCP Tunnel (e.g., SSH)

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

### 3. UDP Tunnel (e.g., WireGuard/Game/DNS)

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

## CLI Options

### server

| Option | Default | Description |
|--------|---------|-------------|
| `--config`, `-c` | - | Path to TOML config file |
| `--default-config` | false | Load config from `~/.config/tunnel-rs/server.toml` |
| `--config-stdin` | false | Read JSON config from stdin for automation/IPC (use `-c` for normal usage) |

### server iroh

| Option | Default | Description |
|--------|---------|-------------|
| `--allowed-tcp` | - | Allowed TCP networks in CIDR notation (repeatable) |
| `--allowed-udp` | - | Allowed UDP networks in CIDR notation (repeatable) |
| `--authorized-keys-file` | required | Path to SSH-like file containing authorized Ed25519 public keys |
| `--max-sessions` | 100 | Maximum concurrent sessions |
| `--secret-file` | - | Path to secret key file for persistent server identity |
| `--relay-url` | public | Custom relay server URL(s), repeatable |
| `--relay-only` | false | Force all traffic through relay (CLI-only; not supported in config files) |

**Environment variables** (for containers and automation scripts):

| Env Var | Description |
|---------|-------------|
| `TUNNEL_RS_AUTHORIZED_KEYS_FILE` | Path to the server authorized-keys file |
| `TUNNEL_RS_SECRET` | Base64-encoded secret key for persistent server identity (use this or `--secret-file`) |

### client

| Option | Default | Description |
|--------|---------|-------------|
| `--config`, `-c` | - | Path to TOML config file |
| `--default-config` | false | Load config from `~/.config/tunnel-rs/client.toml` |
| `--config-stdin` | false | Read JSON config from stdin for automation/IPC (use `-c` for normal usage) |

### client iroh

| Option | Default | Description |
|--------|---------|-------------|
| `--server-node-id`, `-n` | required | EndpointId of the server |
| `--source`, `-s` | required | Source address to request from server (tcp://host:port or udp://host:port) |
| `--target`, `-t` | required | Local address to listen on |
| `--private-key-file` | required | Path to compact Ed25519 authentication private key |
| `--relay-url` | public | Custom relay server URL(s), repeatable |
| `--relay-only` | false | Force all traffic through relay (CLI-only; not supported in config files) |

**Environment variables** (for containers and automation scripts):

| Env Var | Description |
|---------|-------------|
| `TUNNEL_RS_PRIVATE_KEY_FILE` | Path to the compact Ed25519 authentication private key |

## Configuration Files

Use `--default-config` to load from the default location, or `-c <path>` for a custom path (both TOML). For normal usage, prefer config files so your settings are saved and reusable. The `--config-stdin` flag is intended for automation and IPC — it accepts JSON (self-delimiting, so the caller does not need to close stdin). Only one of these may be used at a time. Configuration uses the `[iroh]` section.

> **Security:** Authentication private keys are referenced by path and are not embedded in TOML. The server endpoint `secret` is also rejected in TOML; use `secret_file` instead. Inline server endpoint secrets remain available only through `TUNNEL_RS_SECRET` or JSON `--config-stdin` automation.

**Default locations:**
- Server: `~/.config/tunnel-rs/server.toml`
- Client: `~/.config/tunnel-rs/client.toml`

> **Note:** `--relay-only` is intentionally **CLI-only** and is not supported in config files to avoid accidental activation.

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

### Server Config Example

```toml
# Example server configuration (iroh mode)

# Required: validates config matches CLI command
role = "server"
mode = "iroh"

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
# Load from default location (mode inferred from config)
tunnel-rs server --default-config

# Load from custom path
tunnel-rs server -c ./my-server.toml

```

### Client Config Example

```toml
# Example client configuration (iroh mode)

# Required: validates config matches CLI command
role = "client"
mode = "iroh"

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
# Load from default location (mode inferred from config)
tunnel-rs client --default-config

# Load from custom path
tunnel-rs client -c ./my-client.toml

# Automation/IPC: pass JSON config via stdin (no need to close stdin)
# JSON is self-delimiting, so the parent process can keep stdin open.
```

Example: spawning a client with `--config-stdin` from Python:

```python
import json, socket, subprocess, time

config = {
    "role": "client",
    "mode": "iroh",
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

---

# Utility Commands

## generate-auth-key

Generate a compact Ed25519 client authentication key and print the matching
server authorized-key entry:

```bash
tunnel-rs generate-auth-key \
  --output ~/.config/tunnel-rs/client.key \
  --comment "alice laptop"

# Overwrite an existing key file
tunnel-rs generate-auth-key --output ./client.key --force
```

The private key file is created with `0600` permissions on Unix. Copy the
printed `ed25519 ...` line into the server's `authorized_keys` file.

## generate-server-key

*For iroh mode.*

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

## show-server-id

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

## How It Works

### iroh Mode
1. Server creates an iroh endpoint (with internet discovery on default relays; custom relays disable it and clients use relay hints instead)
2. Server publishes its address via Pkarr/DNS (default relays only)
3. Client resolves the server via discovery, or reaches it directly through the configured relays
4. **QUIC handshake:** Connection uses the fixed ALPN `mf/4` shared by all peers
5. **Authentication phase:** Client opens the dedicated auth stream; the server sends a fresh random challenge
6. **Client proves key possession:** Client signs the domain-separated challenge and sends its public key and signature; the server checks the key against `authorized_keys` and verifies the signature (10s timeout)
   - *If authentication fails, the connection is closed and steps 7–9 do not occur*
7. **Source request phase:** Client opens source stream with `SourceRequest`
8. Server validates source against allowed networks and responds
9. If accepted, traffic forwarding begins
