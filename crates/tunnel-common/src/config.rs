//! Configuration file support for tunnel-rs.
//!
//! Configuration structure:
//! - `role` and `mode` fields for validation
//! - Mode-specific sections: [iroh], [manual], [nostr]
//! - All modes use client-initiated source requests
//!
//! Role-based field semantics are enforced by `validate()` at parse time:
//! - Server-only fields are rejected when role=client
//! - Client-only fields are rejected when role=server
//! - CIDR networks and source URLs are validated for correct format

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

// ============================================================================
// Configuration Structures
// ============================================================================

/// iroh mode configuration (multi-source).
///
/// Some fields are role-specific (enforced by validate()):
/// - Server-only: `allowed_sources`, `max_sessions`, `auth_tokens`, `auth_tokens_file`, `secret`, `secret_file`
/// - Client-only: `request_source`, `target`, `server_node_id`, `auth_token`, `auth_token_file`
#[derive(Deserialize, Default, Clone)]
pub struct IrohConfig {
    /// Path to secret key file for persistent server identity (server only)
    pub secret_file: Option<PathBuf>,
    /// Base64-encoded secret key for persistent server identity (server only).
    /// Prefer `secret_file` in production; inline secrets are best kept to testing or
    /// special cases due to VCS/log exposure risk. Secret files should be 0600 on Unix.
    pub secret: Option<String>,
    pub relay_urls: Option<Vec<String>>,
    pub dns_server: Option<String>,
    /// NodeId of the server to connect to (client only)
    pub server_node_id: Option<String>,
    /// Allowed source networks in CIDR notation (server only).
    /// Clients must request sources within these networks.
    pub allowed_sources: Option<AllowedSources>,
    /// Maximum concurrent sessions (server only, default: 100)
    pub max_sessions: Option<usize>,
    /// Authentication tokens (server only).
    /// Clients must provide one of these tokens to authenticate.
    /// Prefer `auth_tokens_file` in production; inline tokens are best kept to testing or
    /// special cases due to VCS/log exposure risk.
    pub auth_tokens: Option<Vec<String>>,
    /// Path to file containing authentication tokens (server only).
    /// One token per line, # comments allowed.
    pub auth_tokens_file: Option<PathBuf>,
    /// Authentication token to send to server (client only).
    /// Prefer `auth_token_file` in production to avoid exposing tokens in config files.
    pub auth_token: Option<String>,
    /// Path to file containing authentication token (client only).
    pub auth_token_file: Option<PathBuf>,
    /// Source URL to request from server (client only).
    /// Format: tcp://host:port or udp://host:port
    #[serde(alias = "source")]
    pub request_source: Option<String>,
    /// Local address to listen on (client only).
    /// Format: host:port
    pub target: Option<String>,
    /// ALPN token for QUIC handshake-level filtering.
    /// Both server and client must use the same token.
    pub alpn_token: Option<String>,
    /// Path to file containing the ALPN token.
    pub alpn_token_file: Option<PathBuf>,
    /// Transport layer tuning (congestion control, buffer sizes).
    #[serde(default)]
    pub transport: TransportTuning,
}

/// manual mode configuration.
///
/// Some fields are role-specific (enforced by validate()):
/// - Server-only: `allowed_sources`
/// - Client-only: `request_source`, `target`
#[derive(Deserialize, Default, Clone)]
pub struct CustomManualConfig {
    pub stun_servers: Option<Vec<String>>,
    /// Allowed source networks in CIDR notation (server only).
    /// Clients must request sources within these networks.
    pub allowed_sources: Option<AllowedSources>,
    /// Source URL to request from server (client only, required).
    /// Format: tcp://host:port or udp://host:port
    #[serde(alias = "source")]
    pub request_source: Option<String>,
    /// Local address to listen on (client only, required).
    /// Format: host:port (no protocol prefix)
    pub target: Option<String>,
    /// Transport layer tuning (congestion control, buffer sizes).
    #[serde(default)]
    pub transport: TransportTuning,
}

/// Allowed source networks for client-requested source feature.
/// Separate CIDR lists for TCP and UDP protocols.
#[derive(Deserialize, Default, Clone)]
pub struct AllowedSources {
    /// Allowed TCP source networks (CIDR notation, e.g., "127.0.0.0/8", "::1/128")
    #[serde(default)]
    pub tcp: Vec<String>,
    /// Allowed UDP source networks (CIDR notation)
    #[serde(default)]
    pub udp: Vec<String>,
}

/// nostr mode configuration (TOML section: `[nostr]`).
///
/// nostr provides full ICE with automated Nostr relay signaling for static peer key exchange.
///
/// Some fields are role-specific (enforced by validate()):
/// - Server-only: `allowed_sources`, `max_sessions`
/// - Client-only: `request_source`, `target`
#[derive(Deserialize, Default, Clone)]
pub struct NostrConfig {
    /// Nostr relay URLs for signaling
    pub relays: Option<Vec<String>>,
    /// Your Nostr private key (nsec or hex)
    pub nsec: Option<String>,
    /// Path to file containing your Nostr private key (nsec or hex)
    pub nsec_file: Option<PathBuf>,
    /// Peer's Nostr public key (npub or hex)
    pub peer_npub: Option<String>,
    /// STUN servers for ICE candidate gathering
    pub stun_servers: Option<Vec<String>>,
    /// Maximum concurrent sessions (server only, default: 10)
    pub max_sessions: Option<usize>,
    /// Allowed source networks in CIDR notation (server only).
    /// Clients must request sources within these networks.
    pub allowed_sources: Option<AllowedSources>,
    /// Source URL to request from server (client only, required).
    /// Format: tcp://host:port or udp://host:port
    #[serde(alias = "source")]
    pub request_source: Option<String>,
    /// Local address to listen on (client only, required).
    /// Format: host:port (no protocol prefix)
    pub target: Option<String>,
    /// Transport layer tuning (congestion control, buffer sizes).
    #[serde(default)]
    pub transport: TransportTuning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Server,
    Client,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Iroh,
    Manual,
    Nostr,
}

impl Mode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Mode::Iroh => "iroh",
            Mode::Manual => "manual",
            Mode::Nostr => "nostr",
        }
    }
}

/// Congestion controller algorithm selection.
///
/// Controls how the QUIC connection manages congestion and adjusts sending rates.
/// Default is Cubic, which is the most widely tested algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CongestionController {
    /// CUBIC - Default. Loss-based congestion control, widely deployed.
    /// Best for general internet conditions.
    #[default]
    Cubic,
    /// BBR (Bottleneck Bandwidth and RTT) - Model-based congestion control.
    /// May perform better on high-bandwidth, high-latency links.
    /// Experimental - may not be fair to Cubic/NewReno flows.
    Bbr,
    /// NewReno - Classic TCP-like congestion control.
    /// Most conservative, good for compatibility.
    #[serde(alias = "new_reno")]
    NewReno,
}

/// Default QUIC receive window size (8 MB).
pub const DEFAULT_RECEIVE_WINDOW: u32 = 8 * 1024 * 1024;

/// Default QUIC send window size (8 MB).
pub const DEFAULT_SEND_WINDOW: u32 = 8 * 1024 * 1024;

/// Transport tuning configuration for QUIC connections.
///
/// These settings affect performance and memory usage of the QUIC transport layer.
#[derive(Deserialize, Default, Clone, Debug, PartialEq)]
pub struct TransportTuning {
    /// Congestion controller algorithm (default: cubic).
    /// Options: cubic, bbr, newreno
    #[serde(default)]
    pub congestion_controller: CongestionController,

    /// QUIC receive window size in bytes (default: 8388608 = 8MB).
    /// Controls flow control - larger values allow more in-flight data.
    /// Valid range: 1024 to 16777216 (16MB).
    pub receive_window: Option<u32>,

    /// QUIC send window size in bytes (default: 8388608 = 8MB).
    /// Controls how much data can be sent before acknowledgment.
    /// Valid range: 1024 to 16777216 (16MB).
    pub send_window: Option<u32>,
}

fn parse_expected_mode(expected_mode: &str) -> Result<Mode> {
    match expected_mode {
        "iroh" => Ok(Mode::Iroh),
        "manual" => Ok(Mode::Manual),
        "nostr" => Ok(Mode::Nostr),
        _ => anyhow::bail!(
            "Unknown mode '{}'. Valid modes: iroh, manual, nostr",
            expected_mode
        ),
    }
}

/// Unified server configuration.
#[derive(Deserialize, Default)]
pub struct ServerConfig {
    // Validation fields
    pub role: Option<Role>,
    pub mode: Option<Mode>,

    // Shared options
    pub source: Option<String>,

    // Mode-specific sections
    pub iroh: Option<IrohConfig>,
    pub manual: Option<CustomManualConfig>,
    pub nostr: Option<NostrConfig>,
}

/// Unified client configuration.
#[derive(Deserialize, Default)]
pub struct ClientConfig {
    // Validation fields
    pub role: Option<Role>,
    pub mode: Option<Mode>,

    // Mode-specific sections (each mode has its own target field)
    pub iroh: Option<IrohConfig>,
    pub manual: Option<CustomManualConfig>,
    pub nostr: Option<NostrConfig>,
}

// ============================================================================
// Validation Helpers
// ============================================================================

/// Validate that a string is a valid CIDR network (IPv4 or IPv6).
fn validate_cidr(cidr: &str) -> Result<()> {
    cidr.parse::<ipnet::IpNet>().with_context(|| {
        format!(
            "Invalid CIDR network '{}'. Expected format: 192.168.0.0/16 or ::1/128",
            cidr
        )
    })?;
    Ok(())
}

/// Validate that a string is a valid tcp:// or udp:// URL with host and port.
fn validate_tcp_udp_url(value: &str, field_name: &str) -> Result<()> {
    let url = url::Url::parse(value).with_context(|| {
        format!(
            "Invalid {} '{}'. Expected format: tcp://host:port or udp://host:port",
            field_name, value
        )
    })?;

    let scheme = url.scheme();
    if scheme != "tcp" && scheme != "udp" {
        anyhow::bail!(
            "Invalid {} scheme '{}'. Must be 'tcp' or 'udp'",
            field_name,
            scheme
        );
    }

    if url.host_str().is_none() {
        anyhow::bail!("{} '{}' missing host", field_name, value);
    }

    if url.port().is_none() {
        anyhow::bail!("{} '{}' missing port", field_name, value);
    }

    Ok(())
}

/// Validate that a string is a valid host:port address.
fn validate_host_port(value: &str, field_name: &str) -> Result<()> {
    if !value.contains(':') {
        anyhow::bail!(
            "{} '{}' missing port. Expected format: host:port",
            field_name,
            value
        );
    }

    // Use rsplitn to split from the right (handles IPv6 addresses like [::1]:8080)
    let parts: Vec<&str> = value.rsplitn(2, ':').collect();
    if parts.len() != 2 {
        anyhow::bail!(
            "{} '{}' has invalid format. Expected format: host:port",
            field_name,
            value
        );
    }

    let port_str = parts[0];
    let host = parts[1];

    if host.is_empty() {
        anyhow::bail!("{} '{}' missing host", field_name, value);
    }

    port_str
        .parse::<u16>()
        .with_context(|| format!("{} '{}' has invalid port number", field_name, value))?;

    Ok(())
}

/// Validate AllowedSources CIDR lists.
fn validate_allowed_sources(allowed: &AllowedSources) -> Result<()> {
    for cidr in &allowed.tcp {
        validate_cidr(cidr).context("Invalid TCP allowed_sources")?;
    }
    for cidr in &allowed.udp {
        validate_cidr(cidr).context("Invalid UDP allowed_sources")?;
    }
    Ok(())
}

/// Minimum QUIC window size (1 KB).
const MIN_WINDOW_SIZE: u32 = 1024;

/// Maximum QUIC window size (16 MB).
const MAX_WINDOW_SIZE: u32 = 16 * 1024 * 1024;

/// Validate QUIC window size is within acceptable range (1024-16777216 bytes).
fn validate_window_size(size: u32, field_name: &str, section: &str) -> Result<()> {
    if size < MIN_WINDOW_SIZE {
        anyhow::bail!(
            "[{}] {} value {} is below minimum of {} bytes (1KB)",
            section,
            field_name,
            size,
            MIN_WINDOW_SIZE
        );
    }
    if size > MAX_WINDOW_SIZE {
        anyhow::bail!(
            "[{}] {} value {} exceeds maximum of {} bytes (16MB)",
            section,
            field_name,
            size,
            MAX_WINDOW_SIZE
        );
    }
    Ok(())
}

/// Validate TransportTuning window sizes if specified.
pub fn validate_transport_tuning(tuning: &TransportTuning, section: &str) -> Result<()> {
    if let Some(recv) = tuning.receive_window {
        validate_window_size(recv, "receive_window", section)?;
    }
    if let Some(send) = tuning.send_window {
        validate_window_size(send, "send_window", section)?;
    }
    Ok(())
}

// ============================================================================
// Config Accessor Methods
// ============================================================================

impl ServerConfig {
    /// Get iroh config section (multi-source mode).
    pub fn iroh(&self) -> Option<&IrohConfig> {
        self.iroh.as_ref()
    }

    /// Get nostr config section.
    pub fn nostr(&self) -> Option<&NostrConfig> {
        self.nostr.as_ref()
    }

    /// Validate that config matches expected role and mode.
    ///
    /// Enforces:
    /// - Role must be "server"
    /// - Mode must match expected_mode
    /// - Multi-source modes (iroh, nostr): rejects client-only fields (request_source)
    /// - Multi-source modes: validates CIDR format in allowed_sources
    /// - Single-target modes: validates source URL format if present
    pub fn validate(&self, expected_mode: &str) -> Result<()> {
        let role = self
            .role
            .context("Config file missing required 'role' field. Add: role = \"server\"")?;
        if role != Role::Server {
            anyhow::bail!("Config file has role = \"client\", but running as server");
        }

        let mode = self.mode.context(
            "Config file missing required 'mode' field. Add: mode = \"iroh\" (or \"manual\", \"nostr\")",
        )?;
        let expected_mode = parse_expected_mode(expected_mode)?;
        if mode != expected_mode {
            anyhow::bail!(
                "Config file has mode = \"{}\", but running with {}",
                mode.as_str(),
                expected_mode.as_str()
            );
        }

        // Mode-specific validation
        if expected_mode == Mode::Iroh {
            if let Some(ref iroh) = self.iroh {
                if iroh.secret.is_some() && iroh.secret_file.is_some() {
                    anyhow::bail!("[iroh] Use only one of 'secret' or 'secret_file'.");
                }
                // Validate auth_tokens mutual exclusion
                if iroh.auth_tokens.is_some() && iroh.auth_tokens_file.is_some() {
                    anyhow::bail!("[iroh] Use only one of 'auth_tokens' or 'auth_tokens_file'.");
                }
                // Reject client-only fields (auth_token is for clients)
                if iroh.auth_token.is_some() || iroh.auth_token_file.is_some() {
                    anyhow::bail!(
                        "[iroh] 'auth_token' and 'auth_token_file' are client-only fields."
                    );
                }
                // Reject client-only fields
                if iroh.request_source.is_some() || iroh.target.is_some() {
                    anyhow::bail!(
                        "[iroh] 'source' / 'request_source' / 'target' are client-only fields. \
                        Servers use 'allowed_sources' to restrict what clients can request."
                    );
                }
                if iroh.server_node_id.is_some() {
                    anyhow::bail!("[iroh] 'server_node_id' is a client-only field.");
                }
                // Validate CIDR format
                if let Some(ref allowed) = iroh.allowed_sources {
                    validate_allowed_sources(allowed)?;
                }
            }
            // Server iroh mode should not have top-level source
            if self.source.is_some() {
                anyhow::bail!(
                    "Top-level 'source' is not allowed for iroh server mode. \
                    Use [iroh.allowed_sources] to restrict what clients can request."
                );
            }
        }
        if expected_mode == Mode::Nostr {
            if let Some(ref nostr) = self.nostr {
                if nostr.nsec.is_some() && nostr.nsec_file.is_some() {
                    anyhow::bail!("[nostr] Use only one of 'nsec' or 'nsec_file'.");
                }
                // Reject client-only fields
                if nostr.request_source.is_some() || nostr.target.is_some() {
                    anyhow::bail!(
                        "[nostr] 'source' / 'request_source' / 'target' are client-only fields. \
                        Servers use 'allowed_sources' to restrict what clients can request."
                    );
                }
                // Validate CIDR format
                if let Some(ref allowed) = nostr.allowed_sources {
                    validate_allowed_sources(allowed)?;
                }
                validate_transport_tuning(&nostr.transport, "nostr.transport")?;
            }
            // Server nostr mode should not have top-level source
            if self.source.is_some() {
                anyhow::bail!(
                    "Top-level 'source' is not allowed for nostr server mode. \
                    Use [nostr.allowed_sources] to restrict what clients can request."
                );
            }
        }
        if expected_mode == Mode::Manual {
            if let Some(ref manual) = self.manual {
                // Reject client-only fields
                if manual.request_source.is_some() || manual.target.is_some() {
                    anyhow::bail!(
                        "[manual] 'source' / 'request_source' / 'target' are client-only fields. \
                        Servers use 'allowed_sources' to restrict what clients can request."
                    );
                }
                // Validate CIDR format
                if let Some(ref allowed) = manual.allowed_sources {
                    validate_allowed_sources(allowed)?;
                }
                validate_transport_tuning(&manual.transport, "manual.transport")?;
            }
            // Reject top-level source for manual server
            if self.source.is_some() {
                anyhow::bail!(
                    "Top-level 'source' is not allowed for manual server mode. \
                    Use [manual.allowed_sources] to restrict what clients can request."
                );
            }
        }

        Ok(())
    }
}

impl ClientConfig {
    /// Get iroh config section (multi-source mode).
    pub fn iroh(&self) -> Option<&IrohConfig> {
        self.iroh.as_ref()
    }

    /// Get nostr config section.
    pub fn nostr(&self) -> Option<&NostrConfig> {
        self.nostr.as_ref()
    }

    /// Validate that config matches expected role and mode.
    ///
    /// Enforces:
    /// - Role must be "client"
    /// - Mode must match expected_mode
    /// - Multi-source modes (iroh, nostr): rejects server-only fields (allowed_sources, max_sessions)
    /// - Multi-source modes: validates request_source URL format if present
    /// - Single-target modes: validates target URL format if present
    pub fn validate(&self, expected_mode: &str) -> Result<()> {
        let role = self
            .role
            .context("Config file missing required 'role' field. Add: role = \"client\"")?;
        if role != Role::Client {
            anyhow::bail!("Config file has role = \"server\", but running as client");
        }

        let mode = self.mode.context(
            "Config file missing required 'mode' field. Add: mode = \"iroh\" (or \"manual\", \"nostr\")",
        )?;
        let expected_mode = parse_expected_mode(expected_mode)?;
        if mode != expected_mode {
            anyhow::bail!(
                "Config file has mode = \"{}\", but running with {}",
                mode.as_str(),
                expected_mode.as_str()
            );
        }

        // Mode-specific validation
        if expected_mode == Mode::Iroh {
            if let Some(ref iroh) = self.iroh {
                // Validate auth_token mutual exclusion
                if iroh.auth_token.is_some() && iroh.auth_token_file.is_some() {
                    anyhow::bail!("[iroh] Use only one of 'auth_token' or 'auth_token_file'.");
                }
                // Reject server-only fields
                if iroh.allowed_sources.is_some() {
                    anyhow::bail!(
                        "[iroh] 'allowed_sources' is a server-only field. \
                        Clients use 'source' to specify what to request from server."
                    );
                }
                if iroh.max_sessions.is_some() {
                    anyhow::bail!("[iroh] 'max_sessions' is a server-only field.");
                }
                if iroh.auth_tokens.is_some() || iroh.auth_tokens_file.is_some() {
                    anyhow::bail!(
                        "[iroh] 'auth_tokens' and 'auth_tokens_file' are server-only fields."
                    );
                }
                if iroh.secret.is_some() || iroh.secret_file.is_some() {
                    anyhow::bail!(
                        "[iroh] 'secret' and 'secret_file' are server-only fields. \
                        Clients use ephemeral identities with token authentication."
                    );
                }
                // Validate request_source URL format
                if let Some(ref source) = iroh.request_source {
                    validate_tcp_udp_url(source, "request_source")?;
                }
                // Validate target format (host:port)
                if let Some(ref target) = iroh.target {
                    validate_host_port(target, "target")?;
                }
            }
        }
        if expected_mode == Mode::Nostr {
            if let Some(ref nostr) = self.nostr {
                if nostr.nsec.is_some() && nostr.nsec_file.is_some() {
                    anyhow::bail!("[nostr] Use only one of 'nsec' or 'nsec_file'.");
                }
                // Reject server-only fields
                if nostr.allowed_sources.is_some() {
                    anyhow::bail!(
                        "[nostr] 'allowed_sources' is a server-only field. \
                        Clients use 'source' to specify what to request from server."
                    );
                }
                if nostr.max_sessions.is_some() {
                    anyhow::bail!("[nostr] 'max_sessions' is a server-only field.");
                }
                // Validate request_source URL format
                if let Some(ref source) = nostr.request_source {
                    validate_tcp_udp_url(source, "request_source")?;
                }
                // Validate target format (host:port)
                if let Some(ref target) = nostr.target {
                    validate_host_port(target, "target")?;
                }
                validate_transport_tuning(&nostr.transport, "nostr.transport")?;
            }
        }

        if expected_mode == Mode::Manual {
            if let Some(ref manual) = self.manual {
                // Reject server-only fields
                if manual.allowed_sources.is_some() {
                    anyhow::bail!(
                        "[manual] 'allowed_sources' is a server-only field. \
                        Clients use 'source' to specify what to request from server."
                    );
                }
                // Validate request_source URL format
                if let Some(ref source) = manual.request_source {
                    validate_tcp_udp_url(source, "request_source")?;
                }
                // Validate target format (host:port)
                if let Some(ref target) = manual.target {
                    validate_host_port(target, "target")?;
                }
                validate_transport_tuning(&manual.transport, "manual.transport")?;
            }
        }

        Ok(())
    }
}

// ============================================================================
// Path Expansion
// ============================================================================

/// Expand tilde (~) in paths to the user's home directory.
///
/// - `~/...` expands to the user's home directory
/// - `~` alone expands to the home directory
/// - Other paths are returned unchanged
pub fn expand_tilde(path: &Path) -> PathBuf {
    let path_str = path.to_string_lossy();
    if let Some(stripped) = path_str.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(stripped);
        }
    } else if path_str == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    path.to_path_buf()
}

// ============================================================================
// Config Loading
// ============================================================================

/// Load configuration from a TOML file.
fn load_config<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read config file: {}", path.display()))?;
    toml::from_str(&content)
        .with_context(|| format!("Failed to parse config file: {}", path.display()))
}

/// Parse configuration as JSON from a reader (e.g. stdin).
///
/// Uses `serde_json::from_reader` which reads exactly one JSON value
/// and stops, leaving the rest of the stream unconsumed.
/// Times out after 30 seconds since this is intended for automation/IPC.
pub async fn parse_config_from_reader<T: for<'de> Deserialize<'de> + Send + 'static, R: std::io::Read + Send + 'static>(reader: R) -> Result<T> {
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        tokio::task::spawn_blocking(move || {
            serde_json::from_reader(reader).context("Failed to parse JSON config from stdin")
        }),
    )
    .await
    .context("Timed out waiting for JSON config from stdin (30s)")? // timeout
    .context("Failed to read config from stdin")? // join error
}

/// Resolve the default server config path (~/.config/tunnel-rs/server.toml).
fn default_server_config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".config").join("tunnel-rs").join("server.toml"))
}

/// Resolve the default client config path (~/.config/tunnel-rs/client.toml).
fn default_client_config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".config").join("tunnel-rs").join("client.toml"))
}

/// Load server configuration from an explicit path, or from default location.
///
/// - `path`: Some(path) loads from the specified path (tilde-expanded)
/// - `path`: None loads from the default path (~/.config/tunnel-rs/server.toml)
pub fn load_server_config(path: Option<&Path>) -> Result<ServerConfig> {
    let config_path = match path {
        Some(p) => expand_tilde(p),
        None => default_server_config_path().ok_or_else(|| {
            anyhow::anyhow!("Could not find default config path. Use -c to specify a config file.")
        })?,
    };
    load_config(&config_path)
}

/// Load client configuration from an explicit path, or from default location.
///
/// - `path`: Some(path) loads from the specified path (tilde-expanded)
/// - `path`: None loads from the default path (~/.config/tunnel-rs/client.toml)
pub fn load_client_config(path: Option<&Path>) -> Result<ClientConfig> {
    let config_path = match path {
        Some(p) => expand_tilde(p),
        None => default_client_config_path().ok_or_else(|| {
            anyhow::anyhow!("Could not find default config path. Use -c to specify a config file.")
        })?,
    };
    load_config(&config_path)
}

