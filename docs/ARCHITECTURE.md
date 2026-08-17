# tunnel-rs Architecture

This document provides a comprehensive overview of the tunnel-rs architecture, including detailed diagrams of component interactions, data flows, and security considerations.

## Table of Contents

- [System Overview](#system-overview)
- [Features](#features)
- [Tunnel Architecture](#tunnel-architecture)
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
        E2[auth.rs<br/>Ed25519 challenge-response]
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

### NAT Traversal

Provided entirely by iroh and shared with ezvpn and flextunnel: which NAT types
reach a direct path, why symmetric NAT usually stays on the relay, why container
networking depends on the CNI rather than on Kubernetes itself, and what that
means for relay bandwidth. See
[NAT traversal and the QUIC transport](https://github.com/flexaccessdev/iroh-common-architecture/blob/main/nat-traversal-and-transport.md).

---

## Tunnel Architecture

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
        C->>S: Direct QUIC connection (ALPN: mf/4)
        S-->>C: Accept connection
    else NAT Traversal Failed
        C->>RS: Connect via relay
        RS->>S: Forward connection
        S-->>RS: Accept via relay
        RS-->>C: Relay established
    end

    Note over S,C: Encrypted QUIC tunnel established

    Note over C,S: Authentication Phase
    C->>S: Open auth stream
    S->>C: AuthChallenge {random nonce}
    C->>S: AuthRequest {public key, signature}
    alt Key Authorized and Signature Valid
        S-->>C: AuthResponse {accepted: true}
    else Proof Invalid
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

The relay/discovery design is shared with ezvpn and flextunnel and is documented
once in
[iroh-common-architecture / relays-and-address-lookup.md](https://github.com/flexaccessdev/iroh-common-architecture/blob/main/relays-and-address-lookup.md).
In short: `RelayConfig` (`src/iroh_mode/endpoint.rs`) resolves the raw config
once into `Default` or `Custom`, and that single choice decides both the relay
map and whether n0 internet discovery runs. Discovery is not independently
configurable. Every custom relay is probed individually before the real endpoint
binds, and startup fails if any of them is unreachable.

```mermaid
graph TB
    subgraph "Endpoint Creation"
        A[Load/Generate Secret] --> B[Resolve RelayConfig]
        B --> C{Custom relay URLs?}
        C -->|Yes| D[Probe each relay in parallel<br/>all must come online]
        D --> D2[Custom relay map<br/>n0 discovery OFF]
        C -->|No| E[Default relay map<br/>n0 DNS lookup ON<br/>pkarr publish if persistent identity]
        D2 --> F{Relay Only? (CLI-only)}
        E --> F
        F -->|Yes| G[Clear IP transports<br/>no address lookup at all]
        F -->|No| H[Keep IP + relay transports<br/>enable mDNS]
        G --> L[Build + bind Endpoint]
        H --> L
        L --> O[Wait for online, then Ready]
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
    end

    subgraph "iroh Options"
        C[server_node_id<br/>client only]
        D[request_source / target<br/>client only]
        E[allowed_sources / max_sessions<br/>server only]
        F[secret_file / secret*<br/>server only]
        G[authorized_keys_file / authorized_keys*<br/>server only]
        H[private_key_file / private_key*<br/>client only]
        I[relay_urls]
        J[transport<br/>cc + window sizes + ACK threshold]
    end

    A --> S[Validation]
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

`*` marks the inline key forms: a TOML config file carries only the `_file`
paths, while a JSON `--config-stdin` config may carry either. Every other field
is the same in both formats.

### Credential Mapping

tunnel-rs authenticates clients with separate Ed25519 keys. The QUIC ALPN is a fixed value (`mf/4`) shared by all peers and is not configurable; access control happens on the first application stream after the iroh handshake.

| Credential | Env Vars / CLI Flags | Config Key | Expected Usage |
|------------|-----------|-------------|----------------|
| **Client Auth Key** | Server: `TUNNEL_RS_AUTHORIZED_KEYS_FILE` or `--authorized-keys-file`<br>Client: `TUNNEL_RS_PRIVATE_KEY_FILE` or `--private-key-file` | Server: `[iroh].authorized_keys_file`<br>Client: `[iroh].private_key_file` | The client signs a fresh server challenge. The key is not used as the iroh transport identity. |

Example usage with files (recommended):

```bash
# Alice — generate a compact private key with the flexaccess-keys CLI,
# then derive its public entry
flexaccess-keys generate-auth-key alice --output alice.key
flexaccess-keys show-auth-key --private-key-file alice.key > alice.pub

# Alice — reprint that entry later, straight from the private key
flexaccess-keys show-auth-key --private-key-file ./alice.key

# Server — authorize the public entry
cat alice.pub >> authorized_keys
tunnel-rs server --authorized-keys-file ./authorized_keys ...

# Alice's client
tunnel-rs client --private-key-file ./alice.key ...
```

Example config usage:

```toml
# server.toml — using _file variants
[iroh]
authorized_keys_file = "/etc/tunnel-rs/authorized_keys"

# client.toml — using _file variants
[iroh]
private_key_file = "~/.config/tunnel-rs/client.key"
```

A JSON stdin config may instead carry the keys themselves — `authorized_keys`
(an array of authorized-keys lines) on the server and `private_key` (the bare
token or a whole key file) on the client. Those inline forms are rejected in
TOML, where they would outlive the process in VCS and backups.

### Configuration Loading Flow

For normal usage, prefer file-based configs (`-c`, `--default-config`) which use TOML — settings are saved and reusable. The `--config-stdin` flag is intended for automation and IPC only; it uses JSON because JSON is self-delimiting — `serde_json::from_reader` parses exactly one JSON object and returns without waiting for EOF, allowing the caller to keep stdin open. Config passed via stdin is not persisted.

Both formats reject unknown fields, so a misspelled key fails at startup instead of silently doing nothing.

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

    F --> G{Unknown fields?}
    G -->|Yes| H[Error: unknown field]
    G -->|No| I{Required fields?}

    I -->|Missing| J[Error: Missing field]
    I -->|Present| K[Validation Success]

    style C fill:#FFCCBC
    style H fill:#FFCCBC
    style J fill:#FFCCBC
    style K fill:#C8E6C9
```

---

## Security Model

### Encryption Stack

All traffic is end-to-end encrypted by QUIC/TLS 1.3 between the two endpoints;
relay operators forward ciphertext and see only connection metadata. The stack is
iroh's and identical across tunnel-rs, ezvpn, and flextunnel — see
[NAT traversal and the QUIC transport](https://github.com/flexaccessdev/iroh-common-architecture/blob/main/nat-traversal-and-transport.md#encryption-stack).

ALPN is split between the two layers. The *mechanism* — carrying the protocol
identifier in the TLS 1.3 handshake and failing the connection when the two sides
disagree — is TLS's, and iroh exposes it as the ALPN passed to
`Endpoint::connect` and `alpns()`. The *value* is tunnel-rs's own choice: the
fixed `mf/4` (`TUNNEL_ALPN` in `src/iroh_mode/endpoint.rs`), shared by every
tunnel-rs peer and not configurable, which keeps peers of the other two programs
from completing a handshake at all. It is not a secret and grants nothing; the
Ed25519 challenge-response below is what authorizes the *user* rather than the
endpoint.

### Identity and Authentication

```mermaid
graph TB
    subgraph "Identity and Authentication"
        A[Server Secret Key] --> B[Ed25519 Private Key]
        B --> C[EndpointId - Public Key]
        C --> D[Client Connects]
        D --> E[Ed25519 Challenge-Response]
        E --> F{Valid Authorized Proof?}
        F -->|Yes| G[Authenticated]
        F -->|No| H[Rejected]
    end

    style B fill:#FFE0B2
    style C fill:#C8E6C9
    style G fill:#C8E6C9
    style H fill:#FFCCBC
```

### Ed25519 Public-Key Authentication

The QUIC ALPN remains the fixed value `mf/4`. Once the iroh connection is
established, the client opens a dedicated authentication stream. The server
issues a fresh 32-byte random challenge, and the client signs a
domain-separated transcript with its Ed25519 authentication key. This key is
separate from the ephemeral iroh client identity.

1. **Shared key layer**: [`flexaccess-keys`](https://github.com/flexaccessdev/flexaccess-keys) owns the app-independent `ed25519-sec:` / `ed25519-pub:` format, secure key-file output, authorized-key parsing, raw Ed25519 operations, and the key-management CLI (`flexaccess-keys generate-auth-key` / `show-auth-key`). tunnel-rs has no key-management commands of its own.
2. **Configuration**: `authorized_keys_file` points to shared-format public entries, and `private_key_file` points to the compact private-key file generated by `flexaccess-keys generate-auth-key`.
3. **Protocol Flow**: The server sends `AuthChallenge`; the client returns `AuthRequest { public_key, signature }`. **No source requests are accepted until authentication succeeds.**
4. **Validation**: `auth.rs` checks that the public key is authorized and strictly verifies the Ed25519 signature within the 10-second auth timeout.
5. **Rejection**: Unknown keys, malformed proofs, and invalid signatures close the connection with an authentication error.

This validation prevents unauthorized clients from holding open connections or attempting source requests.

```mermaid
sequenceDiagram
    participant C as Client
    participant S as Server
    participant A as Auth Module

    C->>S: Connect (QUIC TLS handshake, ALPN: mf/4)
    S->>C: Accept connection

    Note over C,S: Auth Phase (10s timeout)
    C->>S: Open auth stream
    S->>C: AuthChallenge {version, random challenge}
    C->>A: Sign domain-separated challenge
    C->>S: AuthRequest {version, public_key, signature}
    S->>A: Check authorized key and verify signature
    alt Proof is valid
        A-->>S: Authorized-key comment
        S->>C: AuthResponse {accepted: true}
        Note over S,C: Connection authenticated
    else Proof is invalid
        A-->>S: Rejected
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

### Public-Key Authentication Security Notes

- The private key never crosses the wire; only its public key and a signature over a fresh challenge are sent.
- A new random challenge prevents replaying a proof from another connection.
- Domain separation prevents the signature from being valid for an unrelated Ed25519 protocol.
- Removing a public entry from `authorized_keys` revokes that client after the server is restarted.
- Compact private-key files are created with `0600` permissions on Unix and include their public entry as a comment for administration. Server secret key files carry the same headers, naming their EndpointId; comment lines are skipped when a key is loaded, so a whole key file is also accepted as an inline `secret`.
- The fixed `mf/4` ALPN is not a secret and is unchanged by the authentication mechanism.

### Threat Model

```mermaid
graph TB
    subgraph "Protected Against"
        A[Eavesdropping<br/>TLS 1.3 encryption]
        B[MITM<br/>Peer authentication]
        C[Replay Attacks<br/>Fresh auth challenges]
        D[Tampering<br/>Authenticated encryption]
        E2[Unauthorized Access<br/>Ed25519 Public-Key Authentication]
    end

    subgraph "User Responsibility"
        F[Secret Key Protection<br/>iroh server]
        G[EndpointId Verification<br/>Trust on first use]
        H[Client Private-Key Security]
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

Only the **server** needs a persistent transport secret key to maintain a stable EndpointId. Clients use ephemeral iroh identities. Their separate persistent Ed25519 keys are used only for application authentication after the transport handshake.

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
        Secret->>FS: Write "# EndpointId" header + key (0600)
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

`iroh::Endpoint` supplies discovery, NAT traversal, relay fallback, the QUIC
transport, and Ed25519 endpoint identity. tunnel-rs configures it in
`src/iroh_mode/endpoint.rs` and adds nothing to the transport itself. For what
the endpoint does and how it behaves, see
[NAT traversal and the QUIC transport](https://github.com/flexaccessdev/iroh-common-architecture/blob/main/nat-traversal-and-transport.md)
and
[relays and address lookup](https://github.com/flexaccessdev/iroh-common-architecture/blob/main/relays-and-address-lookup.md).

---

## Performance Considerations

### Connection Establishment Times

Establishment timings (discovery, hole punching, relay fallback) are iroh's and
shared across the three programs — see
[performance characteristics](https://github.com/flexaccessdev/iroh-common-architecture/blob/main/nat-traversal-and-transport.md#performance-characteristics).

### Throughput Characteristics

Specific to tunnel-rs's tunneling layer:

- **TCP Tunneling**: Limited by QUIC stream flow control, congestion control, and optional ACK frequency tuning
- **UDP Tunneling**: Additional framing overhead (2 bytes per packet)

Direct-vs-relay path performance is a property of the iroh transport — see
[performance characteristics](https://github.com/flexaccessdev/iroh-common-architecture/blob/main/nat-traversal-and-transport.md#performance-characteristics).

---

## Error Handling

Connection establishment and its relay fallback are handled by iroh (see
[NAT traversal and the QUIC transport](https://github.com/flexaccessdev/iroh-common-architecture/blob/main/nat-traversal-and-transport.md)).
What follows is how tunnel-rs surfaces the outcome.

### Exit Codes (Client Mode)

The client process uses categorized exit codes so wrapper scripts can distinguish
transient failures (retry) from permanent errors (stop):

| Code | Category | Examples |
|------|----------|---------|
| 0 | Success | Normal termination |
| 1 | General error | Unexpected/uncategorized failures |
| 2 | Configuration | Missing `--source`, invalid key format |
| 3 | Authentication | Signature rejected by server, auth response timeout |
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

- [iroh-common-architecture](https://github.com/flexaccessdev/iroh-common-architecture) —
  shared iroh transport design for tunnel-rs, ezvpn, and flextunnel: [NAT
  traversal and the QUIC transport](https://github.com/flexaccessdev/iroh-common-architecture/blob/main/nat-traversal-and-transport.md),
  [relays and address lookup](https://github.com/flexaccessdev/iroh-common-architecture/blob/main/relays-and-address-lookup.md),
  [relay self-hosting](https://github.com/flexaccessdev/iroh-common-architecture/blob/main/self-hosting.md),
  the discovery findings, and the relay connection trace
- [iroh Documentation](https://iroh.computer/)
- [RFC 9000 - QUIC](https://datatracker.ietf.org/doc/html/rfc9000)
