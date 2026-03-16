//! Iroh multi-source tunnel mode.
//!
//! This mode provides relay-based tunneling with automatic discovery.
//! Clients can request specific sources (tcp://host:port or udp://host:port),
//! and servers validate requests against allowed CIDR lists.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use crate::error::TunnelError;
use iroh::{EndpointId, SecretKey};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::Mutex;
use tokio::task::JoinSet;
use crate::config::TransportTuning;

/// Configuration for the multi-source server.
pub struct MultiSourceServerConfig {
    /// Allowed TCP destination networks in CIDR notation (e.g., "127.0.0.0/8").
    pub allowed_tcp: Vec<String>,
    /// Allowed UDP destination networks in CIDR notation.
    pub allowed_udp: Vec<String>,
    /// Maximum number of concurrent sessions (None = use default).
    pub max_sessions: Option<usize>,
    /// Iroh secret key for the endpoint. **Sensitive field - redacted in Debug output.**
    pub secret: Option<SecretKey>,
    /// Iroh relay URLs.
    pub relay_urls: Vec<String>,
    /// Whether to use relay-only mode (disables direct P2P).
    pub relay_only: bool,
    /// Custom DNS server for resolution, or "none" to disable DNS discovery.
    /// mDNS for local network discovery is unaffected.
    pub dns_server: Option<String>,
    /// Set of valid authentication tokens. **Sensitive field - redacted in Debug output.**
    pub auth_tokens: HashSet<String>,
    /// ALPN token for QUIC handshake-level filtering. **Sensitive field - redacted in Debug output.**
    pub alpn_token: String,
    /// Transport layer tuning (congestion control, buffer sizes).
    pub transport: TransportTuning,
}

impl std::fmt::Debug for MultiSourceServerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MultiSourceServerConfig")
            .field("allowed_tcp", &self.allowed_tcp)
            .field("allowed_udp", &self.allowed_udp)
            .field("max_sessions", &self.max_sessions)
            .field("secret", &self.secret.as_ref().map(|_| "[REDACTED]"))
            .field("relay_urls", &self.relay_urls)
            .field("relay_only", &self.relay_only)
            .field("dns_server", &self.dns_server)
            .field(
                "auth_tokens",
                &format!("[{} tokens]", self.auth_tokens.len()),
            )
            .field("alpn_token", &"[REDACTED]")
            .field("transport", &self.transport)
            .finish()
    }
}

/// Configuration for the multi-source client.
pub struct MultiSourceClientConfig {
    /// Server's Iroh endpoint ID (node ID) to connect to.
    pub node_id: String,
    /// Source URL to request from server (e.g., "tcp://host:port" or "udp://host:port").
    pub source: String,
    /// Local target address to listen on (e.g., "127.0.0.1:2222").
    pub target: String,
    /// Iroh relay URLs.
    pub relay_urls: Vec<String>,
    /// Whether to use relay-only mode (disables direct P2P).
    pub relay_only: bool,
    /// Custom DNS server for resolution, or "none" to disable DNS discovery.
    /// mDNS for local network discovery is unaffected.
    pub dns_server: Option<String>,
    /// Authentication token for server access. **Sensitive field - redacted in Debug output.**
    pub auth_token: String,
    /// ALPN token for QUIC handshake-level filtering. **Sensitive field - redacted in Debug output.**
    pub alpn_token: String,
    /// Transport layer tuning (congestion control, buffer sizes).
    pub transport: TransportTuning,
}

impl std::fmt::Debug for MultiSourceClientConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MultiSourceClientConfig")
            .field("node_id", &self.node_id)
            .field("source", &self.source)
            .field("target", &self.target)
            .field("relay_urls", &self.relay_urls)
            .field("relay_only", &self.relay_only)
            .field("dns_server", &self.dns_server)
            .field("auth_token", &"[REDACTED]")
            .field("alpn_token", &"[REDACTED]")
            .field("transport", &self.transport)
            .finish()
    }
}

use crate::auth::is_token_valid;

use crate::iroh_mode::endpoint::{
    build_multi_alpn, connect_to_server, create_client_endpoint, create_server_endpoint,
    validate_relay_only, watch_connection_paths,
};
use crate::iroh_mode::helpers::{
    bridge_streams, forward_stream_to_udp_client, forward_stream_to_udp_server,
    forward_udp_to_stream, open_bi_with_retry,
};
use crate::net::{
    bind_udp_for_targets, check_source_allowed, extract_addr_from_source, resolve_all_target_addrs,
    resolve_listen_addrs, validate_allowed_networks,
};
use crate::signaling::{
    decode_auth_request, decode_auth_response, decode_source_request, decode_source_response,
    encode_auth_request, encode_auth_response, encode_source_request, encode_source_response,
    read_length_prefixed, AuthRequest, AuthResponse, SourceRequest, SourceResponse,
};

/// Default maximum concurrent sessions for multi-source mode.
const DEFAULT_MAX_SESSIONS: usize = 100;

/// Timeout for receiving authentication request after connection.
const AUTH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Connection close code for authentication failure (invalid token).
const AUTH_FAILED_CODE: u32 = 1;

/// Connection close code for authentication timeout (no auth within deadline).
const AUTH_TIMEOUT_CODE: u32 = 2;

// ============================================================================
// Server
// ============================================================================

/// Run iroh multi-source server.
///
/// This mode allows clients to request specific sources (tcp://host:port or udp://host:port).
/// The server validates requests against allowed_tcp and allowed_udp CIDR lists.
/// Authentication is enforced via pre-shared tokens - clients must provide a valid token in their request.
pub async fn run_multi_source_server(config: MultiSourceServerConfig) -> Result<()> {
    let relay_only = config.relay_only;

    // Validate CIDR notation at startup
    validate_allowed_networks(&config.allowed_tcp, "--allowed-tcp")?;
    validate_allowed_networks(&config.allowed_udp, "--allowed-udp")?;

    if config.allowed_tcp.is_empty() && config.allowed_udp.is_empty() {
        anyhow::bail!(
            "At least one --allowed-tcp or --allowed-udp network must be specified.\n\
            Example: --allowed-tcp 127.0.0.0/8 --allowed-udp 10.0.0.0/8"
        );
    }

    if config.auth_tokens.is_empty() {
        anyhow::bail!(
            "At least one authentication token must be configured.\n\
            Use --auth-tokens <TOKEN> or --auth-tokens-file <FILE>.\n\
            Generate tokens with: tunnel-rs generate-token"
        );
    }

    validate_relay_only(relay_only, &config.relay_urls)?;

    log::info!("Multi-Source Tunnel - Server Mode");
    log::info!("==================================");
    log::info!("Creating iroh endpoint...");

    let alpn = build_multi_alpn(&config.alpn_token);
    let endpoint = create_server_endpoint(
        &config.relay_urls,
        relay_only,
        config.secret,
        config.dns_server.as_deref(),
        &alpn,
        Some(&config.transport),
    )
    .await?;

    let endpoint_id = endpoint.id();
    let max_sessions = config.max_sessions.unwrap_or(DEFAULT_MAX_SESSIONS);

    log::info!("\nEndpointId: {}", endpoint_id);
    log::info!("Allowed TCP networks: {:?}", config.allowed_tcp);
    log::info!("Allowed UDP networks: {:?}", config.allowed_udp);
    log::info!("Max concurrent sessions: {}", max_sessions);
    log::info!("Auth tokens configured: {}", config.auth_tokens.len());

    // Wrap auth_tokens in Arc for cheap cloning across tasks
    let auth_tokens = Arc::new(config.auth_tokens);
    let allowed_tcp = config.allowed_tcp;
    let allowed_udp = config.allowed_udp;

    log::info!("\nOn the client side, run:");
    log::info!(
        "  tunnel-rs client --auth-token <token> --alpn-token <alpn-token> --server-node-id {} --source tcp://target:port --target 127.0.0.1:port\n",
        endpoint_id
    );
    log::info!("Note: Both --auth-token and --alpn-token (or --alpn-token-file) are required to connect");
    log::info!("Waiting for clients to connect...");

    // Session management with semaphore for concurrency limit
    let session_semaphore = Arc::new(tokio::sync::Semaphore::new(max_sessions));
    let mut connection_tasks: JoinSet<()> = JoinSet::new();

    loop {
        // Clean up completed tasks
        while connection_tasks.try_join_next().is_some() {}

        let incoming = match endpoint.accept().await {
            Some(incoming) => incoming,
            None => {
                log::info!("Endpoint closed");
                break;
            }
        };

        let conn = match incoming.await {
            Ok(conn) => conn,
            Err(e) => {
                log::warn!("Failed to accept connection: {}", e);
                continue;
            }
        };

        let remote_id = conn.remote_id();

        log::info!("Client connected: {} (awaiting auth)", remote_id);

        // Clone for the spawned task
        let allowed_tcp = allowed_tcp.clone();
        let allowed_udp = allowed_udp.clone();
        let semaphore = session_semaphore.clone();
        let auth_tokens = Arc::clone(&auth_tokens);

        connection_tasks.spawn(async move {
            if let Err(e) = handle_multi_source_connection(
                conn,
                allowed_tcp,
                allowed_udp,
                semaphore,
                auth_tokens,
            )
            .await
            {
                log::warn!("Connection error for {}: {}", remote_id, e);
            }
        });
    }

    // Wait for remaining tasks to complete
    connection_tasks.shutdown().await;
    endpoint.close().await;
    log::info!("Multi-source server stopped.");

    Ok(())
}

/// Handle a single multi-source connection.
/// First authenticates the client on a dedicated auth stream, then handles source request streams.
async fn handle_multi_source_connection(
    conn: iroh::endpoint::Connection,
    allowed_tcp: Vec<String>,
    allowed_udp: Vec<String>,
    semaphore: Arc<tokio::sync::Semaphore>,
    auth_tokens: Arc<HashSet<String>>,
) -> Result<()> {
    let remote_id = conn.remote_id();

    // Phase 1: Wait for auth stream with timeout
    let auth_result = tokio::time::timeout(AUTH_TIMEOUT, async {
        // Accept the first bi-stream which must be the auth stream
        let (mut send_stream, mut recv_stream) = conn
            .accept_bi()
            .await
            .context("Failed to accept auth stream")?;

        // Read AuthRequest
        let request_bytes = read_length_prefixed(&mut recv_stream)
            .await
            .context("Failed to read auth request")?;
        let request = decode_auth_request(&request_bytes).context("Invalid auth request")?;

        // Validate token
        let token_str = request.auth_token.as_str();
        if !is_token_valid(token_str, &auth_tokens) {
            log::warn!("Invalid auth token from {}", remote_id);
            let response = AuthResponse::rejected("Invalid authentication token");
            let encoded = encode_auth_response(&response)?;
            send_stream.write_all(&encoded).await?;
            send_stream.finish()?;
            anyhow::bail!("Invalid auth token");
        }

        // Send success response
        let response = AuthResponse::accepted();
        let encoded = encode_auth_response(&response)?;
        send_stream.write_all(&encoded).await?;
        send_stream.finish()?;

        log::info!("Client {} authenticated successfully", remote_id);
        Ok::<_, anyhow::Error>(())
    })
    .await;

    match auth_result {
        Ok(Ok(())) => {
            // Authentication succeeded, proceed to handle source streams
        }
        Ok(Err(e)) => {
            log::warn!("Authentication failed for {}: {}", remote_id, e);
            conn.close(AUTH_FAILED_CODE.into(), b"auth_failed");
            return Err(anyhow::anyhow!("auth_failed: {}", e));
        }
        Err(_) => {
            log::warn!("Authentication timeout for {}", remote_id);
            conn.close(AUTH_TIMEOUT_CODE.into(), b"auth_timeout");
            return Err(anyhow::anyhow!("auth_timeout"));
        }
    }

    // Monitor connection path changes (e.g., relay -> direct)
    let _path_watcher = watch_connection_paths(&conn);

    // Phase 2: Handle source streams (existing logic)
    let mut stream_tasks: JoinSet<()> = JoinSet::new();

    loop {
        tokio::select! {
            accept_result = conn.accept_bi() => {
                let (send_stream, recv_stream) = match accept_result {
                    Ok(streams) => streams,
                    Err(e) => {
                        log::info!("Client {} disconnected: {}", remote_id, e);
                        break;
                    }
                };

                // Try to acquire a session permit
                let permit = match semaphore.clone().try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        log::warn!("Session limit reached, rejecting stream from client {}", remote_id);
                        // Send rejection and close stream
                        let response = SourceResponse::rejected("Session limit reached");
                        match encode_source_response(&response) {
                            Ok(encoded) => {
                                let mut send = send_stream;
                                if let Err(e) = send.write_all(&encoded).await {
                                    log::warn!("Failed to write rejection response to client {}: {}", remote_id, e);
                                }
                                if let Err(e) = send.finish() {
                                    log::warn!("Failed to finish rejection stream to client {}: {}", remote_id, e);
                                }
                            }
                            Err(e) => {
                                log::error!("Failed to encode rejection response for client {}: {}", remote_id, e);
                            }
                        }
                        continue;
                    }
                };

                let allowed_tcp = allowed_tcp.clone();
                let allowed_udp = allowed_udp.clone();

                stream_tasks.spawn(async move {
                    let _permit = permit; // Hold permit until task completes
                    if let Err(e) = handle_multi_source_stream(
                        send_stream,
                        recv_stream,
                        allowed_tcp,
                        allowed_udp,
                    ).await {
                        log::warn!("Stream error: {}", e);
                    }
                });
            }
            error = conn.closed() => {
                log::info!("Client {} disconnected: {}", remote_id, error);
                break;
            }
        }

        // Clean up completed stream tasks
        while stream_tasks.try_join_next().is_some() {}
    }

    // Wait for remaining stream tasks
    stream_tasks.shutdown().await;
    conn.close(0u32.into(), b"done");
    log::info!("Connection from {} closed", remote_id);

    Ok(())
}

/// Handle a single stream within a multi-source connection.
/// Reads SourceRequest, validates source against allowed networks, sends SourceResponse, then forwards traffic.
/// Note: Authentication is handled at the connection level via a dedicated auth stream;
/// SourceRequest does not contain an auth_token field.
async fn handle_multi_source_stream(
    mut send_stream: iroh::endpoint::SendStream,
    mut recv_stream: iroh::endpoint::RecvStream,
    allowed_tcp: Vec<String>,
    allowed_udp: Vec<String>,
) -> Result<()> {
    // Read the source request
    let request_bytes = read_length_prefixed(&mut recv_stream)
        .await
        .context("Failed to read source request")?;
    let request = decode_source_request(&request_bytes).context("Invalid source request")?;

    log::info!("Source request: {}", request.source);

    // Determine protocol and validate
    let is_tcp = request.source.starts_with("tcp://");
    let is_udp = request.source.starts_with("udp://");

    if !is_tcp && !is_udp {
        let response = SourceResponse::rejected("Invalid protocol (must be tcp:// or udp://)");
        let encoded = encode_source_response(&response)?;
        send_stream.write_all(&encoded).await?;
        send_stream.finish()?;
        anyhow::bail!("Invalid protocol in source request: {}", request.source);
    }

    // Validate against allowed networks
    let allowed_networks = if is_tcp { &allowed_tcp } else { &allowed_udp };
    let check_result = check_source_allowed(&request.source, allowed_networks).await;

    if !check_result.allowed {
        let reason = check_result.rejection_reason(&request.source, allowed_networks);
        let response = SourceResponse::rejected(&reason);
        let encoded = encode_source_response(&response)?;
        send_stream.write_all(&encoded).await?;
        send_stream.finish()?;
        anyhow::bail!("{}", reason);
    }

    // Extract target address
    let target_addr = extract_addr_from_source(&request.source)
        .ok_or_else(|| anyhow::anyhow!("Invalid source URL format: {}", request.source))?;

    // Send acceptance response
    let response = SourceResponse::accepted();
    let encoded = encode_source_response(&response)?;
    send_stream.write_all(&encoded).await?;

    log::info!("Accepted source request, forwarding to {}", target_addr);

    // Route to appropriate handler based on protocol
    if is_tcp {
        // Resolve and connect to TCP target
        let target_addrs = resolve_all_target_addrs(&target_addr).await?;
        let tcp_stream = crate::net::try_connect_tcp(&target_addrs)
            .await
            .context("Failed to connect to target TCP service")?;

        log::info!("-> Connected to TCP target {}", target_addr);
        bridge_streams(recv_stream, send_stream, tcp_stream).await?;
        log::info!("<- TCP connection to {} closed", target_addr);
    } else {
        // UDP forwarding with multi-address fallback
        let target_addrs = Arc::new(resolve_all_target_addrs(&target_addr).await?);
        if target_addrs.is_empty() {
            anyhow::bail!("No target addresses resolved for '{}'", target_addr);
        }
        let primary_addr = target_addrs.first().copied().unwrap();

        // Bind UDP socket with appropriate address family
        let udp_socket = Arc::new(
            bind_udp_for_targets(&target_addrs)
                .await
                .context("Failed to bind UDP socket")?,
        );

        log::info!(
            "-> Forwarding UDP to {} ({} address(es) resolved)",
            primary_addr,
            target_addrs.len()
        );
        forward_stream_to_udp_server(recv_stream, send_stream, udp_socket, target_addrs).await?;
        log::info!("<- UDP forwarding to {} closed", primary_addr);
    }

    Ok(())
}

// ============================================================================
// Client
// ============================================================================

/// Run iroh multi-source client.
///
/// Authenticate with the server on a dedicated auth stream.
/// Must be called immediately after connection before opening source streams.
async fn authenticate_connection(
    conn: &iroh::endpoint::Connection,
    auth_token: &str,
) -> Result<()> {
    let (mut send_stream, mut recv_stream) = open_bi_with_retry(conn).await?;

    // Send AuthRequest
    let request = AuthRequest::new(auth_token);
    let encoded = encode_auth_request(&request)?;
    send_stream.write_all(&encoded).await?;
    send_stream.finish()?;

    // Read AuthResponse with timeout
    let response_bytes = tokio::time::timeout(AUTH_TIMEOUT, read_length_prefixed(&mut recv_stream))
        .await
        .map_err(|_| TunnelError::auth(anyhow::anyhow!("Auth response timed out")))?
        .context("Failed to read auth response")?;
    let response = decode_auth_response(&response_bytes).context("Invalid auth response")?;

    if !response.accepted {
        let reason = response.reason.unwrap_or_else(|| "Unknown".to_string());
        return Err(TunnelError::auth(anyhow::anyhow!("Authentication rejected: {}", reason)).into());
    }

    log::info!("Authenticated with server successfully");
    Ok(())
}

/// Connects to a server and requests a specific source (tcp://host:port or udp://host:port).
/// The server validates the request and either accepts or rejects it.
/// Note: relay_only disables direct P2P transport.
/// Authentication is done via dedicated auth stream immediately after connection.
pub async fn run_multi_source_client(config: MultiSourceClientConfig) -> Result<()> {
    let relay_only = config.relay_only;

    validate_relay_only(relay_only, &config.relay_urls)?;

    // Validate source format
    let is_tcp = config.source.starts_with("tcp://");
    let is_udp = config.source.starts_with("udp://");
    if !is_tcp && !is_udp {
        anyhow::bail!(
            "Source must start with tcp:// or udp:// (got: {})",
            config.source
        );
    }

    // Resolve listen addresses - for localhost, returns both IPv4 and IPv6
    // to handle macOS clients that prefer IPv6 when connecting to "localhost"
    let listen_addrs: Vec<SocketAddr> = resolve_listen_addrs(&config.target)
        .await
        .context("Invalid target address format. Use format like localhost:2222, 127.0.0.1:2222 or [::]:2222")?;

    let server_id: EndpointId = config
        .node_id
        .parse()
        .context("Invalid EndpointId format. Should be a 52-character base32 string.")?;

    log::info!("Multi-Source Tunnel - Client Mode");
    log::info!("==================================");
    log::info!("Requesting source: {}", config.source);
    log::info!("Creating iroh endpoint (ephemeral identity)...");

    // Client uses ephemeral identity - auth is done via token in protocol
    let endpoint = create_client_endpoint(
        &config.relay_urls,
        relay_only,
        config.dns_server.as_deref(),
        None,
        Some(&config.transport),
    )
    .await?;

    let alpn = build_multi_alpn(&config.alpn_token);
    let conn = connect_to_server(
        &endpoint,
        server_id,
        &config.relay_urls,
        relay_only,
        &alpn,
    )
    .await?;

    log::info!("Connected to server!");
    let _path_watcher = watch_connection_paths(&conn);

    // Authenticate immediately after connection
    authenticate_connection(&conn, &config.auth_token).await?;

    let conn = Arc::new(conn);
    let tunnel_established = Arc::new(AtomicBool::new(false));

    if is_tcp {
        run_multi_source_tcp_client(conn, config.source, &listen_addrs, tunnel_established).await?;
    } else {
        // UDP still uses single address (first one) - multi-listener for UDP is more complex
        let listen_addr = listen_addrs
            .first()
            .ok_or_else(|| anyhow::anyhow!("No listen addresses resolved for target"))?;
        run_multi_source_udp_client(conn, config.source, *listen_addr).await?;
    }

    endpoint.close().await;
    log::info!("Multi-source client stopped.");

    Ok(())
}

/// Run TCP client for multi-source mode.
/// Opens streams for each local connection and sends source requests.
///
/// Binds to multiple addresses when localhost is specified, to handle clients
/// that may connect via IPv4 (127.0.0.1) or IPv6 (::1).
async fn run_multi_source_tcp_client(
    conn: Arc<iroh::endpoint::Connection>,
    source: String,
    listen_addrs: &[SocketAddr],
    tunnel_established: Arc<AtomicBool>,
) -> Result<()> {
    use tokio::sync::mpsc;

    // Create listeners for all addresses
    let mut listeners = Vec::with_capacity(listen_addrs.len());
    for addr in listen_addrs {
        match TcpListener::bind(addr).await {
            Ok(listener) => {
                log::info!(
                    "Listening on TCP {} - configure your client to connect here",
                    addr
                );
                listeners.push(listener);
            }
            Err(e) => {
                // Log warning but continue - some addresses may fail (e.g., IPv6 disabled)
                log::warn!("Failed to bind TCP listener on {}: {}", addr, e);
            }
        }
    }

    if listeners.is_empty() {
        anyhow::bail!("Failed to bind any TCP listeners");
    }

    // Channel to receive accepted connections from all listeners
    let (tx, mut rx) = mpsc::channel::<(TcpStream, SocketAddr)>(32);

    // Spawn accept tasks for each listener
    let mut accept_tasks: JoinSet<()> = JoinSet::new();
    for listener in listeners {
        let tx = tx.clone();
        accept_tasks.spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, peer_addr)) => {
                        if tx.send((stream, peer_addr)).await.is_err() {
                            // Channel closed, stop accepting
                            break;
                        }
                    }
                    Err(e) => {
                        log::warn!("Failed to accept TCP connection: {}", e);
                    }
                }
            }
        });
    }
    drop(tx); // Drop our copy so channel closes when all accept tasks stop

    let mut connection_tasks: JoinSet<()> = JoinSet::new();

    loop {
        tokio::select! {
            accept_result = rx.recv() => {
                let Some((tcp_stream, peer_addr)) = accept_result else {
                    log::info!("All listeners closed");
                    break;
                };

                log::info!("New local connection from {}", peer_addr);

                let conn_clone = conn.clone();
                let source_clone = source.clone();
                let established = tunnel_established.clone();

                connection_tasks.spawn(async move {
                    match handle_multi_source_tcp_client_connection(
                        conn_clone,
                        tcp_stream,
                        peer_addr,
                        source_clone,
                        established,
                    ).await {
                        Ok(()) => {}
                        Err(e) => {
                            log::warn!("TCP tunnel error for {}: {}", peer_addr, e);
                        }
                    }
                });
            }
            error = conn.closed() => {
                log::warn!("QUIC connection closed: {}", error);
                accept_tasks.shutdown().await;
                connection_tasks.shutdown().await;
                return Err(TunnelError::connection_lost(
                    anyhow::anyhow!("QUIC connection closed: {}", error)
                ).into());
            }
        }

        // Clean up completed tasks
        while let Some(result) = connection_tasks.try_join_next() {
            if let Err(e) = result {
                log::error!("Connection task panicked: {}", e);
            }
        }
    }

    accept_tasks.shutdown().await;
    connection_tasks.shutdown().await;
    conn.close(0u32.into(), b"done");
    log::info!("TCP client stopped.");

    Ok(())
}

/// Handle a single TCP connection in multi-source client mode.
async fn handle_multi_source_tcp_client_connection(
    conn: Arc<iroh::endpoint::Connection>,
    tcp_stream: TcpStream,
    peer_addr: SocketAddr,
    source: String,
    tunnel_established: Arc<AtomicBool>,
) -> Result<()> {
    let (mut send_stream, mut recv_stream) = open_bi_with_retry(&conn).await?;

    // Send source request (auth already validated at connection level)
    let request = SourceRequest::new(source.clone());
    let encoded = encode_source_request(&request)?;
    send_stream.write_all(&encoded).await?;

    // Read response
    let response_bytes = tokio::time::timeout(AUTH_TIMEOUT, read_length_prefixed(&mut recv_stream))
        .await
        .context("Timed out waiting for source response")?
        .context("Failed to read source response")?;
    let response = decode_source_response(&response_bytes).context("Invalid source response")?;

    if !response.accepted {
        let reason = response.reason.unwrap_or_else(|| "Unknown".to_string());
        anyhow::bail!("Source request rejected: {}", reason);
    }

    // Print success message only on first successful stream
    if !tunnel_established.swap(true, Ordering::Relaxed) {
        log::info!("Tunnel to server established! Source: {}", source);
    }
    log::info!("-> Opened tunnel for {}", peer_addr);

    bridge_streams(recv_stream, send_stream, tcp_stream).await?;

    log::info!("<- Connection from {} closed", peer_addr);
    Ok(())
}

/// Run UDP client for multi-source mode.
/// Opens a single stream and sends source request, then forwards UDP traffic.
async fn run_multi_source_udp_client(
    conn: Arc<iroh::endpoint::Connection>,
    source: String,
    listen_addr: SocketAddr,
) -> Result<()> {
    let (mut send_stream, mut recv_stream) = open_bi_with_retry(&conn).await?;

    // Send source request (auth already validated at connection level)
    let request = SourceRequest::new(source.clone());
    let encoded = encode_source_request(&request)?;
    send_stream.write_all(&encoded).await?;

    // Read response
    let response_bytes = tokio::time::timeout(AUTH_TIMEOUT, read_length_prefixed(&mut recv_stream))
        .await
        .context("Timed out waiting for source response")?
        .context("Failed to read source response")?;
    let response = decode_source_response(&response_bytes).context("Invalid source response")?;

    if !response.accepted {
        let reason = response.reason.unwrap_or_else(|| "Unknown".to_string());
        anyhow::bail!("Source request rejected: {}", reason);
    }

    log::info!("Tunnel established! Source: {}", source);

    let udp_socket = Arc::new(
        UdpSocket::bind(listen_addr)
            .await
            .context("Failed to bind UDP socket")?,
    );
    log::info!(
        "Listening on UDP {} - configure your client to connect here",
        listen_addr
    );

    let client_addr: Arc<Mutex<Option<SocketAddr>>> = Arc::new(Mutex::new(None));

    let udp_clone = udp_socket.clone();
    let client_clone = client_addr.clone();

    tokio::select! {
        result = forward_udp_to_stream(udp_clone, send_stream, client_clone) => {
            if let Err(e) = result {
                log::warn!("UDP to stream error: {}", e);
            }
        }
        result = forward_stream_to_udp_client(recv_stream, udp_socket, client_addr) => {
            if let Err(e) = result {
                log::warn!("Stream to UDP error: {}", e);
            }
        }
        error = conn.closed() => {
            log::warn!("QUIC connection closed: {}", error);
            return Err(TunnelError::connection_lost(
                anyhow::anyhow!("QUIC connection closed: {}", error)
            ).into());
        }
    }

    conn.close(0u32.into(), b"done");
    log::info!("UDP client stopped.");

    Ok(())
}
