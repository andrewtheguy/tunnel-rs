//! Configuration file support for tunnel-rs.
//!
//! Configuration structure:
//! - `role` field for validation
//! - Transport section: [iroh]
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

/// iroh transport configuration (multi-source).
///
/// Some fields are role-specific (enforced by validate()):
/// - Server-only: `allowed_sources`, `max_sessions`, `authorized_keys_file`, `authorized_keys`,
///   `secret`, `secret_file`
/// - Client-only: `request_source`, `target`, `server_node_id`, `private_key_file`, `private_key`
#[derive(Deserialize, Default, Clone)]
#[serde(deny_unknown_fields)]
pub struct IrohConfig {
    /// Path to secret key file for persistent server identity (server only)
    pub secret_file: Option<PathBuf>,
    /// Base64-encoded secret key for persistent server identity (server only).
    /// Prefer `secret_file` in production; inline secrets are best kept to testing or
    /// special cases due to VCS/log exposure risk. Secret files should be 0600 on Unix.
    pub secret: Option<String>,
    pub relay_urls: Option<Vec<String>>,
    /// Shared bearer token sent to every *custom* relay on the WebSocket
    /// upgrade (`Authorization: Bearer <token>`). Only meaningful together with
    /// `relay_urls`; setting it without custom relays is rejected, since the
    /// default iroh relays never take a token.
    pub relay_auth_token: Option<String>,
    /// NodeId of the server to connect to (client only)
    pub server_node_id: Option<String>,
    /// Allowed source networks in CIDR notation (server only).
    /// Clients must request sources within these networks.
    pub allowed_sources: Option<AllowedSources>,
    /// Maximum concurrent sessions (server only, default: 100)
    pub max_sessions: Option<usize>,
    /// Path to an SSH-style authorized-keys file containing Ed25519 public keys (server only).
    pub authorized_keys_file: Option<PathBuf>,
    /// Inline authorized-keys entries, one per array element (server only).
    /// Same syntax as a line of `authorized_keys_file`. Stdin configs only;
    /// config files must use `authorized_keys_file`.
    pub authorized_keys: Option<Vec<String>>,
    /// Path to a compact tunnel-rs Ed25519 private key (client only).
    pub private_key_file: Option<PathBuf>,
    /// Inline compact tunnel-rs Ed25519 private key (client only).
    /// Accepts the bare `tunnelrsv1authsecret:` token or the whole generated key
    /// file. Stdin configs only; config files must use `private_key_file`.
    pub private_key: Option<String>,
    /// Source URL to request from server (client only).
    /// Format: tcp://host:port or udp://host:port
    #[serde(alias = "source")]
    pub request_source: Option<String>,
    /// Local address to listen on (client only).
    /// Format: host:port
    pub target: Option<String>,
    /// Transport layer tuning (congestion control, buffer sizes).
    #[serde(default)]
    pub transport: TransportTuning,
}

impl IrohConfig {
    /// Inline keys are allowed for stdin automation but not in persistent TOML
    /// files, where they would leak into VCS and backups.
    fn reject_inline_keys(&self) -> Result<()> {
        if self.secret.is_some() {
            anyhow::bail!(
                "[iroh] Plaintext 'secret' is not allowed in config files. \
                 Use 'secret_file' or set TUNNEL_RS_SECRET."
            );
        }
        if self.private_key.is_some() {
            anyhow::bail!(
                "[iroh] Inline 'private_key' is not allowed in config files. \
                 Use 'private_key_file', or pass it in a --config-stdin JSON config."
            );
        }
        if self.authorized_keys.is_some() {
            anyhow::bail!(
                "[iroh] Inline 'authorized_keys' is not allowed in config files. \
                 Use 'authorized_keys_file', or pass them in a --config-stdin JSON config."
            );
        }
        Ok(())
    }
}

/// Allowed source networks for client-requested source feature.
/// Separate CIDR lists for TCP and UDP protocols.
#[derive(Deserialize, Default, Clone)]
#[serde(deny_unknown_fields)]
pub struct AllowedSources {
    /// Allowed TCP source networks (CIDR notation, e.g., "127.0.0.0/8", "::1/128")
    #[serde(default)]
    pub tcp: Vec<String>,
    /// Allowed UDP source networks (CIDR notation)
    #[serde(default)]
    pub udp: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSource {
    /// TOML file on disk — plaintext secrets rejected
    File,
    /// JSON from stdin — all values allowed
    Stdin,
    /// No config, defaults only
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Server,
    Client,
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

/// Default QUIC stream receive window size (64 MB).
pub const DEFAULT_STREAM_RECEIVE_WINDOW: u32 = 64 * 1024 * 1024;

/// Default QUIC send window size (64 MB).
pub const DEFAULT_SEND_WINDOW: u32 = 64 * 1024 * 1024;

/// Transport tuning configuration for QUIC connections.
///
/// These settings affect performance and memory usage of the QUIC transport layer.
#[derive(Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TransportTuning {
    /// Congestion controller algorithm (default: cubic).
    /// Options: cubic, bbr, newreno
    #[serde(default)]
    pub congestion_controller: CongestionController,

    /// QUIC stream receive window size in bytes (default: 67108864 = 64MB).
    /// Controls per-stream flow control. The connection receive window uses iroh's default.
    /// Valid range: 1024 to 67108864 (64MB).
    pub receive_window: Option<u32>,

    /// QUIC send window size in bytes (default: 67108864 = 64MB).
    /// Controls how much data can be sent before acknowledgment.
    /// Valid range: 1024 to 67108864 (64MB).
    pub send_window: Option<u32>,

    /// QUIC ACK-eliciting threshold: the number of ack-eliciting packets the
    /// peer may receive before it must send an ACK to us.
    ///
    /// This requests the peer to delay acknowledgements of the data *we* send,
    /// so it directly affects our own sender-side ACK clock. A larger value
    /// reduces ACK overhead but starves congestion control of feedback, which
    /// hurts bulk-sending endpoints. A value of 0 makes the peer ACK every
    /// packet.
    ///
    /// When unset, the ACK_FREQUENCY extension is left at iroh/quinn defaults
    /// (peer ACKs every other packet). Only set this if you have measured a
    /// benefit. Valid range: 0 to 65535.
    pub ack_eliciting_threshold: Option<u32>,
}

/// Unified server configuration.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    // Validation field
    pub role: Option<Role>,

    // Shared options
    pub source: Option<String>,

    // Transport section
    pub iroh: Option<IrohConfig>,
}

/// Unified client configuration.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ClientConfig {
    // Validation field
    pub role: Option<Role>,

    // Transport section
    pub iroh: Option<IrohConfig>,
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

/// Maximum QUIC window size (64 MB).
const MAX_WINDOW_SIZE: u32 = 64 * 1024 * 1024;

/// Maximum QUIC ACK-eliciting threshold (fits in a QUIC VarInt and stays sane).
const MAX_ACK_ELICITING_THRESHOLD: u32 = 65535;

/// Validate QUIC window size is within acceptable range (1024-67108864 bytes).
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
            "[{}] {} value {} exceeds maximum of {} bytes (64MB)",
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
    if let Some(threshold) = tuning.ack_eliciting_threshold
        && threshold > MAX_ACK_ELICITING_THRESHOLD
    {
        anyhow::bail!(
            "[{}] ack_eliciting_threshold value {} exceeds maximum of {}",
            section,
            threshold,
            MAX_ACK_ELICITING_THRESHOLD
        );
    }
    Ok(())
}

// ============================================================================
// Config Accessor Methods
// ============================================================================

impl ServerConfig {
    /// Get iroh config section.
    pub fn iroh(&self) -> Option<&IrohConfig> {
        self.iroh.as_ref()
    }

    /// Validate that config matches the expected role.
    pub fn validate(&self, source: ConfigSource) -> Result<()> {
        let role = self
            .role
            .context("Config file missing required 'role' field. Add: role = \"server\"")?;
        if role != Role::Server {
            anyhow::bail!("Config file has role = \"client\", but running as server");
        }

        if let Some(ref iroh) = self.iroh {
            if source == ConfigSource::File {
                iroh.reject_inline_keys()?;
            }
            if iroh.secret.is_some() && iroh.secret_file.is_some() {
                anyhow::bail!("[iroh] Use only one of 'secret' or 'secret_file'.");
            }
            if iroh.authorized_keys.is_some() && iroh.authorized_keys_file.is_some() {
                anyhow::bail!("[iroh] Use only one of 'authorized_keys' or 'authorized_keys_file'.");
            }
            if let Some(ref keys) = iroh.authorized_keys
                && keys.is_empty()
            {
                anyhow::bail!("[iroh] 'authorized_keys' is empty. List at least one public key.");
            }
            if iroh.private_key_file.is_some() || iroh.private_key.is_some() {
                anyhow::bail!("[iroh] 'private_key' / 'private_key_file' are client-only fields.");
            }
            if iroh.request_source.is_some() || iroh.target.is_some() {
                anyhow::bail!(
                    "[iroh] 'source' / 'request_source' / 'target' are client-only fields. \
                    Servers use 'allowed_sources' to restrict what clients can request."
                );
            }
            if iroh.server_node_id.is_some() {
                anyhow::bail!("[iroh] 'server_node_id' is a client-only field.");
            }
            if let Some(ref allowed) = iroh.allowed_sources {
                validate_allowed_sources(allowed)?;
            }
            validate_transport_tuning(&iroh.transport, "iroh")?;
        }
        if self.source.is_some() {
            anyhow::bail!(
                "Top-level 'source' is not allowed for server mode. \
                Use [iroh.allowed_sources] to restrict what clients can request."
            );
        }

        Ok(())
    }
}

impl ClientConfig {
    /// Get iroh config section.
    pub fn iroh(&self) -> Option<&IrohConfig> {
        self.iroh.as_ref()
    }

    /// Validate that config matches the expected role.
    pub fn validate(&self, source: ConfigSource) -> Result<()> {
        let role = self
            .role
            .context("Config file missing required 'role' field. Add: role = \"client\"")?;
        if role != Role::Client {
            anyhow::bail!("Config file has role = \"server\", but running as client");
        }

        if let Some(ref iroh) = self.iroh {
            if source == ConfigSource::File {
                iroh.reject_inline_keys()?;
            }
            if iroh.private_key.is_some() && iroh.private_key_file.is_some() {
                anyhow::bail!("[iroh] Use only one of 'private_key' or 'private_key_file'.");
            }
            if iroh.allowed_sources.is_some() {
                anyhow::bail!(
                    "[iroh] 'allowed_sources' is a server-only field. \
                    Clients use 'source' to specify what to request from server."
                );
            }
            if iroh.max_sessions.is_some() {
                anyhow::bail!("[iroh] 'max_sessions' is a server-only field.");
            }
            if iroh.authorized_keys_file.is_some() || iroh.authorized_keys.is_some() {
                anyhow::bail!(
                    "[iroh] 'authorized_keys' / 'authorized_keys_file' are server-only fields."
                );
            }
            if iroh.secret.is_some() || iroh.secret_file.is_some() {
                anyhow::bail!(
                    "[iroh] 'secret' and 'secret_file' are server-only fields. \
                    Clients keep ephemeral iroh identities and authenticate with a separate Ed25519 key."
                );
            }
            if let Some(ref source) = iroh.request_source {
                validate_tcp_udp_url(source, "request_source")?;
            }
            if let Some(ref target) = iroh.target {
                validate_host_port(target, "target")?;
            }
            validate_transport_tuning(&iroh.transport, "iroh")?;
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
    } else if path_str == "~"
        && let Some(home) = dirs::home_dir() {
            return home;
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
/// Uses `serde_json::Deserializer::from_reader` to parse exactly one JSON value
/// without calling `end()`, so it returns immediately after the closing `}`
/// without waiting for EOF. This leaves the rest of the stream unconsumed.
/// Times out after 30 seconds since this is intended for automation/IPC.
pub async fn parse_config_from_reader<T: for<'de> Deserialize<'de> + Send + 'static, R: std::io::Read + Send + 'static>(reader: R) -> Result<T> {
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        tokio::task::spawn_blocking(move || {
            let mut de = serde_json::Deserializer::from_reader(reader);
            T::deserialize(&mut de).context("Failed to parse JSON config from stdin")
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

#[cfg(test)]
mod tests {
    use super::*;

    fn client_config_with_iroh(iroh: IrohConfig) -> ClientConfig {
        ClientConfig {
            role: Some(Role::Client),
            iroh: Some(iroh),
        }
    }

    /// `unwrap_err` would need `Debug` on the config, which the config types
    /// deliberately lack so inline keys can never be printed.
    fn parse_error<T, E: std::fmt::Display>(result: std::result::Result<T, E>) -> String {
        match result {
            Ok(_) => panic!("expected a parse error"),
            Err(err) => err.to_string(),
        }
    }

    fn server_config_with_iroh(iroh: IrohConfig) -> ServerConfig {
        ServerConfig {
            role: Some(Role::Server),
            source: None,
            iroh: Some(iroh),
        }
    }

    #[test]
    fn server_rejects_plaintext_secret_from_file() {
        let cfg = server_config_with_iroh(IrohConfig {
            secret: Some("base64secret".into()),
            ..Default::default()
        });
        let err = cfg.validate(ConfigSource::File).unwrap_err();
        assert!(err.to_string().contains("Plaintext 'secret'"));
    }

    #[test]
    fn server_allows_plaintext_secret_from_stdin() {
        let cfg = server_config_with_iroh(IrohConfig {
            secret: Some("base64secret".into()),
            ..Default::default()
        });
        assert!(cfg.validate(ConfigSource::Stdin).is_ok());
    }

    #[test]
    fn server_rejects_client_private_key_field() {
        let cfg = server_config_with_iroh(IrohConfig {
            private_key_file: Some("client_key".into()),
            ..Default::default()
        });
        let err = cfg.validate(ConfigSource::File).unwrap_err();
        assert!(err.to_string().contains("client-only"));
    }

    #[test]
    fn client_rejects_server_authorized_keys_field() {
        let cfg = client_config_with_iroh(IrohConfig {
            authorized_keys_file: Some("authorized_keys".into()),
            ..Default::default()
        });
        let err = cfg.validate(ConfigSource::File).unwrap_err();
        assert!(err.to_string().contains("server-only"));
    }

    #[test]
    fn client_rejects_inline_private_key_from_file() {
        let cfg = client_config_with_iroh(IrohConfig {
            private_key: Some("tunnelrsv1authsecret:key".into()),
            ..Default::default()
        });
        let err = cfg.validate(ConfigSource::File).unwrap_err();
        assert!(err.to_string().contains("Inline 'private_key'"));
    }

    #[test]
    fn client_allows_inline_private_key_from_stdin() {
        let cfg = client_config_with_iroh(IrohConfig {
            private_key: Some("tunnelrsv1authsecret:key".into()),
            ..Default::default()
        });
        assert!(cfg.validate(ConfigSource::Stdin).is_ok());
    }

    #[test]
    fn client_rejects_both_private_key_forms() {
        let cfg = client_config_with_iroh(IrohConfig {
            private_key: Some("tunnelrsv1authsecret:key".into()),
            private_key_file: Some("client.key".into()),
            ..Default::default()
        });
        let err = cfg.validate(ConfigSource::Stdin).unwrap_err();
        assert!(err.to_string().contains("only one of 'private_key'"));
    }

    #[test]
    fn server_rejects_inline_authorized_keys_from_file() {
        let cfg = server_config_with_iroh(IrohConfig {
            authorized_keys: Some(vec!["tunnelrsv1authpub:key".into()]),
            ..Default::default()
        });
        let err = cfg.validate(ConfigSource::File).unwrap_err();
        assert!(err.to_string().contains("Inline 'authorized_keys'"));
    }

    #[test]
    fn server_allows_inline_authorized_keys_from_stdin() {
        let cfg = server_config_with_iroh(IrohConfig {
            authorized_keys: Some(vec!["tunnelrsv1authpub:key".into()]),
            ..Default::default()
        });
        assert!(cfg.validate(ConfigSource::Stdin).is_ok());
    }

    #[test]
    fn server_rejects_both_authorized_keys_forms() {
        let cfg = server_config_with_iroh(IrohConfig {
            authorized_keys: Some(vec!["tunnelrsv1authpub:key".into()]),
            authorized_keys_file: Some("authorized_keys".into()),
            ..Default::default()
        });
        let err = cfg.validate(ConfigSource::Stdin).unwrap_err();
        assert!(err.to_string().contains("only one of 'authorized_keys'"));
    }

    #[test]
    fn server_rejects_empty_inline_authorized_keys() {
        let cfg = server_config_with_iroh(IrohConfig {
            authorized_keys: Some(Vec::new()),
            ..Default::default()
        });
        let err = cfg.validate(ConfigSource::Stdin).unwrap_err();
        assert!(err.to_string().contains("is empty"));
    }

    #[test]
    fn server_rejects_client_inline_private_key() {
        let cfg = server_config_with_iroh(IrohConfig {
            private_key: Some("tunnelrsv1authsecret:key".into()),
            ..Default::default()
        });
        let err = cfg.validate(ConfigSource::Stdin).unwrap_err();
        assert!(err.to_string().contains("client-only"));
    }

    #[test]
    fn client_rejects_server_inline_authorized_keys() {
        let cfg = client_config_with_iroh(IrohConfig {
            authorized_keys: Some(vec!["tunnelrsv1authpub:key".into()]),
            ..Default::default()
        });
        let err = cfg.validate(ConfigSource::Stdin).unwrap_err();
        assert!(err.to_string().contains("server-only"));
    }

    #[test]
    fn unknown_toml_fields_are_rejected() {
        let cases = [
            "role = \"client\"\nrelay_only = true\n",
            "role = \"client\"\n[iroh]\nsecrete_file = \"x\"\n",
            "role = \"client\"\n[iroh.transport]\nsend_windows = 2048\n",
        ];
        for case in cases {
            let err = parse_error(toml::from_str::<ClientConfig>(case));
            assert!(
                err.contains("unknown field"),
                "expected unknown field error for {case:?}, got: {err}"
            );
        }

        let err = parse_error(toml::from_str::<ServerConfig>(
            "role = \"server\"\n[iroh.allowed_sources]\ntpc = [\"127.0.0.0/8\"]\n",
        ));
        assert!(err.contains("unknown field"), "got: {err}");
    }

    /// The shipped examples are the reference for every documented field, so
    /// `deny_unknown_fields` must accept them verbatim.
    #[test]
    fn example_configs_parse() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let server = std::fs::read_to_string(root.join("server.toml.example")).unwrap();
        let client = std::fs::read_to_string(root.join("client.toml.example")).unwrap();

        toml::from_str::<ServerConfig>(&server)
            .unwrap_or_else(|err| panic!("server.toml.example: {err}"))
            .validate(ConfigSource::File)
            .unwrap();
        toml::from_str::<ClientConfig>(&client)
            .unwrap_or_else(|err| panic!("client.toml.example: {err}"))
            .validate(ConfigSource::File)
            .unwrap();
    }

    #[test]
    fn unknown_stdin_json_fields_are_rejected() {
        let err = parse_error(serde_json::from_str::<ServerConfig>(
            r#"{"role":"server","iroh":{"authorised_keys":["k"]}}"#,
        ));
        assert!(err.contains("unknown field"), "got: {err}");
    }

    #[test]
    fn stdin_json_accepts_inline_keys() {
        let cfg: ClientConfig = serde_json::from_str(
            r#"{"role":"client","iroh":{"private_key":"tunnelrsv1authsecret:key"}}"#,
        )
        .unwrap();
        assert_eq!(
            cfg.iroh().unwrap().private_key.as_deref(),
            Some("tunnelrsv1authsecret:key")
        );

        let cfg: ServerConfig = serde_json::from_str(
            r#"{"role":"server","iroh":{"authorized_keys":["tunnelrsv1authpub:a","tunnelrsv1authpub:b c"]}}"#,
        )
        .unwrap();
        assert_eq!(cfg.iroh().unwrap().authorized_keys.as_ref().unwrap().len(), 2);
    }
}
