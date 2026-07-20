//! Common endpoint helpers for iroh tunnel connections.

use anyhow::{Context, Result};
use crate::error::TunnelError;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use futures::StreamExt;
use iroh::{
    address_lookup::{DnsAddressLookup, PkarrPublisher, PkarrResolver},
    endpoint::{
        presets, AckFrequencyConfig, Builder as EndpointBuilder, ControllerFactory, PathList,
        QuicTransportConfig,
    },
    Endpoint, EndpointAddr, EndpointId, RelayMap, RelayMode, RelayUrl, SecretKey, TransportAddr,
};
use iroh_mdns_address_lookup::MdnsAddressLookup;
use noq_proto::congestion::{Bbr3Config, CubicConfig, NewRenoConfig};
use log::{info, warn};
use tokio::task::JoinHandle;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use url::Url;
use crate::config::{
    CongestionController, TransportTuning, DEFAULT_SEND_WINDOW, DEFAULT_STREAM_RECEIVE_WINDOW,
};

pub const RELAY_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Fixed ALPN protocol identifier for tunnel connections.
///
/// Both server and client must agree on this exact value for the QUIC handshake
/// to succeed. Access control is enforced separately via auth tokens.
pub const TUNNEL_ALPN: &[u8] = b"mf/4";

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
        CongestionController::Bbr => Arc::new(Bbr3Config::default()),
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
/// Internet address discovery is configured independently from the relay map.
///
/// # Arguments
/// * `relay_mode` - The relay mode to use
/// * `relay_only` - If true, only use relay connections (no direct P2P).
/// * `discovery` - Optional custom Pkarr server URL, or "none" to disable internet discovery.
/// * `secret_key` - When present (a persistent identity), the endpoint also
///   publishes itself to the selected discovery server; an ephemeral endpoint
///   (no secret) only resolves and never advertises itself.
/// * `transport_tuning` - Optional transport layer tuning (congestion control, buffer sizes)
pub fn create_endpoint_builder(
    relay_mode: RelayMode,
    relay_only: bool,
    discovery: Option<&str>,
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
    transport_config = transport_config.send_fairness(send_fairness_enabled());

    // Apply transport tuning if provided
    if let Some(tuning) = transport_tuning {
        // Set congestion controller
        let factory = create_congestion_controller_factory(tuning.congestion_controller);
        transport_config = transport_config.congestion_controller_factory(factory);

        // Configure the ACK_FREQUENCY extension only when explicitly requested.
        // This asks the peer to delay ACKs of the data *we* send, so a large
        // threshold starves our own sender-side congestion control of feedback.
        // Left unset by default (iroh/quinn default cadence).
        let ack_threshold_source = if let Some(threshold) = tuning.ack_eliciting_threshold {
            let mut ack_frequency = AckFrequencyConfig::default();
            ack_frequency.ack_eliciting_threshold(threshold.into());
            transport_config = transport_config.ack_frequency_config(Some(ack_frequency));
            threshold.to_string()
        } else {
            "default".to_string()
        };

        // Set the per-stream receive window. Keep iroh's connection-level receive
        // window default, which is effectively unlimited.
        let stream_receive_window = tuning
            .receive_window
            .unwrap_or(DEFAULT_STREAM_RECEIVE_WINDOW);
        transport_config = transport_config.stream_receive_window(stream_receive_window.into());

        // Set the local send window for bulk transfers.
        let send_window = match tuning.send_window {
            Some(send_window) => send_window,
            None if tuning.receive_window.is_none() => DEFAULT_SEND_WINDOW,
            None => stream_receive_window
                .saturating_mul(2)
                .min(DEFAULT_SEND_WINDOW),
        };
        transport_config = transport_config.send_window(send_window.into());

        let recv_source = if tuning.receive_window.is_none() { "default" } else { "config" };
        let send_source = if tuning.send_window.is_none() {
            if tuning.receive_window.is_none() { "default" } else { "derived" }
        } else {
            "config"
        };
        info!(
            "Transport: cc={:?}, stream_receive={}KB ({}), send={}KB ({}), connection_receive=iroh-default, ack_eliciting_threshold={}",
            tuning.congestion_controller,
            stream_receive_window / 1024,
            recv_source,
            send_window / 1024,
            send_source,
            ack_threshold_source
        );
    }

    let transport_config = transport_config.build();
    // iroh 1.0 requires the crypto provider to be set explicitly on the builder
    // when starting from the `Empty` preset — the `tls-ring` feature only makes
    // the ring backend available, it does not wire it in, and rustls' global
    // `install_default()` is not consulted.
    let crypto_provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut builder = Endpoint::builder(presets::Empty)
        .relay_mode(relay_mode)
        .transport_config(transport_config)
        .crypto_provider(crypto_provider);

    if relay_only {
        builder = builder.clear_ip_transports();
    }

    if !relay_only {
        match discovery {
            Some("none") => {
                info!("Internet discovery disabled (discovery=none)");
            }
            Some(discovery_url) => {
                let pkarr_url: Url = discovery_url
                    .parse()
                    .context("Invalid discovery server URL")?;
                if secret_key.is_some() {
                    info!("Using custom discovery server: {}", discovery_url);
                    builder = builder
                        .address_lookup(PkarrPublisher::builder(pkarr_url.clone()))
                        .address_lookup(PkarrResolver::builder(pkarr_url));
                } else {
                    info!(
                        "Using custom discovery server (resolve only): {}",
                        discovery_url
                    );
                    builder = builder.address_lookup(PkarrResolver::builder(pkarr_url));
                }
            }
            None => {
                if secret_key.is_some() {
                    builder = builder.address_lookup(PkarrPublisher::n0_dns());
                }
                builder = builder.address_lookup(DnsAddressLookup::n0_dns());
            }
        }
        // mDNS always enabled for local network discovery
        builder = builder.address_lookup(MdnsAddressLookup::builder());
    }

    Ok(builder)
}

/// QUIC send fairness across streams.
///
/// EXPERIMENTAL (tuning2): `send_fairness(false)` lets one stream drain before
/// servicing others (good for bulk single-stream, but burstier). Overridable
/// via `TUNNEL_SEND_FAIRNESS` (`1`/`true`) to restore quinn's default fair
/// scheduling for bisection. Defaults to `false` (tuning behavior).
fn send_fairness_enabled() -> bool {
    use std::sync::OnceLock;
    static FAIRNESS: OnceLock<bool> = OnceLock::new();
    *FAIRNESS.get_or_init(|| {
        let enabled = std::env::var("TUNNEL_SEND_FAIRNESS")
            .ok()
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        info!("QUIC send_fairness = {}", enabled);
        enabled
    })
}

/// Wait for an endpoint to come online, with a timeout.
async fn wait_for_endpoint_online(endpoint: &Endpoint) -> Result<()> {
    info!(
        "Waiting for endpoint to come online (timeout: {}s)...",
        RELAY_CONNECT_TIMEOUT.as_secs()
    );
    match tokio::time::timeout(RELAY_CONNECT_TIMEOUT, endpoint.online()).await {
        Ok(()) => Ok(()),
        Err(_) => Err(TunnelError::connection(anyhow::anyhow!(
            "Endpoint failed to come online after {}s - check relay server connectivity",
            RELAY_CONNECT_TIMEOUT.as_secs()
        )).into()),
    }
}

/// Create a server endpoint with optional persistent identity.
pub async fn create_server_endpoint(
    relay_urls: &[String],
    relay_only: bool,
    secret: Option<SecretKey>,
    discovery: Option<&str>,
    alpn: &[u8],
    transport_tuning: Option<&TransportTuning>,
) -> Result<Endpoint> {
    let relay_mode = parse_relay_mode(relay_urls)?;
    let using_custom_relay = !matches!(relay_mode, RelayMode::Default);
    print_relay_status(relay_urls, relay_only, using_custom_relay);

    let mut builder =
        create_endpoint_builder(relay_mode, relay_only, discovery, secret.as_ref(), transport_tuning)?
            .alpns(vec![alpn.to_vec()]);

    if let Some(secret) = secret {
        builder = builder.secret_key(secret);
    }

    let endpoint = builder
        .bind()
        .await
        .context("Failed to create iroh endpoint")?;

    wait_for_endpoint_online(&endpoint).await?;

    Ok(endpoint)
}

/// Create a client endpoint.
/// If a secret key is provided, the client will use a persistent identity for authentication.
pub async fn create_client_endpoint(
    relay_urls: &[String],
    relay_only: bool,
    discovery: Option<&str>,
    secret_key: Option<&SecretKey>,
    transport_tuning: Option<&TransportTuning>,
) -> Result<Endpoint> {
    let relay_mode = parse_relay_mode(relay_urls)?;
    let using_custom_relay = !matches!(relay_mode, RelayMode::Default);
    print_relay_status(relay_urls, relay_only, using_custom_relay);

    let mut builder =
        create_endpoint_builder(relay_mode, relay_only, discovery, secret_key, transport_tuning)?;

    // Set the secret key for persistent identity (used for authentication)
    if let Some(secret) = secret_key {
        builder = builder.secret_key(secret.clone());
    }

    let endpoint = builder
        .bind()
        .await
        .context("Failed to create iroh endpoint")?;

    wait_for_endpoint_online(&endpoint).await?;

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
        Err(TunnelError::connection(anyhow::anyhow!(
            "Failed to connect via any relay: {}",
            last_error.unwrap_or_else(|| "No relay URLs provided".to_string())
        )).into())
    } else {
        // Include relay URLs in EndpointAddr if available, allowing iroh to use
        // the relay for initial connection when iroh discovery is disabled.
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
            Ok(Err(e)) => Err(TunnelError::connection(
                anyhow::Error::from(e).context("Failed to connect to server"),
            ).into()),
            Err(_) => Err(TunnelError::connection(anyhow::anyhow!(
                "Connection timed out after {}s",
                RELAY_CONNECT_TIMEOUT.as_secs()
            )).into()),
        }
    }
}

/// Format connection path info for display, showing selected paths with RTT.
fn format_paths(paths: &PathList<'_>) -> String {
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

/// Key identifying the selected-path topology, excluding the volatile RTT,
/// so we only log when the path actually changes.
fn paths_key(paths: &PathList<'_>) -> (bool, Vec<String>) {
    let selected = paths
        .iter()
        .filter(|p| p.is_selected())
        .map(|p| format!("{:?}", p.remote_addr()))
        .collect();
    (paths.is_empty(), selected)
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
    let conn = conn.clone();
    PathWatcherGuard(tokio::spawn(async move {
        // The stream yields the current snapshot on the first poll, then a
        // fresh snapshot whenever the open or selected paths change; it ends
        // when the connection closes.
        let mut stream = conn.paths_stream();
        let mut last_key = None;
        while let Some(paths) = stream.next().await {
            let key = paths_key(&paths);
            if last_key.as_ref() != Some(&key) {
                info!("Connection: {}", format_paths(&paths));
                last_key = Some(key);
            }
        }
    }))
}
