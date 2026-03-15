# tunnel-rs Architecture

This document provides a comprehensive overview of the tunnel-rs architecture, including detailed diagrams of all operational modes, component interactions, data flows, and security considerations.

## Table of Contents

- [System Overview](#system-overview)
- [Mode Comparison](#mode-comparison)
- [iroh Mode Architecture](#iroh-mode-architecture)
- [Configuration System](#configuration-system)
- [Security Model](#security-model)
- [Protocol Support](#protocol-support)
- [Component Details](#component-details)
- [Performance Considerations](#performance-considerations)
- [Error Handling](#error-handling)
- [Mode Capabilities](#mode-capabilities)
- [Current Limitations](#current-limitations)
- [References](#references)

---

## System Overview

tunnel-rs is a P2P TCP/UDP port forwarding tool that supports multiple distinct operational modes, each optimized for different use cases and network environments.

Binary layout:
- `tunnel-rs`: iroh mode (port forwarding)
- `tunnel-rs-ice`: manual and nostr modes (port forwarding)

> **Design Goal:** The project's primary goal is to provide a convenient way to connect to different networks for development or homelab purposes without the hassle and security risk of opening a port. It is **not** meant for production setups or designed to be performant at scale.

```mermaid
graph TB
    subgraph "tunnel-rs Modes"
        A[iroh]
        C[manual]
        D2[nostr]
    end

    subgraph "Use Cases"
        D[Best NAT Traversal<br/>Relay Fallback]
        F[Manual Signaling<br/>Full ICE]
        F2[Automated Signaling<br/>Static Keys]
    end

    subgraph "Infrastructure"
        G[Pkarr/DNS<br/>Relay Servers]
        I[STUN Only]
        I2[STUN + Nostr Relays]
    end

    A --> D
    C --> F
    D2 --> F2

    A --> G
    C --> I
    D2 --> I2

    style A fill:#4CAF50
    style C fill:#FF9800
    style D2 fill:#9C27B0
```

### Binaries & Crates

The project is split into separate binaries to isolate dependencies:

| Binary | Modes | Key Modules |
|--------|-------|-------------|
| `tunnel-rs` | `iroh` | `iroh_mode`, `auth` |
| `tunnel-rs-ice` | `manual`, `nostr` | `custom`, `nostr`, `transport` |

Relay-only is a CLI-only flag that forces connections through relay servers instead of attempting direct connections. It is intended for testing or special scenarios and is not supported in config files to avoid accidental activation. See `tunnel-rs --help` for usage.

### Core Components

```mermaid
graph LR
    subgraph "Core Modules"
        A[main.rs<br/>CLI & Orchestration]
        B[config.rs<br/>Configuration]
        C[tunnel.rs<br/>TCP/UDP Forwarding]
        D[endpoint.rs<br/>iroh Endpoint]
        E[secret.rs<br/>Identity Management]
        E2[auth.rs<br/>Token Authentication]
    end

    subgraph "Manual/Custom Mode"
        F[transport/ice.rs<br/>ICE with str0m]
        G[transport/quic.rs<br/>QUIC with quinn]
        H[signaling/manual.rs<br/>Offer/Answer]
        I[transport/mux.rs<br/>Stream Multiplexing]
    end

    subgraph "Nostr Mode"
        J[signaling/nostr.rs<br/>Nostr Relay Signaling]
    end

    A --> B
    A --> C
    A --> D
    A --> E
    A --> E2
    A --> F
    A --> G
    A --> H
    A --> J

    F --> G
    H --> F
    J --> H
    G --> I

    style A fill:#E3F2FD
    style C fill:#E8F5E9
    style E2 fill:#FFCCBC
    style F fill:#FFF3E0
    style G fill:#FFF3E0
    style J fill:#E1BEE7
```

---

## Mode Comparison

> **Tip for Containerized Environments:** Use `iroh` mode for Docker, Kubernetes, and cloud VM deployments. It includes relay fallback which ensures connectivity even when both peers are behind restrictive NATs (common in cloud environments). The `nostr` and `manual` modes use STUN-only NAT traversal which may fail in these environments.

### Feature Matrix

```mermaid
graph TD
    subgraph "iroh"
        A1[Discovery: Automatic]
        A2[NAT: Relay Fallback]
        A3[Setup: Minimal - EndpointId required]
        A4[Infrastructure: Required]
    end

    subgraph "manual"
        C1[Discovery: Copy-Paste]
        C2[NAT: Full ICE]
        C3[Setup: Manual Exchange]
        C4[Infrastructure: STUN Only]
    end

    style A1 fill:#C8E6C9
    style A2 fill:#C8E6C9
    style A3 fill:#C8E6C9
    style A4 fill:#FFCCBC

    style C1 fill:#FFE0B2
    style C2 fill:#C8E6C9
    style C3 fill:#FFE0B2
    style C4 fill:#C8E6C9
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

    subgraph "manual"
        G1[✓ Direct]
        G2[✓ Direct]
        G3[✓ Direct]
        G4[~ Best-effort<br/>may fail without relay]
    end

    A --> E1
    B --> E2
    C --> E3
    D --> E4

    A --> G1
    B --> G2
    C --> G3
    D --> G4

    style E1 fill:#C8E6C9
    style E2 fill:#C8E6C9
    style E3 fill:#C8E6C9
    style E4 fill:#C8E6C9

    style G1 fill:#C8E6C9
    style G2 fill:#C8E6C9
    style G3 fill:#C8E6C9
    style G4 fill:#FFF9C4
```

---

## iroh Mode Architecture

> For manual and nostr mode architecture, see [docs/ALTERNATIVE-MODES.md](ALTERNATIVE-MODES.md).

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
        S-->>C: AuthResponse {accepted}
    else Token Invalid
        S->>S: Wait out remaining auth timeout
        S->>S: Drop connection silently
    end

    Note over C,S: Source Request Phase
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
        B[mode: iroh/manual/nostr]
        C[source/target: tcp://host:port or udp://host:port]
    end

    subgraph "Mode Sections"
        E[iroh]
        G[manual]
        H[nostr]
    end

    subgraph "iroh Options"
        I[secret_file]
        I2[auth_tokens - server only]
        I3[auth_token - client only]
        I4[alpn_token - required on both]
        J[relay_urls]
        L[dns_server]
        M[server_node_id - client only]
    end

    subgraph "manual Options"
        N[stun_servers]
    end

    subgraph "nostr Options"
        O[nsec/nsec_file]
        P[peer_npub]
        Q[relays]
        R[stun_servers]
    end

    A --> S[Validation]
    B --> S
    S --> E
    S --> G
    S --> H

    E --> I
    E --> I2
    E --> I3
    E --> I4
    E --> J
    E --> L
    E --> M

    G --> N
    H --> O
    H --> P
    H --> Q
    H --> R

    style S fill:#FFF9C4
```

### iroh Credential Mapping

`iroh` mode uses two distinct credential types:

| Credential | CLI Flags | Config Keys | Expected Usage |
|------------|-----------|-------------|----------------|
| **ALPN Token** | Server: `--alpn-token`<br>Client: `--alpn-token` | Server/Client: `[iroh].alpn_token` | Pre-handshake QUIC ALPN filter (`mf/2/<token>`). Typically one shared value for a server and all its clients. |
| **Auth Token** | Server: `--auth-tokens` and/or `--auth-tokens-file`<br>Client: `--auth-token` or `--auth-token-file` | Server: `[iroh].auth_tokens` or `[iroh].auth_tokens_file`<br>Client: `[iroh].auth_token` or `[iroh].auth_token_file` | Per-client credential checked on the auth stream after handshake. Use separate values per client for revocation/rotation. |

Example CLI usage:

```bash
# Server
tunnel-rs server \
  --alpn-token "$ALPN_TOKEN" \
  --auth-tokens "$ALICE_AUTH_TOKEN" \
  --auth-tokens "$BOB_AUTH_TOKEN"

# Alice's client
tunnel-rs client \
  --alpn-token "$ALPN_TOKEN" \
  --auth-token "$ALICE_AUTH_TOKEN"
```

Example config usage:

```toml
# server.toml
[iroh]
alpn_token = "ALPN_TOKEN_SHARED_BY_ALL_CLIENTS"
auth_tokens = ["ALICE_AUTH_TOKEN", "BOB_AUTH_TOKEN"]

# client.toml
[iroh]
alpn_token = "ALPN_TOKEN_SHARED_BY_ALL_CLIENTS"
auth_token = "ALICE_AUTH_TOKEN"
```

### Configuration Loading Flow

For normal usage, prefer file-based configs (`-c`, `--default-config`) which use TOML — settings are saved and reusable. The `--config-stdin` flag is intended for automation and IPC only; it uses JSON because JSON is self-delimiting — `serde_json::from_reader` parses exactly one JSON object and returns without waiting for EOF, allowing the caller to keep stdin open (e.g., for manual mode signaling). Config passed via stdin is not persisted.

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
        Config->>Source: Read ~/.config/tunnel-rs/{role}.toml or {role}_ice.toml
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

Note: For `tunnel-rs-ice`, the mode is inferred from the config file, so `server -c <file>` / `client -c <file>` can be used without a subcommand.

### Config Validation

```mermaid
graph TB
    A[Load Config] --> B{Role matches?}
    B -->|No| C[Error: Role mismatch]
    B -->|Yes| D{Mode matches?}
    D -->|No| E[Error: Mode mismatch]
    D -->|Yes| F{Check sections}
    
    F --> G{Extra sections?}
    G -->|Yes| H[Ignored by parser]
    G -->|No| I{Required fields?}
    
    I -->|Missing| J[Error: Missing field]
    I -->|Present| K[Validation Success]
    
    style C fill:#FFCCBC
    style E fill:#FFCCBC
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

    subgraph "manual Mode"
        I[ICE Credentials] --> J[ufrag + pwd]
        J --> K[STUN Auth]
        K --> L[QUIC TLS]
    end

    style B fill:#FFE0B2
    style C fill:#C8E6C9
    style G fill:#C8E6C9
    style H fill:#FFCCBC
    style L fill:#C8E6C9
```

### Token Authentication (iroh Mode)

Iroh mode uses two layers of authentication. First, a pre-shared ALPN token is embedded in the QUIC protocol identifier (`mf/2/<token>`), rejecting unknown clients at the TLS handshake level before any application streams are opened. Second, clients must provide a valid auth token via a dedicated auth stream within a 10-second timeout. **Both layers are mandatory.**

#### ALPN Token vs Auth Token

- **ALPN Token** (`--alpn-token` / `[iroh].alpn_token`): Pre-handshake shared value used for QUIC ALPN filtering.
- **Auth Token** (server: `--auth-tokens` / `--auth-tokens-file`; client: `--auth-token` / `--auth-token-file`; config: `[iroh].auth_tokens` / `[iroh].auth_token`): Per-client token validated on the auth stream.
- **Mapping**: These are **distinct tokens**, not the same value. In code, ALPN tokens are 14-char Base64URL values, while auth tokens are 47-char `i...` tokens. Typical setup is one shared ALPN token plus per-client auth tokens for revocation.

1. **ALPN Filtering**: Both server and client specify `--alpn-token`. The token is embedded in the QUIC ALPN identifier (`mf/2/<token>`). Connections from clients without a matching ALPN are rejected at the handshake level — acting as a lightweight "port knock".
2. **Server Configuration**: Server specifies `--auth-tokens` with one or more pre-shared tokens
3. **Client Configuration**: Client specifies `--auth-token` with the token received from the server admin
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
        E[Signaling Channel Security<br/>Manual modes]
        F[Secret Key Protection<br/>iroh server]
        G[EndpointId Verification<br/>Trust on first use]
        H[Auth Token Security<br/>Treat tokens like passwords]
    end

    style A fill:#C8E6C9
    style B fill:#C8E6C9
    style C fill:#C8E6C9
    style D fill:#C8E6C9
    style E2 fill:#C8E6C9

    style E fill:#FFF9C4
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

### IceAgent (str0m)

The `IceAgent` from str0m handles ICE connectivity establishment:

- **Candidate Gathering**: Discovers local and server-reflexive addresses
- **Connectivity Checks**: Performs STUN binding checks to all candidate pairs
- **Nomination**: Selects the best working candidate pair
- **Socket Management**: Provides the UDP socket for QUIC transport

### QUIC Endpoint (quinn)

The `quinn` QUIC implementation provides:

- **TLS 1.3**: Encrypted transport with certificate-based auth
- **Stream Multiplexing**: Multiple concurrent streams over one connection
- **Congestion Control**: Built-in congestion control and flow control
- **0-RTT**: Not currently enabled (future optimization)

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

    subgraph "manual"
        H[ICE Gather: 1-2s]
        I[Manual: User dependent]
        J[ICE Checks: 1-3s]
        K[QUIC: 0.5s]
        L[Total: 2.5-5.5s + manual]
    end

    style C fill:#FFF9C4
    style L fill:#FFF9C4
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
    B -->|No| D{Mode?}
    
    D -->|iroh| E{Relay available?}
    E -->|Yes| F[Fallback to relay]
    E -->|No| G[Connection failed]

    D -->|manual| I[ICE checks failed]
    
    F --> C
    H --> G
    I --> G
    
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
- **Code 10** — Connection establishment failed. Retry only if the tunnel has previously connected successfully (a prior exit 11 implies prior success).
- **Code 11** — Connection lost after the tunnel was working. Always safe to retry.

**Mode compatibility:** Auto-retry wrapper scripts work with **iroh mode** and **nostr mode**
(both use automatic signaling). **Manual mode** requires interactive copy-paste SDP exchange
each run, so auto-retry is not feasible.

### Stream Errors

- **TCP**: Connection reset, timeout → close QUIC stream
- **UDP**: Packet loss → no retry (UDP semantics preserved)
- **QUIC**: Stream reset → close local TCP connection or stop UDP forwarding

---

## Mode Capabilities

| Mode | Multi-Session | Dynamic Source | Encryption | Platform |
|------|---------------|----------------|------------|----------|
| `iroh` | **Yes** | **Yes** | QUIC/TLS 1.3 | Linux, macOS, Windows |
| `nostr` | **Yes** | **Yes** | QUIC/TLS 1.3 | Linux, macOS, Windows |
| `manual` | No | **Yes** | QUIC/TLS 1.3 | Linux, macOS, Windows |

**Multi-Session** = Multiple concurrent connections to the same server
**Dynamic Source** = Client specifies which service to tunnel (via `--source`)

---

## Current Limitations

### Single Session (Manual Signaling Mode)

The `manual` mode currently supports only one tunnel session at a time per server instance. Each signaling exchange establishes exactly one tunnel.

```mermaid
graph TB
    subgraph "manual Behavior"
        A[Server starts] --> B[Wait for client offer]
        B --> C[Validate source request]
        C --> D[Establish single tunnel]
        D --> E[Handle streams over this tunnel]
        E --> F[Additional clients timeout]
    end

    subgraph "Workarounds"
        G[Run multiple server instances]
        I[Use iroh mode]
    end

    style F fill:#FFCCBC
    style I fill:#C8E6C9
```

**Why this limitation exists:**
- Manual signaling mode performs a single offer/answer exchange
- The server enters a connection handling loop after establishing the tunnel
- No mechanism to accept additional signaling while serving existing tunnel

**Workarounds:**
- Use `iroh` mode for multi-client support
- Run separate server instances for each tunnel

Use `iroh` or `nostr` mode for multi-session support.

---

## References

- [iroh Documentation](https://iroh.computer/)
- [str0m ICE Implementation](https://github.com/algesten/str0m)
- [quinn QUIC Implementation](https://github.com/quinn-rs/quinn)
- [RFC 8445 - ICE](https://datatracker.ietf.org/doc/html/rfc8445)
- [RFC 9000 - QUIC](https://datatracker.ietf.org/doc/html/rfc9000)
