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

**CLI** (tokens saved to files — recommended):
```bash
# Save tokens to files with restricted permissions
echo "$AUTH_TOKEN" > auth_tokens.txt && chmod 600 auth_tokens.txt
echo "$ALPN_TOKEN" > alpn_token.txt && chmod 600 alpn_token.txt

tunnel-rs server \
  --secret-file ./server.key \
  --allowed-tcp 127.0.0.0/8 \
  --auth-tokens-file ./auth_tokens.txt \
  --alpn-token-file ./alpn_token.txt
```

> **Tip:** For containers and automation scripts, use environment variables (`TUNNEL_RS_AUTH_TOKENS`, `TUNNEL_RS_ALPN_TOKEN`) instead of files. See the server environment variable table below.

**Config file** (`server.toml`):
```toml
[iroh]
secret_file = "./server.key"
```

> **Note:** Clients use ephemeral identities by default. Only the server needs a persistent key to maintain a stable EndpointId that clients can connect to.

> **Note:** The age encryption key (`config-encryption generate-key`) is different from the server identity key (`generate-server-key`). The server key establishes a stable EndpointId for P2P connections. The encryption key protects secrets stored in config files.

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
# Use a file with one token per line (recommended)
tunnel-rs server \
  --allowed-tcp 127.0.0.0/8 \
  --auth-tokens-file /etc/tunnel-rs/auth_tokens.txt

# Or comma-separated via environment variable (for containers/automation)
export TUNNEL_RS_AUTH_TOKENS="token-for-alice,token-for-bob"
tunnel-rs server --allowed-tcp 127.0.0.0/8
```

**Example `auth_tokens.txt`:**
```text
# Alice's token (generate with: tunnel-rs generate-token)
iXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX

# Bob's token
iYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYYY
```

### Configuration File

> **Security:** Plaintext tokens and secrets are **not allowed** in TOML config files. Use the `_file` variants (e.g., `auth_tokens_file`, `auth_token_file`, `alpn_token_file`, `secret_file`) in config files. For non-containerized deployments, `_file` variants are the recommended approach. Environment variables (`TUNNEL_RS_*`) are best suited for containers and automation scripts where secrets are injected dynamically. Plaintext values are also accepted via `--config-stdin` (JSON) for IPC.

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
# Save tokens to files (recommended for non-containerized use)
echo "$AUTH_TOKEN" > auth_tokens.txt && chmod 600 auth_tokens.txt
echo "$ALPN_TOKEN" > alpn_token.txt && chmod 600 alpn_token.txt

tunnel-rs server \
  --secret-file ./server.key \
  --allowed-tcp 127.0.0.0/8 \
  --auth-tokens-file ./auth_tokens.txt \
  --alpn-token-file ./alpn_token.txt
```

Output:
```
EndpointId: 2xnbkpbc7izsilvewd7c62w7wnwziacmpfwvhcrya5nt76dqkpga
Auth tokens: 1 token(s) configured
Waiting for clients to connect...
```

**Client** (on client — requests source from server):
```bash
# Save tokens to files
echo "$AUTH_TOKEN" > auth_token.txt && chmod 600 auth_token.txt
echo "$ALPN_TOKEN" > alpn_token.txt && chmod 600 alpn_token.txt

tunnel-rs client \
  --server-node-id <SERVER_ENDPOINT_ID> \
  --source tcp://127.0.0.1:22 \
  --target 127.0.0.1:2222 \
  --auth-token-file ./auth_token.txt \
  --alpn-token-file ./alpn_token.txt
```

Then connect: `ssh -p 2222 user@127.0.0.1`

> **Tip:** For containers and automation scripts, use environment variables (`TUNNEL_RS_AUTH_TOKEN`, `TUNNEL_RS_ALPN_TOKEN`, etc.) instead of files. See the client environment variable table below.

### 3. UDP Tunnel (e.g., WireGuard/Game/DNS)

**Server**:
```bash
tunnel-rs server \
  --secret-file ./server.key \
  --allowed-udp 127.0.0.0/8 \
  --auth-tokens-file ./auth_tokens.txt \
  --alpn-token-file ./alpn_token.txt
```

**Client**:
```bash
tunnel-rs client \
  --server-node-id <SERVER_ENDPOINT_ID> \
  --source udp://127.0.0.1:51820 \
  --target 0.0.0.0:51820 \
  --auth-token-file ./auth_token.txt \
  --alpn-token-file ./alpn_token.txt
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
| `--auth-tokens-file` | - | Path to file containing authentication tokens (one per line, # comments allowed) |
| `--alpn-token-file` | - | Path to file containing ALPN token |
| `--max-sessions` | 100 | Maximum concurrent sessions |
| `--secret-file` | - | Path to secret key file for persistent server identity |
| `--relay-url` | public | Custom relay server URL(s), repeatable |
| `--relay-only` | false | Force all traffic through relay (CLI-only; not supported in config files) |
| `--dns-server` | public | Custom DNS server URL, or "none" to disable DNS discovery |
| `--encryption-key-file` | - | Path to age identity file for decrypting age-encrypted config values |

**Environment variables** (for containers and automation scripts):

> Environment variables are primarily intended for containerized deployments and automation scripts. For regular use, prefer the `_file` CLI flags or config file equivalents.

| Env Var | Description |
|---------|-------------|
| `TUNNEL_RS_AUTH_TOKENS` | Authentication tokens (comma-separated). Required unless provided via `--auth-tokens-file`. |
| `TUNNEL_RS_ALPN_TOKEN` | ALPN token for QUIC handshake-level filtering (14-char Base64URL with CRC16 checksum). Required unless provided via `--alpn-token-file`. Generate with `generate-token --alpn`. |
| `TUNNEL_RS_SECRET` | Base64-encoded secret key for persistent server identity (use this or `--secret-file`) |
| `TUNNEL_RS_ENCRYPTION_KEY_FILE` | Path to age identity file for decrypting age-encrypted config values |

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
| `--auth-token-file` | - | Path to file containing authentication token |
| `--alpn-token-file` | - | Path to file containing ALPN token |
| `--relay-url` | public | Custom relay server URL(s), repeatable |
| `--relay-only` | false | Force all traffic through relay (CLI-only; not supported in config files) |
| `--dns-server` | public | Custom DNS server URL, or "none" to disable DNS discovery |
| `--encryption-key-file` | - | Path to age identity file for decrypting age-encrypted config values |

**Environment variables** (for containers and automation scripts):

> Environment variables are primarily intended for containerized deployments and automation scripts. For regular use, prefer the `_file` CLI flags or config file equivalents.

| Env Var | Description |
|---------|-------------|
| `TUNNEL_RS_AUTH_TOKEN` | Authentication token to send to server (required unless provided via `--auth-token-file`) |
| `TUNNEL_RS_ALPN_TOKEN` | ALPN token for QUIC handshake-level filtering (14-char Base64URL with CRC16 checksum, must match server). Generate with `generate-token --alpn`. |
| `TUNNEL_RS_ENCRYPTION_KEY_FILE` | Path to age identity file for decrypting age-encrypted config values |

## Configuration Files

Use `--default-config` to load from the default location, or `-c <path>` for a custom path (both TOML). For normal usage, prefer config files so your settings are saved and reusable. The `--config-stdin` flag is intended for automation and IPC — it accepts JSON (self-delimiting, so the caller does not need to close stdin). Only one of these may be used at a time. Configuration uses the `[iroh]` section.

> **Security:** TOML config files **reject plaintext sensitive fields** (`auth_token`, `auth_tokens`, `alpn_token`, `secret`). You have three options: use the corresponding `_file` variants (recommended), use environment variables (`TUNNEL_RS_*`) for containers/automation, or use [age-encrypted inline values](#encrypted-config-values). Plaintext values are also accepted via `--config-stdin` (JSON) for IPC.

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

### Encrypted Config Values

Instead of separate `_file` variants, you can embed age-encrypted secrets directly in TOML config files. This is useful when managing configs for multiple servers — each config is self-contained with a single shared private key.

**Setup:**

```bash
# 1. Generate an age keypair (run again to add keys for rotation)
tunnel-rs config-encryption generate-key --output ~/.config/tunnel-rs/age.key
# Output: age1ql3z7hjy...  (this is your public key / recipient)

# 2. Encrypt a secret value
echo -n "$AUTH_TOKEN" | tunnel-rs config-encryption encrypt-value --recipient age1ql3z7hjy...
```

**Use in config:**

```toml
[iroh]
encryption_key_file = "~/.config/tunnel-rs/age.key"
encryption_recipient = "age1ql3z7hjy..."

auth_token = "ageenc:YWdlLWVuY3J5cHRpb24ub3JnL3Yx..."
alpn_token = "ageenc:YWdlLWVuY3J5cHRpb24ub3JnL3Yx..."
```

Each encrypted value is a single-line `ageenc:` prefixed string (base64-encoded age ciphertext). The `encryption_key_file` can also be specified via `--encryption-key-file` CLI flag or `TUNNEL_RS_ENCRYPTION_KEY_FILE` env var.

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

## config-encryption

Age encryption commands for config file secrets.

### generate-key

Generate an age keypair for encrypting config file secrets:

```bash
tunnel-rs config-encryption generate-key --output ~/.config/tunnel-rs/age.key
# Prints the public key (recipient) to stdout
```

Running again with the same `--output` appends a new keypair to the file, enabling key rotation. Use `--force` to overwrite the file and start fresh.

### encrypt-value

Encrypt a value for embedding in config files (reads plaintext from stdin):

```bash
echo -n "$AUTH_TOKEN" | tunnel-rs config-encryption encrypt-value --recipient age1...

# Or read recipient from a config file
echo -n "$AUTH_TOKEN" | tunnel-rs config-encryption encrypt-value --config client.toml
```

Output is a single-line `ageenc:` string ready to paste into TOML config values.

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
