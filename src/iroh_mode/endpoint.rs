//! tunnel-rs's endpoints: what this program layers onto the shared
//! [`flexaccess_iroh::endpoint`] builder — the tunnel ALPN, its QUIC transport
//! tuning, the user-facing relay-only mode with its sequential relay dial,
//! and the server's secret-key file. Relay configuration, the per-relay
//! startup probe, and the creation-vs-rebuild policy come from the shared
//! crate.

use anyhow::{Context, Result};
use crate::error::TunnelError;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use flexaccess_iroh::endpoint::{
    create_endpoint, endpoint_builder, rebuild_endpoint, EndpointOptions,
};
use futures::StreamExt;
use iroh::{
    endpoint::{AckFrequencyConfig, Builder as EndpointBuilder, PathList, QuicTransportConfig},
    Endpoint, EndpointAddr, EndpointId, SecretKey, TransportAddr,
};
use noq_proto::congestion::{Bbr3Config, ControllerFactory, CubicConfig, NewRenoConfig};
use log::{info, warn};
use tokio::task::JoinHandle;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use crate::config::{
    CongestionController, TransportTuning, DEFAULT_SEND_WINDOW, DEFAULT_STREAM_RECEIVE_WINDOW,
};

pub use flexaccess_iroh::endpoint::EndpointFactory;
pub use flexaccess_iroh::relay::{RelayConfig, RELAY_CONNECT_TIMEOUT};

/// Fixed ALPN protocol identifier for tunnel connections.
///
/// Both server and client must agree on this exact value for the QUIC handshake
/// to succeed. Access control is enforced separately via public-key authentication.
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
///
/// Accepts either the bare base64 key or a whole generated key file, whose
/// leading `#` headers carry the creation time and EndpointId. Blank lines are
/// ignored, so the same value works from a file, an inline `secret`, or
/// `TUNNEL_RS_SECRET`.
pub fn load_secret_from_string(base64_key: &str) -> Result<SecretKey> {
    let base64_key = base64_key
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .context("No secret key found (expected a base64 key line)")?;

    let bytes = BASE64
        .decode(base64_key)
        .context("Invalid base64 in secret key")?;

    SecretKey::try_from(&bytes[..]).context("Invalid secret key (must be 32 bytes)")
}

/// Get public key (EndpointId) from secret key.
pub fn secret_to_endpoint_id(secret: &SecretKey) -> EndpointId {
    secret.public()
}

/// Validate that relay-only mode is used correctly.
///
/// Relay-only is meaningless against the rate-limited default relays, so it
/// requires a custom relay set.
pub fn validate_relay_only(relay_only: bool, relay_config: &RelayConfig) -> Result<()> {
    if relay_only && !relay_config.is_custom() {
        anyhow::bail!(
            "--relay-only requires at least one --relay-url to be specified.\n\
            The default public relay is rate-limited and cannot be used for relay-only mode."
        );
    }

    Ok(())
}

/// Log relay-only mode; the relay set itself is logged by the shared
/// creation path.
fn log_relay_only(relay_only: bool) {
    if relay_only {
        info!("Relay-only mode: all traffic will go through the relay server");
    }
}

/// Build the QUIC transport config shared by every endpoint this process binds.
fn build_quic_transport_config(tuning: &TransportTuning) -> Result<QuicTransportConfig> {
    // Configure transport with keep-alive and idle timeout.
    // See QUIC_KEEP_ALIVE_INTERVAL and QUIC_IDLE_TIMEOUT constants for rationale.
    let mut transport_config = QuicTransportConfig::builder();
    let idle_timeout = QUIC_IDLE_TIMEOUT
        .try_into()
        .context("converting QUIC_IDLE_TIMEOUT to IdleTimeout")?;
    transport_config = transport_config.max_idle_timeout(Some(idle_timeout));
    transport_config = transport_config.keep_alive_interval(QUIC_KEEP_ALIVE_INTERVAL);
    transport_config = transport_config.send_fairness(send_fairness_enabled());

    {
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

    Ok(transport_config.build())
}

/// The shared base builder with tunnel-rs's QUIC transport tuning.
///
/// Internet discovery follows the relay mode and `publish_address` (a
/// persistent identity publishes to n0 pkarr on the default relays; an
/// ephemeral client never advertises itself); mDNS is on unless `relay_only`,
/// which drops the direct IP transports and every address lookup so the
/// endpoint is reachable *only* over the configured relays — what makes
/// tunnel-rs the reference for exercising a self-hosted relay end to end.
fn base_builder(
    relay_config: &RelayConfig,
    relay_only: bool,
    publish_address: bool,
    tuning: &TransportTuning,
) -> Result<EndpointBuilder> {
    Ok(endpoint_builder(
        relay_config,
        EndpointOptions {
            transport_config: build_quic_transport_config(tuning)?,
            publish_address,
            relay_only,
        },
    ))
}

/// A server endpoint builder: persistent identity (published on the default
/// relays) and the tunnel ALPN. Binding policy is the caller's —
/// [`create_server_endpoint`] and [`server_rebuild_factory`] each layer their
/// own.
fn server_builder(
    relay_config: &RelayConfig,
    relay_only: bool,
    secret: SecretKey,
    tuning: &TransportTuning,
) -> Result<EndpointBuilder> {
    Ok(base_builder(relay_config, relay_only, true, tuning)?
        .alpns(vec![TUNNEL_ALPN.to_vec()])
        .secret_key(secret))
}

/// Create the server endpoint with its persistent identity.
///
/// With the default relays internet discovery is on, so the server publishes
/// its current home relay and clients resolve it by endpoint ID. With custom
/// relays discovery is off, so clients reach the server through the relay
/// hints they attach to its `EndpointAddr` (see [`connect_to_server`]). Strict
/// first-creation policy: every custom relay is probed and the endpoint must
/// come online, both reported as [`TunnelError::connection`].
pub async fn create_server_endpoint(
    relay_config: &RelayConfig,
    relay_only: bool,
    secret: SecretKey,
    tuning: &TransportTuning,
) -> Result<Endpoint> {
    log_relay_only(relay_only);
    create_endpoint(relay_config, server_builder(relay_config, relay_only, secret, tuning)?)
        .await
        .map_err(|e| TunnelError::connection(e).into())
}

/// The rebuild recipe for the server endpoint, used when the relay watchdog
/// gives up on the current one. Same identity as the original, so the
/// server's EndpointId — what clients dial — never changes. Tolerant rebuild
/// policy (see [`rebuild_endpoint`]): no relay probe, and the online wait may
/// fail — the watchdog trips again if the relays stay unreachable, with a
/// lengthening deadline so a dead relay does not churn the endpoint every few
/// minutes (see the serve loop in `multi_source`).
pub fn server_rebuild_factory(
    relay_config: RelayConfig,
    relay_only: bool,
    secret: SecretKey,
    tuning: TransportTuning,
) -> EndpointFactory {
    Arc::new(move || {
        let relay_config = relay_config.clone();
        let secret = secret.clone();
        let tuning = tuning.clone();
        Box::pin(async move {
            rebuild_endpoint(server_builder(&relay_config, relay_only, secret, &tuning)?).await
        })
    })
}

/// Create a client endpoint: ephemeral identity, never published (the client
/// only dials out; its credential is the application auth key). Strict
/// first-creation policy, reported as [`TunnelError::connection`].
pub async fn create_client_endpoint(
    relay_config: &RelayConfig,
    relay_only: bool,
    tuning: &TransportTuning,
) -> Result<Endpoint> {
    log_relay_only(relay_only);
    create_endpoint(relay_config, base_builder(relay_config, relay_only, false, tuning)?)
        .await
        .map_err(|e| TunnelError::connection(e).into())
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

/// Connect to a server endpoint with relay failover support.
///
/// The relay hints' role depends on the relay mode. With the **default** relays,
/// internet discovery is on and the server's home relay is resolvable from its
/// published record by endpoint ID (there are no custom hints to add). With
/// **custom** relays, internet discovery is disabled, so these hints are how the
/// client reaches the server at all: iroh sends QUIC Initials to every
/// configured relay and the handshake succeeds via whichever one the server is
/// homed on, while it still attempts hole punching for direct P2P.
///
/// Under `relay_only` there is no direct path to fall back on, so the relays are
/// tried one at a time instead — a dead relay fails fast and the next is dialed.
pub async fn connect_to_server(
    endpoint: &Endpoint,
    server_id: EndpointId,
    relay_config: &RelayConfig,
    relay_only: bool,
    alpn: &[u8],
) -> Result<iroh::endpoint::Connection> {
    info!("Connecting to server {}...", server_id);
    let relay_urls = relay_config.custom_urls();

    if relay_only {
        // Try each relay URL until one works
        let mut last_error = None;
        for relay_url in relay_urls {
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
            for relay_url in relay_urls {
                addr = addr.with_relay_url(relay_url.clone());
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

#[cfg(test)]
mod tests {
    use super::*;

    const RELAY: &str = "https://relay.example.com./";

    #[test]
    fn secret_key_headers_and_blank_lines_are_skipped() {
        let secret = SecretKey::generate();
        let base64_key = BASE64.encode(secret.to_bytes());
        let key_file = format!(
            "# tunnel-rs server secret key (iroh endpoint identity)\n\
             # Created: 2026-08-11T00:00:00Z\n# EndpointId: {}\n\n{}\n",
            secret.public(),
            base64_key
        );

        assert_eq!(
            load_secret_from_string(&key_file).unwrap().to_bytes(),
            secret.to_bytes()
        );
        assert_eq!(
            load_secret_from_string(&base64_key).unwrap().to_bytes(),
            secret.to_bytes()
        );
    }

    #[test]
    fn secret_key_with_only_headers_is_rejected() {
        let error = load_secret_from_string("# Created: 2026-08-11T00:00:00Z\n")
            .expect_err("a file without a key line must be rejected");
        assert!(
            error.to_string().contains("No secret key found"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn relay_only_requires_custom_relays() {
        let err = validate_relay_only(true, &RelayConfig::Default)
            .expect_err("relay-only without custom relays must be rejected");
        assert!(
            err.to_string().contains("--relay-only requires"),
            "unexpected error: {err}"
        );
        let custom = RelayConfig::from_urls(&[RELAY.to_string()]).unwrap();
        assert!(validate_relay_only(true, &custom).is_ok());
        assert!(validate_relay_only(false, &RelayConfig::Default).is_ok());
    }
}
