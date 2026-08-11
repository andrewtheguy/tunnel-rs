//! Common endpoint helpers for iroh tunnel connections.

use anyhow::{Context, Result};
use crate::error::TunnelError;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use futures::future::join_all;
use futures::StreamExt;
use iroh::{
    address_lookup::{DnsAddressLookup, PkarrPublisher},
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
use crate::config::{
    CongestionController, TransportTuning, DEFAULT_SEND_WINDOW, DEFAULT_STREAM_RECEIVE_WINDOW,
};

pub const RELAY_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

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

/// Relay configuration, resolved once from the raw config strings.
///
/// This is the single source of the default-vs-custom distinction. It selects
/// both which relay map iroh uses **and** whether iroh *internet* discovery is
/// enabled: [`Default`](Self::Default) uses the n0 relays with the n0 lookup
/// stack (pkarr publishing + DNS resolution of the peer's home relay — see
/// <https://docs.iroh.computer/concepts/address-lookup>), while
/// [`Custom`](Self::Custom) uses the configured relays with n0 internet discovery
/// disabled (clients use relay hints instead). mDNS local-network discovery is
/// independent of this and stays on in both modes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RelayConfig {
    /// iroh's default relay map, with n0 address lookup.
    #[default]
    Default,
    /// Custom relay set (parsed, sorted, deduped). Never empty.
    ///
    /// `auth_token`, when set, is sent to every custom relay as an
    /// `Authorization: Bearer <token>` header on the WebSocket upgrade (see
    /// [`Self::relay_mode`]). It is only ever carried by custom relays — the
    /// default relays never receive a token (see [`Self::from_urls_with_token`]).
    Custom {
        urls: Vec<RelayUrl>,
        auth_token: Option<String>,
    },
}

impl RelayConfig {
    /// Parse raw config strings with no relay auth token.
    ///
    /// Thin wrapper over [`Self::from_urls_with_token`]; see there for behavior.
    // Kept for parity with the shared relay design (ezvpn/flextunnel expose the
    // same pair); every tunnel-rs call site currently threads a token through.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn from_urls(urls: &[String]) -> Result<Self> {
        Self::from_urls_with_token(urls, None)
    }

    /// Parse raw config strings and attach an optional shared relay auth token.
    ///
    /// Empty input selects the default relays. Parsing fails on the first
    /// malformed URL, so config typos surface at resolve time instead of at each
    /// use site.
    ///
    /// The token is normalized (blank/whitespace-only becomes `None`) and is
    /// **strictly gated to custom relays**: a non-empty token with no custom
    /// relay URLs is a hard error, since the default iroh relays never take a
    /// token. This surfaces the misconfiguration before the endpoint starts.
    pub fn from_urls_with_token(urls: &[String], auth_token: Option<String>) -> Result<Self> {
        let auth_token = auth_token.and_then(|token| {
            let token = token.trim();
            (!token.is_empty()).then(|| token.to_string())
        });
        if urls.is_empty() {
            if auth_token.is_some() {
                anyhow::bail!(
                    "relay_auth_token requires custom relay_urls; it is not used with the default iroh relays"
                );
            }
            return Ok(Self::Default);
        }
        let mut parsed = urls
            .iter()
            .map(|url| {
                url.parse::<RelayUrl>()
                    .with_context(|| format!("Invalid relay URL: {url}"))
            })
            .collect::<Result<Vec<_>>>()?;
        parsed.sort();
        parsed.dedup();
        Ok(Self::Custom {
            urls: parsed,
            auth_token,
        })
    }

    /// The custom relay URLs; empty for [`RelayConfig::Default`].
    pub fn custom_urls(&self) -> &[RelayUrl] {
        match self {
            Self::Default => &[],
            Self::Custom { urls, .. } => urls,
        }
    }

    /// The shared relay auth token, if configured (custom relays only).
    pub fn relay_auth_token(&self) -> Option<&str> {
        match self {
            Self::Default => None,
            Self::Custom { auth_token, .. } => auth_token.as_deref(),
        }
    }

    pub fn is_custom(&self) -> bool {
        matches!(self, Self::Custom { .. })
    }

    /// The corresponding iroh [`RelayMode`].
    ///
    /// For custom relays, an `auth_token` (when set) is applied to every relay in
    /// the map via [`RelayMap::with_auth_token`], which iroh sends as an
    /// `Authorization: Bearer <token>` header on the relay WebSocket upgrade.
    pub fn relay_mode(&self) -> RelayMode {
        match self {
            Self::Default => RelayMode::Default,
            Self::Custom { urls, auth_token } => {
                let map = RelayMap::from_iter(urls.iter().cloned());
                let map = match auth_token {
                    Some(token) => map.with_auth_token(token.clone()),
                    None => map,
                };
                RelayMode::Custom(map)
            }
        }
    }
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

/// Print relay configuration status messages.
pub fn print_relay_status(relay_config: &RelayConfig, relay_only: bool) {
    // Only ever reports *whether* a token is set — never the token itself.
    let auth = if relay_config.relay_auth_token().is_some() {
        " (authenticated)"
    } else {
        ""
    };
    match relay_config.custom_urls().len() {
        0 => {}
        1 => info!("Using custom relay server{}", auth),
        n => info!("Using {} custom relay servers with failover{}", n, auth),
    }
    if relay_only {
        info!("Relay-only mode: all traffic will go through the relay server");
    }
}

/// Build the QUIC transport config shared by every endpoint this process binds.
///
/// `transport_tuning` is optional: without it the endpoint keeps iroh's
/// defaults for congestion control and windows (used by the relay probe
/// endpoints, which move no application data).
fn build_quic_transport_config(
    transport_tuning: Option<&TransportTuning>,
) -> Result<QuicTransportConfig> {
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

    Ok(transport_config.build())
}

/// Create a base endpoint builder with common configuration.
///
/// iroh *internet* discovery (n0 pkarr publishing + DNS-based lookup of
/// `_iroh.<endpoint-id>.dns.iroh.link`, see
/// <https://docs.iroh.computer/concepts/address-lookup>) is **not** configurable;
/// it follows the relay mode:
///
/// - [`RelayConfig::Default`]: the n0 lookup stack is enabled — DNS resolution is
///   always on, and pkarr publishing is added only when a persistent identity
///   (`secret_key`) is present, so an ephemeral client resolves peers but never
///   advertises itself.
/// - [`RelayConfig::Custom`]: n0 internet discovery is disabled — nothing is
///   published to or resolved from n0's public infrastructure. The client reaches
///   the server through the configured relay hints it attaches to the server's
///   `EndpointAddr` (see [`connect_to_server`]): iroh sends QUIC Initials to
///   every configured relay, so the handshake succeeds via whichever relay the
///   server is homed on.
///
/// `relay_only` drops the direct IP transports and every address lookup
/// (including mDNS), so the endpoint is reachable *only* over the configured
/// relays. This is what makes tunnel-rs usable as a relay-only deployment and as
/// the reference for exercising a self-hosted relay end to end.
///
/// # Arguments
/// * `relay_config` - The resolved relay configuration
/// * `relay_only` - If true, only use relay connections (no direct P2P).
/// * `secret_key` - When present (a persistent identity), the endpoint also
///   publishes itself to the public discovery service (default relays only);
///   an ephemeral endpoint (no secret) only resolves and never advertises itself.
/// * `transport_tuning` - Optional transport layer tuning (congestion control, buffer sizes)
pub fn create_endpoint_builder(
    relay_config: &RelayConfig,
    relay_only: bool,
    secret_key: Option<&SecretKey>,
    transport_tuning: Option<&TransportTuning>,
) -> Result<EndpointBuilder> {
    let transport_config = build_quic_transport_config(transport_tuning)?;
    // iroh 1.0 requires the crypto provider to be set explicitly on the builder
    // when starting from the `Empty` preset — the `tls-ring` feature only makes
    // the ring backend available, it does not wire it in, and rustls' global
    // `install_default()` is not consulted.
    let crypto_provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut builder = Endpoint::builder(presets::Empty)
        .relay_mode(relay_config.relay_mode())
        .transport_config(transport_config)
        .crypto_provider(crypto_provider);

    if relay_only {
        builder = builder.clear_ip_transports();
    }

    if !relay_only {
        if relay_config.is_custom() {
            info!("Internet discovery disabled (custom relays configured)");
        } else {
            // Default n0 relays: always resolve through n0 DNS, but only publish
            // (pkarr) when we have a persistent identity. An ephemeral endpoint
            // (no secret) shouldn't advertise itself.
            if secret_key.is_some() {
                builder = builder.address_lookup(PkarrPublisher::n0_dns());
            }
            builder = builder.address_lookup(DnsAddressLookup::n0_dns());
        }
        // mDNS always enabled for local network discovery
        builder = builder.address_lookup(MdnsAddressLookup::builder());
    }

    Ok(builder)
}

/// Build a minimal, relay-only endpoint for probing a single relay.
///
/// It uses an ephemeral identity (no persistent secret, no address publishing)
/// and clears IP transports so [`Endpoint::online`] reflects *pure relay*
/// connectivity — a holepunched direct path can never mask a dead or
/// auth-rejecting relay. The auth token, when set, rides the WebSocket upgrade
/// exactly as it does for the real endpoint, so the probe validates the token too.
fn probe_endpoint_builder(
    relay_url: &RelayUrl,
    auth_token: Option<&str>,
) -> Result<EndpointBuilder> {
    let transport_config = build_quic_transport_config(None)?;
    let map = RelayMap::from_iter([relay_url.clone()]);
    let map = match auth_token {
        Some(token) => map.with_auth_token(token.to_string()),
        None => map,
    };
    let builder = Endpoint::builder(presets::Empty)
        .relay_mode(RelayMode::Custom(map))
        .transport_config(transport_config)
        .crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
        // Relay-only: drop direct IP transports so `online()` is a pure relay
        // reachability signal, independent of holepunching.
        .clear_ip_transports();
    Ok(builder)
}

/// Probe a single custom relay by binding a relay-only endpoint and waiting for
/// it to come online, bounded by [`RELAY_CONNECT_TIMEOUT`]. `Ok(())` means the
/// relay connected (and accepted the auth token, if any); otherwise the error
/// describes the failure. The probe endpoint is always closed before returning.
async fn probe_relay(relay_url: &RelayUrl, auth_token: Option<&str>) -> Result<()> {
    let endpoint = probe_endpoint_builder(relay_url, auth_token)?
        .bind()
        .await
        .with_context(|| format!("Failed to bind probe endpoint for relay {relay_url}"))?;
    let outcome = tokio::time::timeout(RELAY_CONNECT_TIMEOUT, endpoint.online()).await;
    endpoint.close().await;
    outcome.map_err(|_| {
        anyhow::anyhow!(
            "did not come online within {}s (unreachable or rejected the auth token)",
            RELAY_CONNECT_TIMEOUT.as_secs()
        )
    })
}

/// Probe every configured custom relay individually (in parallel) and fail if
/// **any** relay is unreachable.
///
/// This is stricter than a single endpoint-wide `online()` wait, which only
/// proves that *one* relay in the set (the home relay) connected and so reports
/// a misleading all-clear when a backup relay is down. A configured relay that
/// is silently dead is worse than a startup failure: it is a failover path that
/// does not actually exist.
///
/// **Startup is strict; runtime is not.** Once the endpoint is bound, losing a
/// relay is survivable — iroh re-homes onto a surviving configured relay within
/// ~30s, and [`connect_to_server`] keeps dialing across the whole relay set.
/// Default relays are not probed (returns `Ok(())` immediately).
async fn probe_custom_relays(relay_config: &RelayConfig) -> Result<()> {
    let RelayConfig::Custom { urls, auth_token } = relay_config else {
        return Ok(());
    };
    let token = auth_token.as_deref();
    info!("Probing {} custom relay(s) for reachability...", urls.len());
    let results = join_all(
        urls.iter()
            .map(|url| async move { (url, probe_relay(url, token).await) }),
    )
    .await;
    let failures: Vec<String> = results
        .into_iter()
        .filter_map(|(url, res)| res.err().map(|e| format!("{url}: {e}")))
        .collect();
    if !failures.is_empty() {
        return Err(TunnelError::connection(anyhow::anyhow!(
            "{} of {} custom relay(s) failed to come online:\n  {}",
            failures.len(),
            urls.len(),
            failures.join("\n  ")
        ))
        .into());
    }
    Ok(())
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
///
/// With the default relays internet discovery is on, so the server publishes its
/// current home relay and clients resolve it by endpoint ID. With custom relays
/// discovery is off, so clients reach the server through the relay hints they
/// attach to its `EndpointAddr` (see [`create_endpoint_builder`]).
pub async fn create_server_endpoint(
    relay_config: &RelayConfig,
    relay_only: bool,
    secret: Option<SecretKey>,
    alpn: &[u8],
    transport_tuning: Option<&TransportTuning>,
) -> Result<Endpoint> {
    print_relay_status(relay_config, relay_only);

    // Validate each custom relay individually (fail if any is unreachable); a
    // no-op for the default relays.
    probe_custom_relays(relay_config).await?;

    let mut builder =
        create_endpoint_builder(relay_config, relay_only, secret.as_ref(), transport_tuning)?
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
    relay_config: &RelayConfig,
    relay_only: bool,
    secret_key: Option<&SecretKey>,
    transport_tuning: Option<&TransportTuning>,
) -> Result<Endpoint> {
    print_relay_status(relay_config, relay_only);

    // Validate each custom relay individually (fail if any is unreachable); a
    // no-op for the default relays.
    probe_custom_relays(relay_config).await?;

    let mut builder =
        create_endpoint_builder(relay_config, relay_only, secret_key, transport_tuning)?;

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
    fn empty_urls_no_token_is_default() {
        let cfg = RelayConfig::from_urls_with_token(&[], None).unwrap();
        assert_eq!(cfg, RelayConfig::Default);
        assert!(!cfg.is_custom());
        assert_eq!(cfg.relay_auth_token(), None);
    }

    #[test]
    fn blank_token_without_urls_is_default() {
        // A whitespace-only token normalizes to None, so it is not an error.
        let cfg = RelayConfig::from_urls_with_token(&[], Some("   ".to_string())).unwrap();
        assert_eq!(cfg, RelayConfig::Default);
    }

    #[test]
    fn token_without_custom_urls_is_error() {
        let err = RelayConfig::from_urls_with_token(&[], Some("secret".to_string()))
            .expect_err("token without custom relays must be rejected");
        assert!(
            err.to_string()
                .contains("relay_auth_token requires custom relay_urls"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn malformed_custom_url_is_rejected_without_token() {
        // Custom relays are always parse-validated, independent of any token.
        let err = RelayConfig::from_urls_with_token(&["not a url".to_string()], None)
            .expect_err("malformed relay URL must be rejected");
        assert!(
            err.to_string().contains("Invalid relay URL"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn custom_urls_without_token() {
        let cfg = RelayConfig::from_urls_with_token(&[RELAY.to_string()], None).unwrap();
        assert!(cfg.is_custom());
        assert_eq!(cfg.custom_urls().len(), 1);
        assert_eq!(cfg.relay_auth_token(), None);
        assert!(matches!(cfg.relay_mode(), RelayMode::Custom(_)));
    }

    #[test]
    fn custom_urls_with_token_trimmed() {
        let cfg =
            RelayConfig::from_urls_with_token(&[RELAY.to_string()], Some("  secret\n".to_string()))
                .unwrap();
        assert!(cfg.is_custom());
        assert_eq!(cfg.relay_auth_token(), Some("secret"));
        assert!(matches!(cfg.relay_mode(), RelayMode::Custom(_)));
    }

    #[test]
    fn token_is_trimmed_to_none_with_custom_urls() {
        // A blank token alongside custom relays is simply no token, not an error.
        let cfg =
            RelayConfig::from_urls_with_token(&[RELAY.to_string()], Some("  ".to_string())).unwrap();
        assert!(cfg.is_custom());
        assert_eq!(cfg.relay_auth_token(), None);
    }

    #[test]
    fn duplicate_custom_urls_are_deduped() {
        let cfg =
            RelayConfig::from_urls(&[RELAY.to_string(), RELAY.to_string()]).unwrap();
        assert_eq!(cfg.custom_urls().len(), 1);
    }

    #[test]
    fn from_urls_carries_no_token() {
        let cfg = RelayConfig::from_urls(&[RELAY.to_string()]).unwrap();
        assert_eq!(cfg.relay_auth_token(), None);
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
