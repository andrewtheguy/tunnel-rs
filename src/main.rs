//! tunnel-rs
//!
//! Forwards TCP or UDP traffic through iroh P2P connections.

mod auth;
mod buffer;
mod config;
mod error;
mod iroh_mode;
mod net;
mod secret;
mod signaling;

use ::iroh::SecretKey;
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use crate::error::{ErrorCategory, TunnelError};

use crate::config::{
    expand_tilde, load_client_config, load_server_config, parse_config_from_reader,
    validate_transport_tuning, ClientConfig, ConfigSource, ServerConfig, TransportTuning,
};
use crate::iroh_mode::endpoint::{
    load_secret, load_secret_from_string, secret_to_endpoint_id, RelayConfig,
};

/// Default `env_logger` filter: tunnel-rs's own code at `info`, the noisy
/// transport deps (iroh and its tracing bridge) at `warn`. Fully overridable at
/// runtime via `RUST_LOG`.
const DEFAULT_LOG_FILTER: &str = "info,iroh=warn,tracing=warn";

#[derive(Parser)]
#[command(name = "tunnel-rs")]
#[command(version)]
#[command(about = "Forward TCP/UDP traffic through iroh P2P connections")]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run as server (accepts connections and forwards to source)
    #[command(arg_required_else_help = true)]
    Server {
        /// Path to config file
        #[arg(short, long)]
        config: Option<PathBuf>,

        /// Load config from default location (~/.config/tunnel-rs/server.toml)
        #[arg(long)]
        default_config: bool,

        /// Read JSON config from stdin for automation/IPC (use -c for normal usage)
        #[arg(long)]
        config_stdin: bool,

        /// Allowed TCP source networks in CIDR notation (repeatable)
        /// E.g., --allowed-tcp 127.0.0.0/8 --allowed-tcp 192.168.0.0/16
        #[arg(long = "allowed-tcp")]
        allowed_tcp: Vec<String>,

        /// Allowed UDP source networks in CIDR notation (repeatable)
        /// E.g., --allowed-udp 10.0.0.0/8 --allowed-udp ::1/128
        #[arg(long = "allowed-udp")]
        allowed_udp: Vec<String>,

        /// Maximum concurrent sessions (default: 100)
        #[arg(long)]
        max_sessions: Option<usize>,

        /// Path to secret key file for persistent identity
        #[arg(long)]
        secret_file: Option<PathBuf>,

        /// Custom relay server URL(s) for failover
        #[arg(long = "relay-url")]
        relay_urls: Vec<String>,

        /// Shared bearer token for the custom relay(s) (requires --relay-url).
        /// Prefer TUNNEL_RS_RELAY_AUTH_TOKEN or config to keep it out of the process list.
        #[arg(long, value_name = "TOKEN")]
        relay_auth_token: Option<String>,

        /// Force all connections through the relay server (disables direct P2P).
        #[arg(long)]
        relay_only: bool,

        /// Path to an SSH-like file containing authorized Ed25519 public keys.
        #[arg(long, value_name = "FILE")]
        authorized_keys_file: Option<PathBuf>,
    },
    /// Run as client (connects to server and exposes local port)
    #[command(arg_required_else_help = true)]
    Client {
        /// Path to config file
        #[arg(short, long)]
        config: Option<PathBuf>,

        /// Load config from default location (~/.config/tunnel-rs/client.toml)
        #[arg(long)]
        default_config: bool,

        /// Read JSON config from stdin for automation/IPC (use -c for normal usage)
        #[arg(long)]
        config_stdin: bool,

        /// EndpointId of the server to connect to
        #[arg(short = 'n', long)]
        server_node_id: Option<String>,

        /// Source address to request from server (tcp://host:port or udp://host:port)
        /// The server must have this in its --allowed-tcp or --allowed-udp list
        #[arg(short, long)]
        source: Option<String>,

        /// Local address to listen on (e.g., 127.0.0.1:2222)
        #[arg(short, long)]
        target: Option<String>,

        /// Custom relay server URL(s) for failover
        #[arg(long = "relay-url")]
        relay_urls: Vec<String>,

        /// Shared bearer token for the custom relay(s) (requires --relay-url).
        /// Prefer TUNNEL_RS_RELAY_AUTH_TOKEN or config to keep it out of the process list.
        #[arg(long, value_name = "TOKEN")]
        relay_auth_token: Option<String>,

        /// Force all connections through the relay server (disables direct P2P).
        #[arg(long)]
        relay_only: bool,

        /// Path to the compact tunnel-rs Ed25519 private key used for authentication.
        #[arg(long)]
        private_key_file: Option<PathBuf>,
    },
    /// Generate a server private key for persistent identity
    ///
    /// The private key gives the server a stable EndpointId that clients connect to.
    /// Use show-server-id to display the public EndpointId derived from this key.
    GenerateServerKey {
        /// Path where to save the private key file
        #[arg(short, long, required_unless_present = "json", conflicts_with = "json")]
        output: Option<PathBuf>,

        /// Overwrite existing file if it exists
        #[arg(long, requires = "output")]
        force: bool,

        /// Print the public and private keys as JSON instead of saving a file
        #[arg(long)]
        json: bool,
    },
    /// Show the server's public EndpointId derived from a private key
    ///
    /// Clients use this EndpointId with --server-node-id to connect.
    ShowServerId {
        /// Path to the private key file
        #[arg(short, long)]
        secret_file: PathBuf,
    },
    /// Generate a compact Ed25519 client authentication key.
    ///
    /// Without --output the private key file is written to stdout and the
    /// authorized-key entry to stderr. With --output the private key is saved
    /// to that path (0600 on Unix) and the authorized-key entry is printed to
    /// stdout.
    GenerateAuthKey {
        /// Path where to save the compact base64 private key ("-" means stdout).
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Comment appended to the printed authorized-key entry.
        #[arg(short, long, default_value = "")]
        comment: String,

        /// Overwrite an existing private key file.
        #[arg(long, requires = "output")]
        force: bool,
    },
}

fn env_var_opt(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn normalize_optional_endpoint(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Resolved parameters for iroh server mode.
/// CLI values take precedence over config file values.
struct ServerIrohParams {
    allowed_tcp: Vec<String>,
    allowed_udp: Vec<String>,
    max_sessions: Option<usize>,
    secret: Option<String>,
    secret_file: Option<PathBuf>,
    relay_urls: Vec<String>,
    relay_auth_token: Option<String>,
    authorized_keys_file: Option<PathBuf>,
    transport: TransportTuning,
}

/// Resolve iroh server parameters from CLI and config.
/// Env vars take precedence over config for sensitive fields.
fn resolve_server_iroh_params(
    cli: &Command,
    iroh_cfg: Option<&crate::config::IrohConfig>,
) -> ServerIrohParams {
    let cfg = iroh_cfg.cloned().unwrap_or_default();
    let cfg_allowed = cfg.allowed_sources.clone().unwrap_or_default();

    let Command::Server {
        allowed_tcp,
        allowed_udp,
        max_sessions,
        secret_file,
        relay_urls,
        relay_auth_token,
        authorized_keys_file,
        ..
    } = cli
    else {
        unreachable!("resolve_server_iroh_params called with non-server command");
    };

    let env_secret = env_var_opt("TUNNEL_RS_SECRET");
    let (secret, secret_file) = if env_secret.is_some() || secret_file.is_some() {
        (env_secret, secret_file.clone())
    } else {
        (cfg.secret.clone(), cfg.secret_file.clone())
    };

    ServerIrohParams {
        allowed_tcp: if allowed_tcp.is_empty() {
            cfg_allowed.tcp.clone()
        } else {
            allowed_tcp.clone()
        },
        allowed_udp: if allowed_udp.is_empty() {
            cfg_allowed.udp.clone()
        } else {
            allowed_udp.clone()
        },
        max_sessions: max_sessions.or(cfg.max_sessions),
        secret,
        secret_file,
        relay_urls: if relay_urls.is_empty() {
            cfg.relay_urls.clone().unwrap_or_default()
        } else {
            relay_urls.clone()
        },
        relay_auth_token: relay_auth_token
            .clone()
            .or_else(|| env_var_opt("TUNNEL_RS_RELAY_AUTH_TOKEN"))
            .or(cfg.relay_auth_token.clone()),
        authorized_keys_file: authorized_keys_file
            .clone()
            .or_else(|| env_var_opt("TUNNEL_RS_AUTHORIZED_KEYS_FILE").map(PathBuf::from))
            .or(cfg.authorized_keys_file.clone()),
        transport: cfg.transport.clone(),
    }
}

/// Resolved parameters for iroh client mode.
/// CLI values take precedence over config file values.
struct ClientIrohParams {
    server_node_id: Option<String>,
    source: Option<String>,
    target: Option<String>,
    relay_urls: Vec<String>,
    relay_auth_token: Option<String>,
    private_key_file: Option<PathBuf>,
    transport: TransportTuning,
}

/// Resolve iroh client parameters from CLI and config.
/// Env vars take precedence over config for sensitive fields.
fn resolve_client_iroh_params(
    cli: &Command,
    iroh_cfg: Option<&crate::config::IrohConfig>,
) -> ClientIrohParams {
    let cfg = iroh_cfg.cloned().unwrap_or_default();

    let Command::Client {
        server_node_id,
        source,
        target,
        relay_urls,
        relay_auth_token,
        private_key_file,
        ..
    } = cli
    else {
        unreachable!("resolve_client_iroh_params called with non-client command");
    };

    ClientIrohParams {
        server_node_id: server_node_id.clone().or(cfg.server_node_id.clone()),
        source: normalize_optional_endpoint(source.clone())
            .or_else(|| normalize_optional_endpoint(cfg.request_source.clone())),
        target: target.clone().or(cfg.target.clone()),
        relay_urls: if relay_urls.is_empty() {
            cfg.relay_urls.clone().unwrap_or_default()
        } else {
            relay_urls.clone()
        },
        relay_auth_token: relay_auth_token
            .clone()
            .or_else(|| env_var_opt("TUNNEL_RS_RELAY_AUTH_TOKEN"))
            .or(cfg.relay_auth_token.clone()),
        private_key_file: private_key_file
            .clone()
            .or_else(|| env_var_opt("TUNNEL_RS_PRIVATE_KEY_FILE").map(PathBuf::from))
            .or(cfg.private_key_file.clone()),
        transport: cfg.transport.clone(),
    }
}

fn resolve_iroh_secret(secret: Option<String>, secret_file: Option<PathBuf>) -> Result<SecretKey> {
    match (secret, secret_file) {
        (Some(_), Some(_)) => {
            anyhow::bail!(
                "Cannot combine TUNNEL_RS_SECRET with --secret-file (or secret and secret_file in config)."
            );
        }
        (Some(secret), None) => {
            let trimmed = secret.trim();
            if trimmed.is_empty() {
                anyhow::bail!("Inline secret is empty. Provide a base64-encoded secret key.");
            }
            let secret = load_secret_from_string(trimmed)
                .context("Invalid inline secret key (expected base64)")?;
            let endpoint_id = secret_to_endpoint_id(&secret);
            log::info!("Loaded identity from inline secret");
            log::info!("EndpointId: {}", endpoint_id);
            Ok(secret)
        }
        (None, Some(path)) => {
            let expanded = expand_tilde(&path);
            let secret = load_secret(&expanded)?;
            let endpoint_id = secret_to_endpoint_id(&secret);
            log::info!("Loaded identity from: {}", expanded.display());
            log::info!("EndpointId: {}", endpoint_id);
            Ok(secret)
        }
        (None, None) => {
            anyhow::bail!(
                "Server identity is required. Generate a key with:\n\
                 tunnel-rs generate-server-key --output ./server.key\n\
                 Then pass --secret-file ./server.key or set [iroh].secret_file in server.toml."
            );
        }
    }
}

/// Load server config based on flags. Returns (config, source).
async fn resolve_server_config(
    config: Option<PathBuf>,
    default_config: bool,
    config_stdin: bool,
) -> Result<(ServerConfig, ConfigSource)> {
    let source_count = config.is_some() as u8 + default_config as u8 + config_stdin as u8;
    if source_count > 1 {
        anyhow::bail!(
            "Only one of -c/--config, --default-config, or --config-stdin may be used"
        );
    }

    if config_stdin {
        Ok((parse_config_from_reader(std::io::stdin()).await?, ConfigSource::Stdin))
    } else if let Some(path) = config {
        Ok((load_server_config(Some(&path))?, ConfigSource::File))
    } else if default_config {
        Ok((load_server_config(None)?, ConfigSource::File))
    } else {
        Ok((ServerConfig::default(), ConfigSource::None))
    }
}

/// Load client config based on flags. Returns (config, source).
async fn resolve_client_config(
    config: Option<PathBuf>,
    default_config: bool,
    config_stdin: bool,
) -> Result<(ClientConfig, ConfigSource)> {
    let source_count =
        config.is_some() as u8 + default_config as u8 + config_stdin as u8;
    if source_count > 1 {
        anyhow::bail!(
            "Only one of -c/--config, --default-config, or --config-stdin may be used"
        );
    }

    if config_stdin {
        Ok((parse_config_from_reader(std::io::stdin()).await?, ConfigSource::Stdin))
    } else if let Some(path) = config {
        Ok((load_client_config(Some(&path))?, ConfigSource::File))
    } else if default_config {
        Ok((load_client_config(None)?, ConfigSource::File))
    } else {
        Ok((ClientConfig::default(), ConfigSource::None))
    }
}

#[tokio::main]
async fn main() {
    std::process::exit(run().await);
}

async fn run() -> i32 {
    match run_inner().await {
        Ok(()) => 0,
        Err(err) => {
            let code = err
                .downcast_ref::<TunnelError>()
                .map(|e| match e.category {
                    ErrorCategory::Config => 2,
                    ErrorCategory::Auth => 3,
                    ErrorCategory::Connection => 10,
                    ErrorCategory::ConnectionLost => 11,
                })
                .unwrap_or(1);
            eprintln!("Error: {:#}", err);
            code
        }
    }
}

async fn run_inner() -> Result<()> {
    let _ = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or(DEFAULT_LOG_FILTER),
    )
    .try_init();

    let args = Args::parse();
    let command = args.command;

    match &command {
        Command::Server {
            config,
            default_config,
            config_stdin,
            relay_only,
            ..
        } => {
            let (cfg, source) =
                resolve_server_config(config.clone(), *default_config, *config_stdin).await?;
            if source != ConfigSource::None {
                cfg.validate(source)?;
            }

            let ServerIrohParams {
                allowed_tcp,
                allowed_udp,
                max_sessions,
                secret,
                secret_file,
                relay_urls,
                relay_auth_token,
                authorized_keys_file,
                transport,
            } = resolve_server_iroh_params(&command, cfg.iroh());
            let relay_config = RelayConfig::from_urls_with_token(&relay_urls, relay_auth_token)
                .map_err(TunnelError::config)?;
            let secret = resolve_iroh_secret(secret, secret_file)?;
            let authorized_keys_file = authorized_keys_file.ok_or_else(|| {
                TunnelError::config(anyhow::anyhow!(
                    "Authentication requires --authorized-keys-file, \
                     TUNNEL_RS_AUTHORIZED_KEYS_FILE, or [iroh].authorized_keys_file."
                ))
            })?;
            let authorized_keys_file = expand_tilde(&authorized_keys_file);
            let authorized_keys = auth::load_authorized_keys(&authorized_keys_file)
                .map_err(TunnelError::config)?;
            if authorized_keys.is_empty() {
                return Err(TunnelError::config(anyhow::anyhow!(
                    "No Ed25519 public keys found in {}",
                    authorized_keys_file.display()
                )).into());
            }

            log::info!("Authorized client keys: {}", authorized_keys.len());
            validate_transport_tuning(&transport, "iroh.transport")?;

            iroh_mode::run_multi_source_server(iroh_mode::MultiSourceServerConfig {
                allowed_tcp,
                allowed_udp,
                max_sessions,
                secret: Some(secret),
                relay_config,
                relay_only: *relay_only,
                authorized_keys,
                transport,
            })
            .await
        }
        Command::Client {
            config,
            default_config,
            config_stdin,
            relay_only,
            ..
        } => {
            let (cfg, source) =
                resolve_client_config(config.clone(), *default_config, *config_stdin).await?;
            if source != ConfigSource::None {
                cfg.validate(source)?;
            }

            let ClientIrohParams {
                server_node_id,
                source,
                target,
                relay_urls,
                relay_auth_token,
                private_key_file,
                transport,
            } = resolve_client_iroh_params(&command, cfg.iroh());
            let relay_config = RelayConfig::from_urls_with_token(&relay_urls, relay_auth_token)
                .map_err(TunnelError::config)?;
            let server_node_id = server_node_id.ok_or_else(|| TunnelError::config(
                anyhow::anyhow!("server_node_id is required. Provide via --server-node-id or in config file."),
            ))?;
            let source = source.ok_or_else(|| TunnelError::config(
                anyhow::anyhow!("--source is required for iroh client mode. Specify the source to request from server (e.g., --source tcp://127.0.0.1:22)"),
            ))?;
            let target = target.ok_or_else(|| TunnelError::config(
                anyhow::anyhow!("--target is required. Provide the local address to listen on (e.g., --target 127.0.0.1:2222)"),
            ))?;
            let private_key_file = private_key_file.ok_or_else(|| {
                TunnelError::config(anyhow::anyhow!(
                    "Authentication requires --private-key-file, \
                     TUNNEL_RS_PRIVATE_KEY_FILE, or [iroh].private_key_file."
                ))
            })?;
            let private_key = auth::load_private_key(&expand_tilde(&private_key_file))
                .map_err(TunnelError::config)?;

            validate_transport_tuning(&transport, "iroh.transport")
                .map_err(TunnelError::config)?;

            iroh_mode::run_multi_source_client(iroh_mode::MultiSourceClientConfig {
                node_id: server_node_id,
                source,
                target,
                relay_config,
                relay_only: *relay_only,
                private_key,
                transport,
            })
            .await
        }
        Command::GenerateServerKey {
            output,
            force,
            json,
        } => {
            if *json {
                secret::generate_secret_json()
            } else {
                secret::generate_secret(expand_tilde(
                    output.as_ref().expect("clap requires --output without --json"),
                ), *force)
            }
        }
        Command::ShowServerId { secret_file } => secret::show_id(expand_tilde(secret_file)),
        Command::GenerateAuthKey {
            output,
            comment,
            force,
        } => {
            let output = output.as_deref().map(expand_tilde);
            auth::generate_auth_key(output.as_deref(), comment, *force)
        }
    }
}
