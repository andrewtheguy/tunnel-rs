//! tunnel-rs
//!
//! Forwards TCP or UDP traffic through iroh P2P connections.

mod auth;
mod buffer;
mod config;
mod encryption;
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
    load_secret, load_secret_from_string, secret_to_endpoint_id,
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

        /// Pkarr discovery server URL, or "none" to disable internet discovery.
        /// With custom relay URLs configured, internet discovery is disabled
        /// automatically unless a discovery server is set explicitly.
        /// mDNS for local network discovery is unaffected.
        #[arg(long)]
        discovery: Option<String>,

        /// Force all connections through the relay server (disables direct P2P).
        #[arg(long)]
        relay_only: bool,

        /// Path to file containing authentication tokens (one per line, # comments allowed).
        #[arg(long, value_name = "FILE")]
        auth_tokens_file: Option<PathBuf>,

        /// Path to age identity file for decrypting age-encrypted config values
        #[arg(long)]
        encryption_key_file: Option<PathBuf>,
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

        /// Pkarr discovery server URL, or "none" to disable internet discovery.
        /// With custom relay URLs configured, internet discovery is disabled
        /// automatically unless a discovery server is set explicitly.
        /// mDNS for local network discovery is unaffected.
        #[arg(long)]
        discovery: Option<String>,

        /// Force all connections through the relay server (disables direct P2P).
        #[arg(long)]
        relay_only: bool,

        /// Path to file containing authentication token
        #[arg(long)]
        auth_token_file: Option<PathBuf>,

        /// Path to age identity file for decrypting age-encrypted config values
        #[arg(long)]
        encryption_key_file: Option<PathBuf>,
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
    /// Generate a client authentication token
    ///
    /// Tokens are shared with clients for authentication (like API keys).
    /// Server configures accepted tokens via TUNNEL_RS_AUTH_TOKENS env var or --auth-tokens-file.
    GenerateAuthToken {
        /// Number of tokens to generate (default: 1)
        #[arg(short, long, default_value = "1")]
        count: usize,

        /// Print the generated token(s) as JSON
        #[arg(long)]
        json: bool,
    },
    /// Age encryption commands for config file secrets
    ConfigEncryption {
        #[command(subcommand)]
        action: ConfigEncryptionCommand,
    },
}

#[derive(Subcommand)]
enum ConfigEncryptionCommand {
    /// Generate an age encryption keypair
    ///
    /// Without --output, prints both keys to stdout. With --output, saves the
    /// private key to a file and prints the public key (recipient) to stdout.
    GenerateKey {
        /// Path where to save the age identity (private key) file
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Overwrite existing file if it exists (requires --output)
        #[arg(long, requires = "output")]
        force: bool,
    },
    /// Encrypt a value for use in config files (reads plaintext from stdin)
    ///
    /// Outputs an `ageenc:` prefixed single-line string that can be used directly
    /// as a TOML config value.
    EncryptValue {
        /// Age recipient (public key, starts with "age1...")
        #[arg(short, long)]
        recipient: Option<String>,

        /// Config file to read encryption_recipient from (alternative to --recipient)
        #[arg(short, long)]
        config: Option<PathBuf>,
    },
}

fn env_var_opt(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
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
    discovery: Option<String>,
    auth_tokens: Vec<String>,
    auth_tokens_file: Option<PathBuf>,
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
        discovery,
        auth_tokens_file,
        encryption_key_file: _,
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

    let env_auth_tokens: Vec<String> = env_var_opt("TUNNEL_RS_AUTH_TOKENS")
        .map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();

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
        discovery: discovery.clone().or(cfg.discovery.clone()),
        auth_tokens: if !env_auth_tokens.is_empty() {
            env_auth_tokens
        } else {
            cfg.auth_tokens.clone().unwrap_or_default()
        },
        auth_tokens_file: auth_tokens_file.clone().or(cfg.auth_tokens_file.clone()),
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
    discovery: Option<String>,
    auth_token: Option<String>,
    auth_token_file: Option<PathBuf>,
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
        discovery,
        auth_token_file,
        encryption_key_file: _,
        ..
    } = cli
    else {
        unreachable!("resolve_client_iroh_params called with non-client command");
    };

    let env_auth_token = env_var_opt("TUNNEL_RS_AUTH_TOKEN");
    let (auth_token, auth_token_file) = if env_auth_token.is_some() || auth_token_file.is_some() {
        (env_auth_token, auth_token_file.clone())
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
        discovery: discovery.clone().or(cfg.discovery.clone()),
        auth_token,
        auth_token_file,
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
            encryption_key_file,
            ..
        } => {
            let (mut cfg, source) =
                resolve_server_config(config.clone(), *default_config, *config_stdin).await?;

            if source != ConfigSource::None {
                cfg.validate(source)?;
            }

            // Decrypt age-encrypted values if present
            let enc_key = encryption_key_file
                .clone()
                .or_else(|| env_var_opt("TUNNEL_RS_ENCRYPTION_KEY_FILE").map(PathBuf::from))
                .or_else(|| {
                    cfg.iroh
                        .as_ref()
                        .and_then(|i| i.encryption_key_file.clone())
                })
                .map(|p| expand_tilde(&p));
            if let Some(ref mut iroh) = cfg.iroh {
                iroh.decrypt_secrets(enc_key.as_deref())?;
            }

            let iroh_cfg = cfg.iroh();
            let ServerIrohParams {
                allowed_tcp,
                allowed_udp,
                max_sessions,
                secret,
                secret_file,
                relay_urls,
                discovery,
                auth_tokens,
                auth_tokens_file,
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
                    "Authentication required: set TUNNEL_RS_AUTH_TOKENS environment variable or use --auth-tokens-file.\n\
                    Clients will need to provide a token via TUNNEL_RS_AUTH_TOKEN or --auth-token-file."
                );
            }

            log::info!("Auth tokens: {} token(s) configured", auth_tokens.len());

            // Validate transport tuning window sizes
            validate_transport_tuning(&transport, "iroh.transport")?;

            iroh_mode::run_multi_source_server(iroh_mode::MultiSourceServerConfig {
                allowed_tcp,
                allowed_udp,
                max_sessions,
                secret: Some(secret),
                relay_urls,
                relay_only,
                discovery,
                auth_tokens,
                transport,
            })
            .await
        }
        Command::Client {
            config,
            default_config,
            config_stdin,
            relay_only,
            encryption_key_file,
            ..
        } => {
            let (mut cfg, source) =
                resolve_client_config(config.clone(), *default_config, *config_stdin).await?;

            if source != ConfigSource::None {
                cfg.validate(source)?;
            }

            // Decrypt age-encrypted values if present
            let enc_key = encryption_key_file
                .clone()
                .or_else(|| env_var_opt("TUNNEL_RS_ENCRYPTION_KEY_FILE").map(PathBuf::from))
                .or_else(|| {
                    cfg.iroh
                        .as_ref()
                        .and_then(|i| i.encryption_key_file.clone())
                })
                .map(|p| expand_tilde(&p));
            if let Some(ref mut iroh) = cfg.iroh {
                iroh.decrypt_secrets(enc_key.as_deref())?;
            }

            let iroh_cfg = cfg.iroh();
            let ClientIrohParams {
                server_node_id,
                source,
                target,
                relay_urls,
                discovery,
                auth_token,
                auth_token_file,
                transport,
            } = resolve_client_iroh_params(&command, iroh_cfg);

            let relay_only = *relay_only;

            let server_node_id = server_node_id.ok_or_else(|| TunnelError::config(
                anyhow::anyhow!("server_node_id is required. Provide via --server-node-id or in config file."),
            ))?;
            let source = source.ok_or_else(|| TunnelError::config(
                anyhow::anyhow!("--source is required for iroh client mode. Specify the source to request from server (e.g., --source tcp://127.0.0.1:22)"),
            ))?;
            let target = target.ok_or_else(|| TunnelError::config(
                anyhow::anyhow!("--target is required. Provide the local address to listen on (e.g., --target 127.0.0.1:2222)"),
            ))?;

            // Resolve auth token from env var or file
            let auth_token = match (auth_token, auth_token_file) {
                (Some(_), Some(_)) => {
                    return Err(TunnelError::config(anyhow::anyhow!(
                        "Cannot combine TUNNEL_RS_AUTH_TOKEN with --auth-token-file (or auth_token and auth_token_file in config)."
                    )).into());
                }
                (Some(token), None) => token,
                (None, Some(file)) => {
                    let expanded = expand_tilde(&file);
                    auth::load_auth_token_from_file(&expanded)
                        .map_err(TunnelError::config)?
                }
                (None, None) => {
                    return Err(TunnelError::config(anyhow::anyhow!(
                        "Auth token is required. Set TUNNEL_RS_AUTH_TOKEN environment variable or use --auth-token-file."
                    )).into());
                }
            };

            // Validate token format before connecting (fail fast)
            auth::validate_token(&auth_token)
                .context("Invalid auth token format. Generate a valid token with: tunnel-rs generate-auth-token")
                .map_err(TunnelError::config)?;

            // Validate transport tuning window sizes
            validate_transport_tuning(&transport, "iroh.transport")
                .map_err(TunnelError::config)?;

            iroh_mode::run_multi_source_client(iroh_mode::MultiSourceClientConfig {
                node_id: server_node_id,
                source,
                target,
                relay_urls,
                relay_only,
                discovery,
                auth_token,
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
        Command::GenerateAuthToken { count, json } => {
            let tokens: Vec<String> = (0..*count).map(|_| auth::generate_token()).collect();
            if *json {
                println!(
                    "{}",
                    serde_json::to_string(&serde_json::json!({ "auth_tokens": tokens }))?
                );
            } else {
                for token in tokens {
                    println!("{}", token);
                }
            }
            Ok(())
        }
        Command::ConfigEncryption { action } => match action {
            ConfigEncryptionCommand::GenerateKey { output, force } => {
                let (secret_key, public_key) = encryption::generate_keypair();
                if let Some(path) = output {
                    let path = expand_tilde(path);
                    encryption::write_identity_file(&path, &secret_key, &public_key, *force)?;
                    log::info!("Encryption key saved to: {}", path.display());
                    println!("{}", public_key);
                } else {
                    let now = jiff::Zoned::now().strftime("%Y-%m-%dT%H:%M:%S%:z");
                    println!("# created: {}", now);
                    println!("# public key: {}", public_key);
                    println!("{}", secret_key);
                }
                Ok(())
            }
            ConfigEncryptionCommand::EncryptValue { recipient, config } => {
                let recipient_str = match (recipient, config) {
                    (Some(_), Some(_)) => {
                        anyhow::bail!(
                            "Cannot combine --recipient and --config. Use only one."
                        );
                    }
                    (Some(r), None) => r.clone(),
                    (None, Some(config_path)) => {
                        let expanded = expand_tilde(config_path);
                        let content = std::fs::read_to_string(&expanded).with_context(|| {
                            format!("Failed to read config: {}", expanded.display())
                        })?;

                        #[derive(serde::Deserialize)]
                        struct MinimalConfig {
                            iroh: Option<MinimalIroh>,
                        }
                        #[derive(serde::Deserialize)]
                        struct MinimalIroh {
                            encryption_recipient: Option<String>,
                        }

                        let cfg: MinimalConfig =
                            toml::from_str(&content).with_context(|| {
                                format!("Failed to parse config: {}", expanded.display())
                            })?;
                        cfg.iroh
                            .and_then(|i| i.encryption_recipient)
                            .ok_or_else(|| {
                                anyhow::anyhow!(
                                    "No [iroh].encryption_recipient found in {}",
                                    expanded.display()
                                )
                            })?
                    }
                    (None, None) => {
                        anyhow::bail!(
                            "Provide --recipient or --config to specify the age public key"
                        );
                    }
                };

                let mut plaintext = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut plaintext)
                    .context("Failed to read plaintext from stdin")?;
                let plaintext = plaintext.trim_end();
                if plaintext.is_empty() {
                    anyhow::bail!("No input provided on stdin");
                }

                let encrypted = encryption::encrypt_value(plaintext, &recipient_str)?;
                println!("{}", encrypted);
                Ok(())
            }
        },
    }
}
