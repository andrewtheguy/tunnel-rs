//! tunnel-rs
//!
//! Forwards TCP or UDP traffic through iroh P2P connections.


use ::iroh::SecretKey;
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use tunnel_common::config::{
    expand_tilde, load_client_config, load_server_config, parse_config_from_reader,
    validate_transport_tuning, ClientConfig, ServerConfig, TransportTuning,
};
use tunnel_iroh::iroh_mode::endpoint::{
    load_secret, load_secret_from_string, secret_to_endpoint_id,
};
use tunnel_iroh::{auth, iroh_mode, secret};

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
    Server {
        /// Path to config file
        #[arg(short, long)]
        config: Option<PathBuf>,

        /// Load config from default location (~/.config/tunnel-rs/server.toml)
        #[arg(long)]
        default_config: bool,

        /// Read JSON config from stdin for automation/IPC (file configs use TOML)
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

        /// Base64-encoded secret key for persistent identity
        #[arg(long)]
        secret: Option<String>,

        /// Path to secret key file for persistent identity
        #[arg(long)]
        secret_file: Option<PathBuf>,

        /// Custom relay server URL(s) for failover
        #[arg(long = "relay-url")]
        relay_urls: Vec<String>,

        /// Force all connections through the relay server (disables direct P2P).
        #[arg(long)]
        relay_only: bool,

        /// Custom DNS server URL for peer discovery, or "none" to disable DNS discovery.
        /// mDNS for local network discovery is unaffected.
        #[arg(long)]
        dns_server: Option<String>,

        /// Authentication tokens (repeatable). Clients must provide one of these tokens to connect.
        /// Required for authentication. Use with --auth-tokens-file for file-based config.
        #[arg(long = "auth-tokens", value_name = "TOKEN")]
        auth_tokens: Vec<String>,

        /// Path to file containing authentication tokens (one per line, # comments allowed).
        /// Can be combined with --auth-tokens for additional inline tokens.
        #[arg(long, value_name = "FILE")]
        auth_tokens_file: Option<PathBuf>,

        /// ALPN token for QUIC handshake-level filtering (14-char Base64URL with CRC16 checksum).
        /// Both server and client must use the same token.
        /// Generate with: tunnel-rs generate-token --alpn
        #[arg(long)]
        alpn_token: Option<String>,

        /// Path to file containing ALPN token (use this or --alpn-token, not both)
        #[arg(long)]
        alpn_token_file: Option<PathBuf>,
    },
    /// Run as client (connects to server and exposes local port)
    Client {
        /// Path to config file
        #[arg(short, long)]
        config: Option<PathBuf>,

        /// Load config from default location (~/.config/tunnel-rs/client.toml)
        #[arg(long)]
        default_config: bool,

        /// Read JSON config from stdin for automation/IPC (file configs use TOML)
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

        /// Force all connections through the relay server (disables direct P2P).
        #[arg(long)]
        relay_only: bool,

        /// Custom DNS server URL for peer discovery, or "none" to disable DNS discovery.
        /// mDNS for local network discovery is unaffected.
        #[arg(long)]
        dns_server: Option<String>,

        /// Authentication token to send to server
        #[arg(long)]
        auth_token: Option<String>,

        /// Path to file containing authentication token
        #[arg(long)]
        auth_token_file: Option<PathBuf>,

        /// ALPN token for QUIC handshake-level filtering (14-char Base64URL with CRC16 checksum).
        /// Must match the server's --alpn-token value.
        /// Generate with: tunnel-rs generate-token --alpn
        #[arg(long)]
        alpn_token: Option<String>,

        /// Path to file containing ALPN token (use this or --alpn-token, not both)
        #[arg(long)]
        alpn_token_file: Option<PathBuf>,
    },
    /// Generate a server private key for persistent identity
    ///
    /// The private key gives the server a stable EndpointId that clients connect to.
    /// Use show-server-id to display the public EndpointId derived from this key.
    GenerateServerKey {
        /// Path where to save the private key file
        #[arg(short, long)]
        output: PathBuf,

        /// Overwrite existing file if it exists
        #[arg(long)]
        force: bool,
    },
    /// Show the server's public EndpointId derived from a private key
    ///
    /// Clients use this EndpointId with --server-node-id to connect.
    ShowServerId {
        /// Path to the private key file
        #[arg(short, long)]
        secret_file: PathBuf,
    },
    /// Generate a client authentication token
    ///
    /// Tokens are shared with clients for authentication (like API keys).
    /// Server configures accepted tokens via --auth-tokens or --auth-tokens-file.
    GenerateToken {
        /// Number of tokens to generate (default: 1)
        #[arg(short, long, default_value = "1")]
        count: usize,

        /// Generate an ALPN token (14-char Base64URL) instead of an auth token
        #[arg(long)]
        alpn: bool,
    },
}

fn normalize_optional_endpoint(value: Option<String>) -> Option<String> {
    value.and_then(|v| if v.trim().is_empty() { None } else { Some(v) })
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
    dns_server: Option<String>,
    auth_tokens: Vec<String>,
    auth_tokens_file: Option<PathBuf>,
    alpn_token: Option<String>,
    alpn_token_file: Option<PathBuf>,
    transport: TransportTuning,
}

/// Resolve iroh server parameters from CLI and config.
/// CLI values take precedence; empty CLI vectors fall back to config.
fn resolve_server_iroh_params(
    cli: &Command,
    iroh_cfg: Option<&tunnel_common::config::IrohConfig>,
) -> ServerIrohParams {
    let cfg = iroh_cfg.cloned().unwrap_or_default();
    let cfg_allowed = cfg.allowed_sources.clone().unwrap_or_default();

    let Command::Server {
        allowed_tcp,
        allowed_udp,
        max_sessions,
        secret,
        secret_file,
        relay_urls,
        dns_server,
        auth_tokens,
        auth_tokens_file,
        alpn_token,
        alpn_token_file,
        ..
    } = cli
    else {
        unreachable!("resolve_server_iroh_params called with non-server command");
    };

    let (secret, secret_file) = if secret.is_some() || secret_file.is_some() {
        (secret.clone(), secret_file.clone())
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
        dns_server: dns_server.clone().or(cfg.dns_server.clone()),
        auth_tokens: if auth_tokens.is_empty() {
            cfg.auth_tokens.clone().unwrap_or_default()
        } else {
            auth_tokens.clone()
        },
        auth_tokens_file: auth_tokens_file.clone().or(cfg.auth_tokens_file.clone()),
        alpn_token: if alpn_token.is_some() || alpn_token_file.is_some() {
            alpn_token.clone()
        } else {
            cfg.alpn_token.clone()
        },
        alpn_token_file: if alpn_token.is_some() || alpn_token_file.is_some() {
            alpn_token_file.clone()
        } else {
            cfg.alpn_token_file.clone()
        },
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
    dns_server: Option<String>,
    auth_token: Option<String>,
    auth_token_file: Option<PathBuf>,
    alpn_token: Option<String>,
    alpn_token_file: Option<PathBuf>,
    transport: TransportTuning,
}

/// Resolve iroh client parameters from CLI and config.
/// CLI values take precedence; empty CLI vectors fall back to config.
fn resolve_client_iroh_params(
    cli: &Command,
    iroh_cfg: Option<&tunnel_common::config::IrohConfig>,
) -> ClientIrohParams {
    let cfg = iroh_cfg.cloned().unwrap_or_default();

    let Command::Client {
        server_node_id,
        source,
        target,
        relay_urls,
        dns_server,
        auth_token,
        auth_token_file,
        alpn_token,
        alpn_token_file,
        ..
    } = cli
    else {
        unreachable!("resolve_client_iroh_params called with non-client command");
    };

    let (auth_token, auth_token_file) = if auth_token.is_some() || auth_token_file.is_some() {
        (auth_token.clone(), auth_token_file.clone())
    } else {
        (cfg.auth_token.clone(), cfg.auth_token_file.clone())
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
        dns_server: dns_server.clone().or(cfg.dns_server.clone()),
        auth_token,
        auth_token_file,
        alpn_token: if alpn_token.is_some() || alpn_token_file.is_some() {
            alpn_token.clone()
        } else {
            cfg.alpn_token.clone()
        },
        alpn_token_file: if alpn_token.is_some() || alpn_token_file.is_some() {
            alpn_token_file.clone()
        } else {
            cfg.alpn_token_file.clone()
        },
        transport: cfg.transport.clone(),
    }
}

fn resolve_iroh_secret(secret: Option<String>, secret_file: Option<PathBuf>) -> Result<SecretKey> {
    match (secret, secret_file) {
        (Some(_), Some(_)) => {
            anyhow::bail!(
                "Cannot combine --secret with --secret-file (or secret and secret_file in config)."
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

/// Load server config based on flags. Returns (config, was_loaded_from_file).
fn resolve_server_config(
    config: Option<PathBuf>,
    default_config: bool,
    config_stdin: bool,
) -> Result<(ServerConfig, bool)> {
    let source_count = config.is_some() as u8 + default_config as u8 + config_stdin as u8;
    if source_count > 1 {
        anyhow::bail!(
            "Only one of -c/--config, --default-config, or --config-stdin may be used"
        );
    }

    if config_stdin {
        Ok((parse_config_from_reader(std::io::stdin())?, true))
    } else if let Some(path) = config {
        Ok((load_server_config(Some(&path))?, true))
    } else if default_config {
        Ok((load_server_config(None)?, true))
    } else {
        Ok((ServerConfig::default(), false))
    }
}

/// Load client config based on flags. Returns (config, was_loaded_from_file).
fn resolve_client_config(
    config: Option<PathBuf>,
    default_config: bool,
    config_stdin: bool,
) -> Result<(ClientConfig, bool)> {
    let source_count =
        config.is_some() as u8 + default_config as u8 + config_stdin as u8;
    if source_count > 1 {
        anyhow::bail!(
            "Only one of -c/--config, --default-config, or --config-stdin may be used"
        );
    }

    if config_stdin {
        Ok((parse_config_from_reader(std::io::stdin())?, true))
    } else if let Some(path) = config {
        Ok((load_client_config(Some(&path))?, true))
    } else if default_config {
        Ok((load_client_config(None)?, true))
    } else {
        Ok((ClientConfig::default(), false))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .filter_module("tunnel_rs", log::LevelFilter::Info)
        .filter_module("tunnel_iroh", log::LevelFilter::Info)
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
            let (cfg, from_file) =
                resolve_server_config(config.clone(), *default_config, *config_stdin)?;

            if from_file {
                cfg.validate("iroh")?;
            }

            let iroh_cfg = cfg.iroh();
            let ServerIrohParams {
                allowed_tcp,
                allowed_udp,
                max_sessions,
                secret,
                secret_file,
                relay_urls,
                dns_server,
                auth_tokens,
                auth_tokens_file,
                alpn_token,
                alpn_token_file,
                transport,
            } = resolve_server_iroh_params(&command, iroh_cfg);

            let relay_only = *relay_only;

            let secret = resolve_iroh_secret(secret, secret_file)?;

            // Load auth tokens for authentication
            let auth_tokens_file_expanded = auth_tokens_file.as_ref().map(|p| expand_tilde(p));
            let auth_tokens =
                auth::load_auth_tokens(&auth_tokens, auth_tokens_file_expanded.as_deref())?;

            if auth_tokens.is_empty() {
                anyhow::bail!(
                    "Authentication required: specify --auth-tokens or --auth-tokens-file.\n\
                    Clients will need to provide one of these tokens via --auth-token."
                );
            }

            log::info!("Auth tokens: {} token(s) configured", auth_tokens.len());

            // Resolve ALPN token from inline or file
            let alpn_token = match (alpn_token, alpn_token_file) {
                (Some(_), Some(_)) => {
                    anyhow::bail!(
                        "Cannot combine --alpn-token with --alpn-token-file (or alpn_token and alpn_token_file in config)."
                    );
                }
                (Some(token), None) => token,
                (None, Some(file)) => {
                    let expanded = expand_tilde(&file);
                    auth::load_alpn_token_from_file(&expanded)?
                }
                (None, None) => {
                    anyhow::bail!(
                        "--alpn-token is required. Provide an ALPN token for QUIC handshake filtering.\n\
                        Generate one with: tunnel-rs generate-token --alpn"
                    );
                }
            };
            auth::validate_alpn_token(&alpn_token).context(
                "Invalid ALPN token format. Generate a valid token with: tunnel-rs generate-token --alpn",
            )?;

            // Validate transport tuning window sizes
            validate_transport_tuning(&transport, "iroh.transport")?;

            iroh_mode::run_multi_source_server(iroh_mode::MultiSourceServerConfig {
                allowed_tcp,
                allowed_udp,
                max_sessions,
                secret: Some(secret),
                relay_urls,
                relay_only,
                dns_server,
                auth_tokens,
                alpn_token,
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
            let (cfg, from_file) =
                resolve_client_config(config.clone(), *default_config, *config_stdin)?;

            if from_file {
                cfg.validate("iroh")?;
            }

            let iroh_cfg = cfg.iroh();
            let ClientIrohParams {
                server_node_id,
                source,
                target,
                relay_urls,
                dns_server,
                auth_token,
                auth_token_file,
                alpn_token,
                alpn_token_file,
                transport,
            } = resolve_client_iroh_params(&command, iroh_cfg);

            let relay_only = *relay_only;

            let server_node_id = server_node_id.context(
                "server_node_id is required. Provide via --server-node-id or in config file.",
            )?;
            let source = source.context(
                "--source is required for iroh client mode. Specify the source to request from server (e.g., --source tcp://127.0.0.1:22)",
            )?;
            let target = target.context(
                "--target is required. Provide the local address to listen on (e.g., --target 127.0.0.1:2222)",
            )?;

            // Resolve auth token from CLI or file
            let auth_token = match (auth_token, auth_token_file) {
                (Some(_), Some(_)) => {
                    anyhow::bail!(
                        "Cannot combine --auth-token with --auth-token-file (or auth_token and auth_token_file in config)."
                    );
                }
                (Some(token), None) => token,
                (None, Some(file)) => {
                    let expanded = expand_tilde(&file);
                    auth::load_auth_token_from_file(&expanded)?
                }
                (None, None) => {
                    anyhow::bail!(
                        "--auth-token is required. Provide an authentication token to connect to the server."
                    );
                }
            };

            // Validate token format before connecting (fail fast)
            auth::validate_token(&auth_token).context(
                "Invalid auth token format. Generate a valid token with: tunnel-rs generate-token",
            )?;

            // Resolve ALPN token from inline or file
            let alpn_token = match (alpn_token, alpn_token_file) {
                (Some(_), Some(_)) => {
                    anyhow::bail!(
                        "Cannot combine --alpn-token with --alpn-token-file (or alpn_token and alpn_token_file in config)."
                    );
                }
                (Some(token), None) => token,
                (None, Some(file)) => {
                    let expanded = expand_tilde(&file);
                    auth::load_alpn_token_from_file(&expanded)?
                }
                (None, None) => {
                    anyhow::bail!(
                        "--alpn-token is required. Provide the ALPN token shared by the server.\n\
                        Generate one with: tunnel-rs generate-token --alpn"
                    );
                }
            };
            auth::validate_alpn_token(&alpn_token).context(
                "Invalid ALPN token format. Generate a valid token with: tunnel-rs generate-token --alpn",
            )?;

            // Validate transport tuning window sizes
            validate_transport_tuning(&transport, "iroh.transport")?;

            iroh_mode::run_multi_source_client(iroh_mode::MultiSourceClientConfig {
                node_id: server_node_id,
                source,
                target,
                relay_urls,
                relay_only,
                dns_server,
                auth_token,
                alpn_token,
                transport,
            })
            .await
        }
        Command::GenerateServerKey { output, force } => {
            secret::generate_secret(expand_tilde(output), *force)
        }
        Command::ShowServerId { secret_file } => secret::show_id(expand_tilde(secret_file)),
        Command::GenerateToken { count, alpn } => {
            for _ in 0..*count {
                if *alpn {
                    println!("{}", auth::generate_alpn_token());
                } else {
                    println!("{}", auth::generate_token());
                }
            }
            Ok(())
        }
    }
}
