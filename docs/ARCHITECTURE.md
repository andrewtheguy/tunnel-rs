# tunnel-rs Architecture

This document provides a comprehensive overview of the tunnel-rs architecture, including detailed diagrams of component interactions, data flows, and security considerations.

## Table of Contents

- [System Overview](#system-overview)
- [Features](#features)
- [iroh Mode Architecture](#iroh-mode-architecture)
- [Configuration System](#configuration-system)
- [Security Model](#security-model)
- [Protocol Support](#protocol-support)
- [Component Details](#component-details)
- [Performance Considerations](#performance-considerations)
- [Error Handling](#error-handling)
- [Capabilities](#capabilities)
- [References](#references)

---

## System Overview

tunnel-rs is a P2P TCP/UDP port forwarding tool using iroh for peer discovery, relay fallback, and encrypted QUIC transport.

Binary: `tunnel-rs`

> **Design Goal:** The project's primary goal is to provide a convenient way to connect to different networks for development or homelab purposes without the hassle and security risk of opening a port. It is **not** meant for production setups or designed to be performant at scale.

```mermaid
graph TB
    subgraph "tunnel-rs"
        A[iroh]
    end

    subgraph "Use Cases"
        D[Best NAT Traversal<br/>Relay Fallback]
    end

    subgraph "Infrastructure"
        G[Pkarr/DNS<br/>Relay Servers]
    end

    A --> D
    A --> G

    style A fill:#4CAF50
```

Relay-only is a CLI-only flag that forces connections through relay servers instead of attempting direct connections. It is intended for testing or special scenarios and is not supported in config files to avoid accidental activation. See `tunnel-rs --help` for usage.

### Core Components

```mermaid
graph LR
    subgraph "Core Modules"
        A[main.rs<br/>CLI & orchestration]
        B[config.rs<br/>Config loading & validation]
        C[multi_source.rs<br/>Server/client tunnel loops]
        C2[net.rs<br/>Address parsing & ACL checks]
        D[endpoint.rs<br/>iroh endpoint setup]
        E[secret.rs<br/>Identity management]
        E2[auth.rs<br/>Auth + ALPN tokens]
        F[signaling/codec.rs<br/>Handshake messages]
    end

    A --> B
    A --> C
    A --> E
    A --> E2
    C --> C2
    C --> D
    C --> E2
    C --> F

    style A fill:#E3F2FD
    style C fill:#E8F5E9
    style E2 fill:#FFCCBC
```

---

## Features

### Feature Summary

```mermaid
graph TD
    subgraph "iroh"
        A1[Discovery: Automatic]
        A2[NAT: Relay Fallback]
        A3[Setup: Minimal - EndpointId required]
        A4[Infrastructure: Required]
    end

    style A1 fill:#C8E6C9
    style A2 fill:#C8E6C9
    style A3 fill:#C8E6C9
    style A4 fill:#FFCCBC
```

### NAT Traversal Capabilities

```mermaid
graph LR
    subgraph "NAT Types"
        A[Full Cone]
        B[Restricted Cone]
        C[Port Restricted]
        D[Symmetric]
    end

    subgraph "iroh"
        E1[✓ Direct/Relay]
        E2[✓ Direct/Relay]
        E3[✓ Direct/Relay]
        E4[✓ Relay]
    end

    A --> E1
    B --> E2
    C --> E3
    D --> E4

    style E1 fill:#C8E6C9
    style E2 fill:#C8E6C9
    style E3 fill:#C8E6C9
    style E4 fill:#C8E6C9
```

---

## iroh Mode Architecture

### Architecture Overview

```mermaid
graph TB
    subgraph "Server Side"
        A[tunnel-rs server]
        B[iroh Endpoint]
        C[Target Service<br/>e.g., SSH:22]
        D[Discovery<br/>Pkarr/DNS]
        E[Relay Server]
    end

    subgraph "Client Side"
        F[tunnel-rs client]
        G[iroh Endpoint]
        H[Local Client<br/>e.g., SSH client]
        I[Discovery<br/>Pkarr/DNS]
        J[Relay Server]
    end

    A --> B
    B --> C
    B --> D
    B --> E

    F --> G
    G --> H
    G --> I
    G --> J

    B <-.QUIC/TLS.-> G
    D <-.Publish/Resolve.-> I
    E <-.Fallback.-> J

    style A fill:#E8F5E9
    style F fill:#E8F5E9
    style B fill:#BBDEFB
    style G fill:#BBDEFB
```

### Connection Establishment Flow

```mermaid
sequenceDiagram
    participant S as Server
    participant SD as Discovery Service
    participant C as Client
    participant RS as Relay Server

    Note over S: Generate/Load Secret Key
    S->>S: Create iroh Endpoint
    S->>SD: Publish EndpointId + Addresses
    Note over S: Display EndpointId
    S->>RS: Connect to relay

    Note over C: User provides EndpointId
    C->>C: Create iroh Endpoint
    C->>SD: Resolve EndpointId
    SD-->>C: Return addresses
    C->>RS: Connect to relay

    alt Direct Connection Possible
        C->>S: Direct QUIC connection (ALPN: mf/2/<token>)
        S-->>C: Accept connection (ALPN match)
    else ALPN Mismatch
        C->>S: QUIC connection (wrong ALPN)
        S-->>C: Handshake rejected (no matching ALPN)
    else NAT Traversal Failed
        C->>RS: Connect via relay
        RS->>S: Forward connection
        S-->>RS: Accept via relay (ALPN match)
        RS-->>C: Relay established
    end

    Note over S,C: Encrypted QUIC tunnel established

    Note over C,S: Authentication Phase
    C->>S: Open auth stream
    C->>S: AuthRequest {token}
    alt Token Valid
        S-->>C: AuthResponse {accepted: true}
    else Token Invalid
        S-->>C: AuthResponse {accepted: false, reason}
        S->>S: Close connection (error code 1)
    else Auth Timeout
        S->>S: Close connection (error code 2)
    end

    Note over C,S: Source Request Phase (after successful auth)
    C->>S: Open source stream
    C->>S: SourceRequest {source}
    S-->>C: SourceResponse {accepted}

    loop Data Transfer
        C->>S: Forward client traffic
        S->>S: Forward to target
        S->>C: Return target response
        C->>C: Forward to client
    end
```

### TCP Tunnel Data Flow

```mermaid
graph LR
    subgraph "Client"
        A[TCP Client] -->|connect| B[Listen Socket]
        B -->|accept| C[TCP Stream]
        C -->|read| D[Buffer]
        D -->|write| E[iroh SendStream]
    end

    subgraph "QUIC Transport"
        E <-->|encrypted| F[iroh RecvStream]
    end

    subgraph "Server"
        F -->|read| G[Buffer]
        G -->|write| H[TCP Stream]
        H -->|connect| I[Target Service]
        I -->|response| H
        H -->|read| J[Buffer]
        J -->|write| K[iroh SendStream]
    end

    subgraph "Return Path"
        K <-->|encrypted| L[iroh RecvStream]
        L -->|read| M[Buffer]
        M -->|write| C
        C -->|send| A
    end

    style E fill:#BBDEFB
    style F fill:#BBDEFB
    style K fill:#BBDEFB
    style L fill:#BBDEFB
```

### UDP Tunnel Data Flow

```mermaid
graph TB
    subgraph "Client"
        A[UDP Client] -->|sendto| B[UDP Socket]
        B -->|recvfrom| C[Packet Buffer]
        C -->|encode length + data| D[iroh SendStream]
    end

    subgraph "QUIC Transport"
        D <-->|encrypted| E[iroh RecvStream]
    end

    subgraph "Server"
        E -->|decode| F[Packet Buffer]
        F -->|sendto| G[UDP Socket]
        G -->|forward| H[Target Service]
        H -->|response| G
        G -->|recvfrom| I[Response Buffer]
        I -->|encode| J[iroh SendStream]
    end

    subgraph "Return Path"
        J <-->|encrypted| K[iroh RecvStream]
        K -->|decode| L[Packet Buffer]
        L -->|sendto| B
        B -->|deliver| A
    end

    style D fill:#BBDEFB
    style E fill:#BBDEFB
    style J fill:#BBDEFB
    style K fill:#BBDEFB
```

### Endpoint Management

```mermaid
graph TB
    subgraph "Endpoint Creation"
        A[Load/Generate Secret] --> B[Create Endpoint Builder]
        B --> C{Relay URLs?}
        C -->|Yes| D[Add Custom Relays]
        C -->|No| E[Use Default Relays]
        D --> F{Relay Only? (CLI-only)}
        E --> F
        F -->|Yes| G[Disable IP transports]
        F -->|No| H[Keep IP + relay transports]
        G --> I{DNS Server?}
        H --> I
        I -->|Yes| J[Add Custom DNS]
        I -->|No| K[Use Default DNS]
        J --> L[Build Endpoint]
        K --> L
    end

    subgraph "Discovery"
        L --> M[Publish to Pkarr/DNS]
        M --> N[Enable mDNS]
        N --> O[Endpoint Ready]
    end

    style A fill:#FFE0B2
    style L fill:#C8E6C9
    style O fill:#C8E6C9
```

---

## Configuration System

### Configuration File Structure

```mermaid
graph TB
    subgraph "Config File"
        A[role: server/client]
        B[mode: iroh]
    end

    subgraph "iroh Options"
        C[server_node_id<br/>client only]
        D[request_source / target<br/>client only]
        E[allowed_sources / max_sessions<br/>server only]
        F[secret_file / secret<br/>server only]
        G[auth_token* / auth_tokens*]
        H[alpn_token* / encryption_key_file]
        I[relay_urls / dns_server]
        J[transport<br/>cc + window sizes]
    end

    A --> S[Validation]
    B --> S
    S --> C
    S --> D
    S --> E
    S --> F
    S --> G
    S --> H
    S --> I
    S --> J

    style S fill:#FFF9C4
```

### iroh Credential Mapping

`iroh` mode uses two distinct credential types:

| Credential | Env Vars / CLI Flags | Config Keys (TOML: use `_file` variants or age-encrypted inline) | Expected Usage |
|------------|-----------|-------------|----------------|
| **ALPN Token** | `TUNNEL_RS_ALPN_TOKEN` or `--alpn-token-file` | Server/Client: `[iroh].alpn_token_file` or age-encrypted `[iroh].alpn_token` | Pre-handshake QUIC ALPN filter (`mf/2/<token>`). Typically one shared value for a server and all its clients. |
| **Auth Token** | Server: `TUNNEL_RS_AUTH_TOKENS` or `--auth-tokens-file`<br>Client: `TUNNEL_RS_AUTH_TOKEN` or `--auth-token-file` | Server: `[iroh].auth_tokens_file` or age-encrypted `[iroh].auth_tokens`<br>Client: `[iroh].auth_token_file` or age-encrypted `[iroh].auth_token` | Per-client credential checked on the auth stream after handshake. Use separate values per client for revocation/rotation. |

Example usage with files (recommended):

```bash
# Server — save tokens to files with restricted permissions
echo "$ALPN_TOKEN" > alpn_token.txt && chmod 600 alpn_token.txt
printf '%s\n' "$ALICE_AUTH_TOKEN" "$BOB_AUTH_TOKEN" > auth_tokens.txt && chmod 600 auth_tokens.txt
tunnel-rs server --alpn-token-file ./alpn_token.txt --auth-tokens-file ./auth_tokens.txt ...

# Alice's client
echo "$ALICE_AUTH_TOKEN" > auth_token.txt && chmod 600 auth_token.txt
tunnel-rs client --alpn-token-file ./alpn_token.txt --auth-token-file ./auth_token.txt ...
```

Example usage with environment variables (for containers/automation):

```bash
# Server
export TUNNEL_RS_ALPN_TOKEN="$ALPN_TOKEN"
export TUNNEL_RS_AUTH_TOKENS="$ALICE_AUTH_TOKEN,$BOB_AUTH_TOKEN"
tunnel-rs server ...

# Alice's client
export TUNNEL_RS_ALPN_TOKEN="$ALPN_TOKEN"
export TUNNEL_RS_AUTH_TOKEN="$ALICE_AUTH_TOKEN"
tunnel-rs client ...
```

Example config usage (plaintext tokens are not allowed in TOML config files — use `_file` variants or age-encrypted inline values):

```toml
# server.toml — using _file variants
[iroh]
alpn_token_file = "/etc/tunnel-rs/alpn_token.txt"
auth_tokens_file = "/etc/tunnel-rs/auth_tokens.txt"

# client.toml — using _file variants
[iroh]
alpn_token_file = "~/.config/tunnel-rs/alpn_token.txt"
auth_token_file = "~/.config/tunnel-rs/token.txt"
```

```toml
# client.toml — using age-encrypted inline values
[iroh]
encryption_key_file = "~/.config/tunnel-rs/age.key"

auth_token = "ageenc:YWdlLWVuY3J5cHRpb24ub3JnL3Yx..."
alpn_token = "ageenc:YWdlLWVuY3J5cHRpb24ub3JnL3Yx..."
```

### Configuration Loading Flow

For normal usage, prefer file-based configs (`-c`, `--default-config`) which use TOML — settings are saved and reusable. The `--config-stdin` flag is intended for automation and IPC only; it uses JSON because JSON is self-delimiting — `serde_json::from_reader` parses exactly one JSON object and returns without waiting for EOF, allowing the caller to keep stdin open. Config passed via stdin is not persisted.

```mermaid
sequenceDiagram
    participant CLI as CLI Parser
    participant Main as Main
    participant Config as Config Module
    participant Source as Config Source (file or stdin)

    CLI->>Main: Parse arguments
    Main->>Main: Check config flags (only one allowed)

    alt --default-config
        Main->>Config: Load from default path
        Config->>Source: Read ~/.config/tunnel-rs/{role}.toml
        Source-->>Config: TOML content
    else -c <path>
        Main->>Config: Load from specified path
        Config->>Source: Read file
        Source-->>Config: TOML content
    else --config-stdin
        Main->>Config: Read from stdin
        Source-->>Config: JSON content
    else No config flag
        Main->>Main: Use CLI arguments only
    end

    alt Config loaded
        Config->>Config: Parse (TOML from file, JSON from stdin)
        Config->>Config: Validate role + mode
        Config-->>Main: Validated config
        Main->>Main: Merge with CLI args
    end

    Main->>Main: Proceed with merged config
```

### Config Validation

```mermaid
graph TB
    A[Load Config] --> B{Role matches?}
    B -->|No| C[Error: Role mismatch]
    B -->|Yes| F{Check sections}

    F --> G{Extra sections?}
    G -->|Yes| H[Ignored by parser]
    G -->|No| I{Required fields?}

    I -->|Missing| J[Error: Missing field]
    I -->|Present| K[Validation Success]

    style C fill:#FFCCBC
    style H fill:#FFF9C4
    style J fill:#FFCCBC
    style K fill:#C8E6C9
```

---

## Security Model

### Encryption Stack

```mermaid
graph TB
    subgraph "Application Data"
        A[TCP/UDP Payload]
    end
    
    subgraph "QUIC Layer"
        B[QUIC Stream Encryption]
        C[TLS 1.3]
        D[Per-Stream Keys]
    end
    
    subgraph "Transport"
        E[QUIC Packets]
        F[Authenticated Encryption]
    end
    
    subgraph "Network"
        G[UDP Datagrams]
    end
    
    A --> B
    B --> C
    C --> D
    D --> E
    E --> F
    F --> G
    
    style C fill:#C8E6C9
    style D fill:#C8E6C9
    style F fill:#C8E6C9
```

### Identity and Authentication

```mermaid
graph TB
    subgraph "iroh Mode"
        A[Server Secret Key] --> B[Ed25519 Private Key]
        B --> C[EndpointId - Public Key]
        C --> D[Client Connects]
        D --> D2[ALPN Token Validation]
        D2 --> E[Auth Token Validation]
        E --> F{Valid Token?}
        F -->|Yes| G[Authenticated]
        F -->|No| H[Rejected]
    end

    style B fill:#FFE0B2
    style C fill:#C8E6C9
    style G fill:#C8E6C9
    style H fill:#FFCCBC
```

### Token Authentication (iroh Mode)

Iroh mode uses two layers of authentication. First, a pre-shared ALPN token is embedded in the QUIC protocol identifier (`mf/2/<token>`), rejecting unknown clients at the TLS handshake level before any application streams are opened. Second, clients must provide a valid auth token via a dedicated auth stream within a 10-second timeout. **Both layers are mandatory.**

#### ALPN Token vs Auth Token

- **ALPN Token** (`TUNNEL_RS_ALPN_TOKEN` env var / `--alpn-token-file` / `[iroh].alpn_token_file`): Pre-handshake shared value used for QUIC ALPN filtering.
- **Auth Token** (server: `TUNNEL_RS_AUTH_TOKENS` env var / `--auth-tokens-file` / `[iroh].auth_tokens_file`; client: `TUNNEL_RS_AUTH_TOKEN` env var / `--auth-token-file` / `[iroh].auth_token_file`): Per-client token validated on the auth stream.
- **Mapping**: These are **distinct tokens**, not the same value. In code, ALPN tokens are 14-char Base64URL values, while auth tokens are 47-char `i...` tokens. Typical setup is one shared ALPN token plus per-client auth tokens for revocation.

1. **ALPN Filtering**: Both server and client set `TUNNEL_RS_ALPN_TOKEN`. The token is embedded in the QUIC ALPN identifier (`mf/2/<token>`). Connections from clients without a matching ALPN are rejected at the handshake level — acting as a lightweight "port knock".
2. **Server Configuration**: Server sets `TUNNEL_RS_AUTH_TOKENS` with one or more pre-shared tokens (comma-separated)
3. **Client Configuration**: Client sets `TUNNEL_RS_AUTH_TOKEN` with the token received from the server admin
4. **Protocol Flow**: Client opens a dedicated auth stream immediately after connection and sends an `AuthRequest`. **No source requests are accepted until authentication succeeds.**
5. **Validation**: Server validates the token using `is_token_valid()` within a 10-second timeout
6. **Rejection**: Invalid tokens are rejected with an `AuthResponse` containing the rejection reason, and the connection is closed with an error code.

This layered validation prevents unauthorized clients from holding open connections or attempting source requests.

```mermaid
sequenceDiagram
    participant C as Client
    participant S as Server
    participant A as Auth Module

    C->>S: Connect (QUIC TLS handshake, ALPN: mf/2/<token>)
    alt ALPN matches
        S->>C: Accept connection
    else ALPN mismatch
        S-->>C: Handshake rejected
        Note over S,C: Connection rejected at handshake level
    end

    Note over C,S: Auth Phase (10s timeout)
    C->>S: Open auth stream
    C->>S: AuthRequest {version, auth_token}
    S->>A: is_token_valid(auth_token, auth_tokens)
    alt Token is valid
        A-->>S: true
        S->>C: AuthResponse {accepted: true}
        Note over S,C: Connection authenticated
    else Token is invalid
        A-->>S: false
        S->>C: AuthResponse {accepted: false, reason}
        S->>S: Close connection (error code 1)
        Note over S,C: Connection closed with rejection
    else Timeout (no auth within 10s)
        S->>S: Close connection (error code 2)
        Note over S,C: Connection closed (auth timeout)
    end

    Note over C,S: Source Request Phase (after successful auth)
    C->>S: Open source stream
    C->>S: SourceRequest {source}
    S->>S: Validate source against allowed networks
    S->>C: SourceResponse::accepted()
    Note over S,C: Proceed with tunnel data transfer
```

### Token Security Notes (iroh Mode)

- Tokens are **bearer credentials**: possession is sufficient for access. Use one token per client to enable revocation.
- Token strength comes from **randomness, not format**: 32 random bytes (256 bits of entropy). Treat tokens like high‑entropy secrets.
- Tokens are sent only **after** the QUIC/TLS 1.3 handshake, so the auth stream is encrypted in transit.
- The CRC16-CCITT-FALSE checksum is **for typo detection only**, not cryptographic security.
- Tokens are Base64URL-encoded and validated as ASCII.
- The **ALPN token** acts as a pre-handshake filter (lightweight "port knock"). It is embedded in the TLS ClientHello and therefore **visible in cleartext** on the wire — it is not a secret, but prevents casual scanners from completing a QUIC handshake.
- Avoid logging or sharing tokens; the `AuthToken` wrapper redacts values in Debug output, but treat them like passwords.
- Prefer token files with restricted permissions (e.g., `0600`) and rotate tokens if exposure is suspected.

### Threat Model

```mermaid
graph TB
    subgraph "Protected Against"
        A[Eavesdropping<br/>TLS 1.3 encryption]
        B[MITM<br/>Peer authentication]
        C[Replay Attacks<br/>QUIC nonces]
        D[Tampering<br/>Authenticated encryption]
        E2[Unauthorized Access<br/>Token Authentication - iroh mode]
    end

    subgraph "User Responsibility"
        F[Secret Key Protection<br/>iroh server]
        G[EndpointId Verification<br/>Trust on first use]
        H[Auth Token Security<br/>Treat tokens like passwords]
    end

    style A fill:#C8E6C9
    style B fill:#C8E6C9
    style C fill:#C8E6C9
    style D fill:#C8E6C9
    style E2 fill:#C8E6C9

    style F fill:#FFF9C4
    style G fill:#FFF9C4
    style H fill:#FFF9C4
```

### Secret Key Management (Server Only)

In iroh mode, only the **server** needs a persistent secret key to maintain a stable EndpointId. Clients use ephemeral identities and authenticate via tokens.

```mermaid
sequenceDiagram
    participant User as User
    participant CLI as CLI
    participant Secret as Secret Module
    participant FS as File System

    alt Generate Server Key
        User->>CLI: generate-server-key --output server.key
        CLI->>Secret: Generate Ed25519 key
        Secret->>Secret: Derive EndpointId
        Secret->>FS: Write with 0600 permissions
        FS-->>Secret: Success
        Secret->>CLI: Display EndpointId
        CLI->>User: Show EndpointId (share with clients)
    end

    alt Load Server Secret
        User->>CLI: server --secret-file server.key
        CLI->>FS: Read key file
        FS-->>Secret: Key bytes
        Secret->>Secret: Parse Ed25519 key
        Secret->>Secret: Derive EndpointId
        Secret-->>CLI: Secret + EndpointId
    end

    alt Show EndpointId
        User->>CLI: show-server-id --secret-file server.key
        CLI->>FS: Read key file
        FS-->>Secret: Key bytes
        Secret->>Secret: Derive EndpointId
        Secret->>User: Display EndpointId
    end
```

---

## Protocol Support

### TCP Tunneling Architecture

```mermaid
graph TB
    subgraph "Client Side"
        A[Listen Socket] --> B[Accept Connection]
        B --> C[TCP Stream]
        C --> D[Async Read/Write]
    end

    subgraph "QUIC Tunnel"
        E[Open Bi-Stream]
        F[Send Stream]
        G[Recv Stream]
    end

    subgraph "Server Side"
        H[Connect to Target]
        I[TCP Stream]
        J[Async Read/Write]
    end
    
    D --> E
    E --> F
    E --> G
    
    F --> J
    G --> D
    J --> H
    
    style E fill:#BBDEFB
    style F fill:#BBDEFB
    style G fill:#BBDEFB
```

### UDP Tunneling Architecture

```mermaid
graph TB
    subgraph "Client Side"
        A[UDP Socket] --> B[Receive Packet]
        B --> C[Track Client Address]
        C --> D[Encode: u16 len + data]
    end

    subgraph "QUIC Tunnel"
        E[Single Bidirectional Stream]
        F[Send Stream]
        G[Recv Stream]
    end

    subgraph "Server Side"
        H[Decode Packet]
        I[Send to Target]
        J[Receive Response]
        K[Encode Response]
    end
    
    subgraph "Return Path"
        L[Send via QUIC]
        M[Decode at Client]
        N[Send to Client]
    end
    
    D --> E
    E --> F
    F --> H
    H --> I
    I --> J
    J --> K
    K --> L
    L --> G
    G --> M
    M --> N
    N --> C
    
    style E fill:#BBDEFB
    style F fill:#BBDEFB
    style G fill:#BBDEFB
    style L fill:#BBDEFB
```

### UDP Packet Framing

```mermaid
graph LR
    subgraph "UDP Packet"
        A[Payload<br/>variable length]
    end
    
    subgraph "QUIC Stream Frame"
        B[Length<br/>u16 BE]
        C[Payload<br/>bytes]
    end
    
    subgraph "Decoding"
        D[Read 2 bytes]
        E[Parse length]
        F[Read N bytes]
        G[Reconstruct packet]
    end
    
    A --> B
    A --> C
    
    B --> D
    D --> E
    E --> F
    C --> F
    F --> G
    
    style B fill:#FFF9C4
    style C fill:#C8E6C9
```

---

## Component Details

### Endpoint (iroh)

The `iroh::Endpoint` provides:

- **Discovery**: Automatic peer discovery via Pkarr/DNS/mDNS
- **Relay**: Fallback relay servers for NAT traversal
- **QUIC**: Built-in QUIC transport with hole punching
- **Identity**: Ed25519-based peer identity and authentication

---

## Performance Considerations

### Connection Establishment Times

> **Note:** These are illustrative, environment-dependent ranges (network conditions, NAT type, relay availability, and DNS). Treat as rough guidance, not guarantees.

```mermaid
graph LR
    subgraph "iroh"
        A[Discovery: 1-3s]
        B[Connection: 0.5-2s]
        C[Total: 1.5-5s]
    end

    style C fill:#FFF9C4
```

### Throughput Characteristics

- **TCP Tunneling**: Limited by QUIC stream flow control and congestion control
- **UDP Tunneling**: Additional framing overhead (2 bytes per packet)
- **Relay Mode**: Higher latency, potentially lower throughput
- **Direct Mode**: Near-native performance with encryption overhead

---

## Error Handling

### Connection Failures

```mermaid
graph TB
    A[Connection Attempt] --> B{Success?}
    B -->|Yes| C[Established]
    B -->|No| E{Relay available?}

    E -->|Yes| F[Fallback to relay]
    E -->|No| G[Connection failed]

    F --> C

    style C fill:#C8E6C9
    style F fill:#FFF9C4
    style G fill:#FFCCBC
```

### Exit Codes (Client Mode)

The client process uses categorized exit codes so wrapper scripts can distinguish
transient failures (retry) from permanent errors (stop):

| Code | Category | Examples |
|------|----------|---------|
| 0 | Success | Normal termination |
| 1 | General error | Unexpected/uncategorized failures |
| 2 | Configuration | Missing `--source`, invalid token format, bad ALPN |
| 3 | Authentication | Token rejected by server, auth response timeout |
| 10 | Connection failed | Relay timeout, endpoint offline, server unreachable |
| 11 | Connection lost | QUIC connection closed after tunnel was established |

Retry guidance:

- **Code 1** — Ambiguous. Retry a limited number of times with backoff; escalate if the error persists.
- **Codes 2, 3** — Do not retry. These require human intervention (fix config or credentials).
- **Code 10** — Connection establishment failed. Retry only if the tunnel has previously connected successfully.
- **Code 11** — Connection lost after the tunnel was working. Always safe to retry.

### Stream Errors

- **TCP**: Connection reset, timeout → close QUIC stream
- **UDP**: Packet loss → no retry (UDP semantics preserved)
- **QUIC**: Stream reset → close local TCP connection or stop UDP forwarding

---

## Capabilities

| Feature | Support |
|---------|---------|
| Multi-Session | **Yes** - Multiple concurrent connections to the same server |
| Dynamic Source | **Yes** - Client specifies which service to tunnel (via `--source`) |
| Encryption | QUIC/TLS 1.3 |
| Platform | Linux, macOS, Windows |

---

## References

- [iroh Documentation](https://iroh.computer/)
- [RFC 9000 - QUIC](https://datatracker.ietf.org/doc/html/rfc9000)
