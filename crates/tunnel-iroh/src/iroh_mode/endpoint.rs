//! Common endpoint helpers for iroh tunnel connections.

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use iroh::{
    address_lookup::{DnsAddressLookup, MdnsAddressLookup, PkarrPublisher, PkarrResolver},
    endpoint::{Builder as EndpointBuilder, ControllerFactory, PathInfoList, QuicTransportConfig},
    Endpoint, EndpointAddr, EndpointId, RelayMap, RelayMode, RelayUrl, SecretKey, TransportAddr,
    Watcher,
};
use iroh_quinn_proto::congestion::{BbrConfig, CubicConfig, NewRenoConfig};
use log::{info, warn};
use tokio::task::JoinHandle;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tunnel_common::config::{CongestionController, TransportTuning, DEFAULT_RECEIVE_WINDOW};
use url::Url;

pub const RELAY_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Build the ALPN protocol identifier with an embedded pre-shared token.
///
/// The ALPN acts as a lightweight "port knock" — connections from clients that
/// don't know the token fail at the QUIC handshake level before any application
/// streams are opened.
pub fn build_multi_alpn(alpn_token: &str) -> Vec<u8> {
    format!("mf/2/{}", alpn_token).into_bytes()
}

/// QUIC keep-alive interval for tunnel connections.
///
/// Active connections send pings at this interval to prevent idle timeout.
/// This value matches iroh's relay ping interval (15s), which is designed to be
/// well under half common QUIC idle timeout defaults (30s is typical in many
/// implementations and protocol discussions). This codebase uses a more generous
/// [`QUIC_IDLE_TIMEOUT`] of 300s for long-running tunnels, but 15s keep-alive
/// remains appropriate for NAT traversal and prompt dead-connection detection.
///
/// For long-running tunnels, 15s is a good balance between:
/// - Keeping NAT mappings alive (most NAT timeouts are 30-120s)
/// - Not wasting bandwidth with excessive pings
/// - Detecting dead connections reasonably quickly
///
/// Reference: iroh uses 1s for endpoint default, 15s for relay pings.
pub const QUIC_KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(15);

/// QUIC idle timeout for tunnel connections.
///
/// Connections without activity (no data or keep-alive pings) for this duration
/// are considered dead and closed. With QUIC_KEEP_ALIVE_INTERVAL enabled,
/// this timeout only triggers for truly unresponsive connections.
///
/// 5 minutes is generous for tunnels where the underlying TCP/UDP connection
/// may have long idle periods between bursts of activity.
pub const QUIC_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// Create a congestion controller factory based on the selected algorithm.
fn create_congestion_controller_factory(
    controller: CongestionController,
) -> Arc<dyn ControllerFactory + Send + Sync> {
    match controller {
        CongestionController::Cubic => Arc::new(CubicConfig::default()),
        CongestionController::Bbr => Arc::new(BbrConfig::default()),
        CongestionController::NewReno => Arc::new(NewRenoConfig::default()),
    }
}

/// Load secret key from file (base64 encoded).
pub fn load_secret(path: &Path) -> Result<SecretKey> {
    if !path.exists() {
        anyhow::bail!(
            "Secret key file not found: {}\nGenerate one with: tunnel-rs generate-server-key --output {}",
            path.display(),
            path.display()
        );
    }

    let content = std::fs::read_to_string(path).context("Failed to read secret key file")?;
    load_secret_from_string(content.trim())
}

/// Load secret key from a base64-encoded string.
pub fn load_secret_from_string(base64_key: &str) -> Result<SecretKey> {
    let bytes = BASE64
        .decode(base64_key)
        .context("Invalid base64 in secret key")?;

    SecretKey::try_from(&bytes[..]).context("Invalid secret key (must be 32 bytes)")
}

/// Get public key (EndpointId) from secret key.
pub fn secret_to_endpoint_id(secret: &SecretKey) -> EndpointId {
    secret.public()
}

/// Parse relay URL strings into a RelayMode.
pub fn parse_relay_mode(relay_urls: &[String]) -> Result<RelayMode> {
    if relay_urls.is_empty() {
        Ok(RelayMode::Default)
    } else {
        let parsed_urls: Vec<RelayUrl> = relay_urls
            .iter()
            .map(|url| url.parse().context(format!("Invalid relay URL: {}", url)))
            .collect::<Result<Vec<_>>>()?;
        let relay_map = RelayMap::from_iter(parsed_urls);
        Ok(RelayMode::Custom(relay_map))
    }
}

/// Validate that relay-only mode is used correctly.
pub fn validate_relay_only(relay_only: bool, relay_urls: &[String]) -> Result<()> {
    if relay_only && relay_urls.is_empty() {
        anyhow::bail!(
            "--relay-only requires at least one --relay-url to be specified.\n\
            The default public relay is rate-limited and cannot be used for relay-only mode."
        );
    }

    Ok(())
}

/// Print relay configuration status messages.
pub fn print_relay_status(relay_urls: &[String], relay_only: bool, using_custom_relay: bool) {
    if using_custom_relay {
        if relay_urls.len() == 1 {
            info!("Using custom relay server");
        } else {
            info!(
                "Using {} custom relay servers (with failover)",
                relay_urls.len()
            );
        }
    }
    if relay_only {
        info!("Relay-only mode: all traffic will go through the relay server");
    }
}

/// Create a base endpoint builder with common configuration.
///
/// # Arguments
/// * `relay_mode` - The relay mode to use
/// * `relay_only` - If true, only use relay connections (no direct P2P).
/// * `dns_server` - Optional custom DNS server URL (e.g., "https://dns.example.com"), or "none" to disable DNS discovery
/// * `secret_key` - Optional secret key (required for publishing to custom DNS server)
/// * `transport_tuning` - Optional transport layer tuning (congestion control, buffer sizes)
pub fn create_endpoint_builder(
    relay_mode: RelayMode,
    relay_only: bool,
    dns_server: Option<&str>,
    secret_key: Option<&SecretKey>,
    transport_tuning: Option<&TransportTuning>,
) -> Result<EndpointBuilder> {
    // Configure transport with keep-alive and idle timeout.
    // See QUIC_KEEP_ALIVE_INTERVAL and QUIC_IDLE_TIMEOUT constants for rationale.
    let mut transport_config = QuicTransportConfig::builder();
    let idle_timeout = QUIC_IDLE_TIMEOUT
        .try_into()
        .context("converting QUIC_IDLE_TIMEOUT to IdleTimeout")?;
    transport_config = transport_config.max_idle_timeout(Some(idle_timeout));
    transport_config = transport_config.keep_alive_interval(QUIC_KEEP_ALIVE_INTERVAL);

    // Apply transport tuning if provided
    if let Some(tuning) = transport_tuning {
        // Set congestion controller
        let factory = create_congestion_controller_factory(tuning.congestion_controller);
        transport_config = transport_config.congestion_controller_factory(factory);
        info!("Using {:?} congestion controller", tuning.congestion_controller);

        // Set receive window (flow control) for connection + streams
        let receive_window = tuning.receive_window.unwrap_or(DEFAULT_RECEIVE_WINDOW);
        transport_config = transport_config.receive_window(receive_window.into());
        transport_config = transport_config.stream_receive_window(receive_window.into());

        // Set send window (defaults to receive window if not specified)
        let send_window = tuning.send_window.unwrap_or(receive_window);
        transport_config = transport_config.send_window(send_window.into());

        info!(
            "Iroh transport: cc={:?}, stream/receive={}KB, send={}KB",
            tuning.congestion_controller,
            receive_window / 1024,
            send_window / 1024
        );
        let recv_source = if tuning.receive_window.is_none() { "default" } else { "config" };
        let send_source = if tuning.send_window.is_none() {
            if tuning.receive_window.is_none() {
                "default"
            } else {
                "derived"
            }
        } else {
            "config"
        };
        info!(
            "Transport windows: stream/receive={}KB ({}), send={}KB ({})",
            receive_window / 1024,
            recv_source,
            send_window / 1024,
            send_source
        );
    }

    let transport_config = transport_config.build();
    let mut builder = Endpoint::empty_builder(relay_mode).transport_config(transport_config);

    if relay_only {
        builder = builder.clear_ip_transports();
    }

    if !relay_only {
        // DNS-based peer discovery (can be disabled via dns_server="none")
        match dns_server {
            Some("none") => {
                // Explicitly disabled
                info!("DNS discovery disabled (dns_server=none)");
            }
            Some(dns_url) => {
                // Custom DNS server with publishing and resolving via HTTP (pkarr)
                let pkarr_url: Url = dns_url.parse().context("Invalid DNS server URL")?;
                if let Some(secret) = secret_key {
                    info!("Using custom DNS server: {}", dns_url);
                    builder = builder
                        .address_lookup(PkarrPublisher::builder(pkarr_url.clone()).build(secret.clone()))
                        .address_lookup(PkarrResolver::builder(pkarr_url));
                } else {
                    // Custom DNS server, resolve only via HTTP (no secret = can't publish)
                    info!("Using custom DNS server (resolve only): {}", dns_url);
                    builder = builder.address_lookup(PkarrResolver::builder(pkarr_url));
                }
            }
            None => {
                // Default n0 DNS
                builder = builder
                    .address_lookup(PkarrPublisher::n0_dns())
                    .address_lookup(DnsAddressLookup::n0_dns());
            }
        }
        // mDNS always enabled for local network discovery
        builder = builder.address_lookup(MdnsAddressLookup::builder());
    }

    Ok(builder)
}

/// Create a server endpoint with optional persistent identity.
pub async fn create_server_endpoint(
    relay_urls: &[String],
    relay_only: bool,
    secret: Option<SecretKey>,
    dns_server: Option<&str>,
    alpn: &[u8],
    transport_tuning: Option<&TransportTuning>,
) -> Result<Endpoint> {
    let relay_mode = parse_relay_mode(relay_urls)?;
    let using_custom_relay = !matches!(relay_mode, RelayMode::Default);
    print_relay_status(relay_urls, relay_only, using_custom_relay);

    let mut builder =
        create_endpoint_builder(relay_mode, relay_only, dns_server, secret.as_ref(), transport_tuning)?
            .alpns(vec![alpn.to_vec()]);

    if let Some(secret) = secret {
        builder = builder.secret_key(secret);
    }

    let endpoint = builder
        .bind()
        .await
        .context("Failed to create iroh endpoint")?;

    // Wait for endpoint to come online with timeout
    info!(
        "Waiting for endpoint to come online (timeout: {}s)...",
        RELAY_CONNECT_TIMEOUT.as_secs()
    );
    match tokio::time::timeout(RELAY_CONNECT_TIMEOUT, endpoint.online()).await {
        Ok(()) => {}
        Err(_) => anyhow::bail!(
            "Endpoint failed to come online after {}s - check relay server connectivity",
            RELAY_CONNECT_TIMEOUT.as_secs()
        ),
    }

    Ok(endpoint)
}

/// Create a client endpoint.
/// If a secret key is provided, the client will use a persistent identity for authentication.
pub async fn create_client_endpoint(
    relay_urls: &[String],
    relay_only: bool,
    dns_server: Option<&str>,
    secret_key: Option<&SecretKey>,
    transport_tuning: Option<&TransportTuning>,
) -> Result<Endpoint> {
    let relay_mode = parse_relay_mode(relay_urls)?;
    let using_custom_relay = !matches!(relay_mode, RelayMode::Default);
    print_relay_status(relay_urls, relay_only, using_custom_relay);

    let mut builder = create_endpoint_builder(relay_mode, relay_only, dns_server, secret_key, transport_tuning)?;

    // Set the secret key for persistent identity (used for authentication)
    if let Some(secret) = secret_key {
        builder = builder.secret_key(secret.clone());
    }

    let endpoint = builder
        .bind()
        .await
        .context("Failed to create iroh endpoint")?;

    // Wait for endpoint to come online with timeout
    info!(
        "Waiting for endpoint to come online (timeout: {}s)...",
        RELAY_CONNECT_TIMEOUT.as_secs()
    );
    match tokio::time::timeout(RELAY_CONNECT_TIMEOUT, endpoint.online()).await {
        Ok(()) => {}
        Err(_) => anyhow::bail!(
            "Endpoint failed to come online after {}s - check relay server connectivity",
            RELAY_CONNECT_TIMEOUT.as_secs()
        ),
    }

    Ok(endpoint)
}

/// Connect to a server endpoint with relay failover support.
pub async fn connect_to_server(
    endpoint: &Endpoint,
    server_id: EndpointId,
    relay_urls: &[String],
    relay_only: bool,
    alpn: &[u8],
) -> Result<iroh::endpoint::Connection> {
    info!("Connecting to server {}...", server_id);

    if relay_only {
        // Try each relay URL until one works
        let mut last_error = None;
        for relay_url_str in relay_urls {
            let relay_url: RelayUrl = relay_url_str.parse().context("Invalid relay URL")?;
            let endpoint_addr = EndpointAddr::new(server_id).with_relay_url(relay_url.clone());
            info!(
                "Trying relay: {} (timeout: {}s)",
                relay_url,
                RELAY_CONNECT_TIMEOUT.as_secs()
            );

            match tokio::time::timeout(RELAY_CONNECT_TIMEOUT, endpoint.connect(endpoint_addr, alpn))
                .await
            {
                Ok(Ok(conn)) => {
                    info!("Connected via relay: {}", relay_url);
                    return Ok(conn);
                }
                Ok(Err(e)) => {
                    warn!("Failed to connect via {}: {}", relay_url, e);
                    last_error = Some(e.to_string());
                }
                Err(_) => {
                    warn!("Connection to {} timed out", relay_url);
                    last_error = Some(format!("Connection to {} timed out", relay_url));
                }
            }
        }
        anyhow::bail!(
            "Failed to connect via any relay: {}",
            last_error.unwrap_or_else(|| "No relay URLs provided".to_string())
        )
    } else {
        // Include relay URLs in EndpointAddr if available, allowing iroh to use
        // the relay for initial connection when DNS discovery is disabled.
        // Iroh will still attempt hole punching for direct P2P connections.
        let endpoint_addr = if !relay_urls.is_empty() {
            let mut addr = EndpointAddr::new(server_id);
            for relay_url_str in relay_urls {
                let relay_url: RelayUrl = relay_url_str.parse().context("Invalid relay URL")?;
                addr = addr.with_relay_url(relay_url);
            }
            info!(
                "Connecting with {} relay hint(s) (timeout: {}s)...",
                relay_urls.len(),
                RELAY_CONNECT_TIMEOUT.as_secs()
            );
            addr
        } else {
            info!(
                "Connecting (timeout: {}s)...",
                RELAY_CONNECT_TIMEOUT.as_secs()
            );
            EndpointAddr::new(server_id)
        };
        match tokio::time::timeout(RELAY_CONNECT_TIMEOUT, endpoint.connect(endpoint_addr, alpn))
            .await
        {
            Ok(Ok(conn)) => Ok(conn),
            Ok(Err(e)) => Err(e).context("Failed to connect to server"),
            Err(_) => anyhow::bail!(
                "Connection timed out after {}s",
                RELAY_CONNECT_TIMEOUT.as_secs()
            ),
        }
    }
}

/// Format connection path info for display, showing selected paths with RTT.
fn format_paths(paths: &PathInfoList) -> String {
    if paths.is_empty() {
        return "establishing...".to_string();
    }
    let parts: Vec<String> = paths
        .iter()
        .filter(|p| p.is_selected())
        .map(|path| {
            let rtt = path.rtt();
            match path.remote_addr() {
                TransportAddr::Ip(addr) => format!("Direct {} (rtt {:.0?})", addr, rtt),
                TransportAddr::Relay(url) => format!("Relay {} (rtt {:.0?})", url, rtt),
                other => format!("{:?} (rtt {:.0?})", other, rtt),
            }
        })
        .collect();
    if parts.is_empty() {
        "no selected path".to_string()
    } else {
        parts.join(", ")
    }
}

/// RAII guard that aborts the background path watcher task on drop.
pub struct PathWatcherGuard(JoinHandle<()>);

impl Drop for PathWatcherGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Log the current connection paths and spawn a background task that
/// logs updates whenever the active path changes (e.g., relay -> direct).
///
/// The returned [`PathWatcherGuard`] aborts the background task when dropped.
/// Callers must keep the guard alive for the duration of the connection.
pub fn watch_connection_paths(conn: &iroh::endpoint::Connection) -> PathWatcherGuard {
    let mut watcher = conn.paths();

    // Log initial snapshot
    let initial = watcher.get();
    info!("Connection: {}", format_paths(&initial));

    // Spawn background task that logs on changes
    let mut last = initial;
    PathWatcherGuard(tokio::spawn(async move {
        while let Ok(paths) = watcher.updated().await {
            if paths != last {
                info!("Connection: {}", format_paths(&paths));
                last = paths;
            }
        }
    }))
}
