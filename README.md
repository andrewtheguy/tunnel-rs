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

tunnel-rs uses iroh for establishing tunnels, providing NAT traversal with relay fallback, automatic discovery, and client authentication. Clients use ephemeral identities by default, so multiple clients can connect to the same server with the same auth token — each session is independent.

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
tunnel-rs server --secret-file ./server.key --allowed-tcp 127.0.0.0/8 --auth-tokens "$AUTH_TOKEN" --alpn-token "$ALPN_TOKEN"
```

**Config file** (`server.toml`):
```toml
[iroh]
secret_file = "./server.key"
```

> **Note:** Clients use ephemeral identities by default. Only the server needs a persistent key to maintain a stable EndpointId that clients can connect to.

## Authentication

Iroh mode requires authentication using pre-shared tokens. Clients must provide a valid token to connect.

**Token Format:**
- Exactly 47 characters
- Starts with `i` (for iroh)
- Remaining 46 characters are Base64URL-encoded (no padding)
- Decoded payload: 32 random bytes + 2-byte CRC16-CCITT-FALSE checksum

The CRC16 checksum detects all single-byte errors in the token payload.

Generate tokens with: `tunnel-rs generate-token`

### Token Management

```bash
# Generate a valid token
AUTH_TOKEN=$(tunnel-rs generate-token)
echo $AUTH_TOKEN  # Share this with authorized clients

# Generate multiple tokens
tunnel-rs generate-token -c 5
```

### Multiple Tokens (Server)

```bash
# Multiple --auth-tokens flags
tunnel-rs server \
  --allowed-tcp 127.0.0.0/8 \
  --auth-tokens "token-for-alice" \
  --auth-tokens "token-for-bob"

# Or use a file (one token per line, # comments allowed)
tunnel-rs server \
  --allowed-tcp 127.0.0.0/8 \
  --auth-tokens-file /etc/tunnel-rs/auth_tokens.txt
```

**Example `auth_tokens.txt`:**
```text
# Alice's token (generate with: tunnel-rs generate-token)
iXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX

# Bob's token
iYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYY
```

### Configuration File

> **Security:** Plaintext tokens and secrets are **not allowed** in TOML config files. Use the `_file` variants (e.g., `auth_tokens_file`, `auth_token_file`, `alpn_token_file`, `secret_file`) in config files. Plaintext values are accepted via `--config-stdin` (JSON) and CLI arguments since those are transient.

**Server** (`server.toml`):
```toml
[iroh]
auth_tokens_file = "/etc/tunnel-rs/auth_tokens.txt"
```

**Client** (`client.toml`):
```toml
[iroh]
auth_token_file = "~/.config/tunnel-rs/token.txt"
```

## Self-Hosting

For custom relay servers, DNS discovery, or fully independent operation without public infrastructure, see [docs/SELF-HOSTING.md](docs/SELF-HOSTING.md).

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

Generate a server key and create an authentication token:

```bash
# On server machine - generate persistent identity
tunnel-rs generate-server-key --output ./server.key
# Output: EndpointId: 2xnbkpbc7izsilvewd7c62w7wnwziacmpfwvhcrya5nt76dqkpga

# Create a shared authentication token
# Share this token with authorized clients
AUTH_TOKEN=$(tunnel-rs generate-token)
echo $AUTH_TOKEN

# Create an ALPN token (shared between server and all clients)
ALPN_TOKEN=$(tunnel-rs generate-token --alpn)
echo $ALPN_TOKEN
```

### 2. TCP Tunnel (e.g., SSH)

**Server** (on server — waits for client connections):
```bash
tunnel-rs server \
  --secret-file ./server.key \
  --allowed-tcp 127.0.0.0/8 \
  --auth-tokens "$AUTH_TOKEN" \
  --alpn-token "$ALPN_TOKEN"
```

Output:
```
EndpointId: 2xnbkpbc7izsilvewd7c62w7wnwziacmpfwvhcrya5nt76dqkpga
Auth tokens: 1 token(s) configured
Waiting for clients to connect...
```

**Client** (on client — requests source from server):
```bash
tunnel-rs client \
  --server-node-id <SERVER_ENDPOINT_ID> \
  --source tcp://127.0.0.1:22 \
  --target 127.0.0.1:2222 \
  --auth-token "$AUTH_TOKEN" \
  --alpn-token "$ALPN_TOKEN"
```

Then connect: `ssh -p 2222 user@127.0.0.1`

### 3. UDP Tunnel (e.g., WireGuard/Game/DNS)

**Server**:
```bash
tunnel-rs server \
  --secret-file ./server.key \
  --allowed-udp 127.0.0.0/8 \
  --auth-tokens "$AUTH_TOKEN" \
  --alpn-token "$ALPN_TOKEN"
```

**Client**:
```bash
tunnel-rs client \
  --server-node-id <SERVER_ENDPOINT_ID> \
  --source udp://127.0.0.1:51820 \
  --target 0.0.0.0:51820 \
  --auth-token "$AUTH_TOKEN" \
  --alpn-token "$ALPN_TOKEN"
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
| `--auth-tokens` | - | Authentication tokens (repeatable). Required unless provided via `--auth-tokens-file`. |
| `--auth-tokens-file` | - | Path to file containing authentication tokens (one per line, # comments allowed). Can be combined with `--auth-tokens`. |
| `--alpn-token` | required | ALPN token for QUIC handshake-level filtering (14-char Base64URL with CRC16 checksum). Generate with `generate-token --alpn`. |
| `--alpn-token-file` | - | Path to file containing ALPN token (use this or `--alpn-token`, not both) |
| `--max-sessions` | 100 | Maximum concurrent sessions |
| `--secret` | - | Base64-encoded secret key for persistent server identity (use this or `--secret-file`) |
| `--secret-file` | - | Path to secret key file for persistent server identity (use this or `--secret`) |
| `--relay-url` | public | Custom relay server URL(s), repeatable |
| `--relay-only` | false | Force all traffic through relay (CLI-only; not supported in config files) |
| `--dns-server` | public | Custom DNS server URL, or "none" to disable DNS discovery |

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
| `--auth-token` | - | Authentication token to send to server (required unless provided via `--auth-token-file`) |
| `--auth-token-file` | - | Path to file containing authentication token (use this or `--auth-token`) |
| `--alpn-token` | required | ALPN token for QUIC handshake-level filtering (14-char Base64URL with CRC16 checksum, must match server). Generate with `generate-token --alpn`. |
| `--alpn-token-file` | - | Path to file containing ALPN token (use this or `--alpn-token`, not both) |
| `--relay-url` | public | Custom relay server URL(s), repeatable |
| `--relay-only` | false | Force all traffic through relay (CLI-only; not supported in config files) |
| `--dns-server` | public | Custom DNS server URL, or "none" to disable DNS discovery |

## Configuration Files

Use `--default-config` to load from the default location, or `-c <path>` for a custom path (both TOML). For normal usage, prefer config files so your settings are saved and reusable. The `--config-stdin` flag is intended for automation and IPC — it accepts JSON (self-delimiting, so the caller does not need to close stdin). Only one of these may be used at a time. Configuration uses the `[iroh]` section.

> **Security:** TOML config files **reject plaintext sensitive fields** (`auth_token`, `auth_tokens`, `alpn_token`, `secret`). Use the corresponding `_file` variants in config files, or pass values via CLI arguments or `--config-stdin` (JSON), which are transient and not persisted to disk.

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

### Server Config Example

```toml
# Example server configuration (iroh mode)

# Required: validates config matches CLI command
role = "server"
mode = "iroh"

[iroh]
secret_file = "./server.key"
# relay_urls = ["https://relay.example.com"]
dns_server = "https://dns.example.com/pkarr"
max_sessions = 100

# Authentication tokens file (one token per line, # comments allowed)
# Generate tokens with: tunnel-rs generate-token
auth_tokens_file = "/etc/tunnel-rs/auth_tokens.txt"

# ALPN token file for QUIC handshake-level filtering
# Generate with: tunnel-rs generate-token --alpn
alpn_token_file = "/etc/tunnel-rs/alpn_token.txt"

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
dns_server = "https://dns.example.com/pkarr"

# Authentication token file (get token from server admin, 47 chars)
auth_token_file = "~/.config/tunnel-rs/token.txt"

# ALPN token file (must match server, 14-char Base64URL)
alpn_token_file = "~/.config/tunnel-rs/alpn_token.txt"
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
        "auth_token": "<AUTH_TOKEN>",
        "alpn_token": "<ALPN_TOKEN>",
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

## generate-token

Generate authentication tokens for iroh mode:

```bash
# Generate a single auth token
tunnel-rs generate-token
# Output: i<base64url-encoded-payload>

# Generate multiple auth tokens
tunnel-rs generate-token -c 5

# Generate an ALPN token (14-char Base64URL with checksum)
tunnel-rs generate-token --alpn
```

Auth token format: `i` + Base64URL-encoded(32 random bytes + CRC16 checksum) = 47 characters total.

ALPN token format: Base64URL-encoded(8 random bytes + 2-byte CRC16 checksum) = 14 characters total.

## generate-server-key

*For iroh mode.*

```bash
tunnel-rs generate-server-key --output ./server.key
```

## show-server-id

```bash
tunnel-rs show-server-id --secret-file ./server.key
```

---

## Security

- All traffic is encrypted using QUIC/TLS 1.3
- The EndpointId is a public key that identifies the server
- **ALPN-level filtering:** A pre-shared ALPN token is embedded in the QUIC protocol identifier (`mf/2/<token>`). Connections from clients without the correct token are rejected at the QUIC handshake level — before any application streams are opened — acting as a lightweight "port knock".
- **Token Authentication (iroh mode):** Clients authenticate immediately after QUIC connection via a dedicated auth stream. Invalid tokens are rejected with an `AuthResponse` and the connection is closed with an error code. See [Architecture: Token Authentication](docs/ARCHITECTURE.md#token-authentication-iroh-mode).
- Secret key files are created with `0600` permissions (Unix) and appropriate permissions on Windows
- Treat secret key files, auth tokens, and ALPN tokens like passwords

## Exit Codes (Client Mode)

The client process uses categorized exit codes so wrapper scripts can distinguish transient failures (retry) from permanent errors (stop).

| Exit Code | Meaning | Retry? |
|-----------|---------|--------|
| 0 | Success | N/A |
| 1 | General/unexpected error | Use judgment |
| 2 | Configuration error (invalid arguments, bad token format, missing fields) | No — fix configuration |
| 3 | Authentication failure (token rejected, auth timeout) | No — fix credentials |
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
1. Server creates an iroh endpoint with discovery services
2. Server publishes its address via Pkarr/DNS
3. Client resolves the server via discovery
4. **ALPN handshake:** QUIC connection requires matching ALPN token (`mf/2/<token>`) — clients without the token are rejected at the handshake level
5. **Authentication phase:** Client opens dedicated auth stream and sends `AuthRequest` with token
6. **Server validates token** (10s timeout) — invalid tokens are rejected with an error response
   - *If authentication fails, the connection is closed and steps 7–9 do not occur*
7. **Source request phase:** Client opens source stream with `SourceRequest`
8. Server validates source against allowed networks and responds
9. If accepted, traffic forwarding begins

