//! tunnel-rs-ice (ICE-only)
//!
//! Forwards TCP or UDP traffic through ICE/QUIC connections.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;

use tunnel_common::config::{
    expand_tilde, load_client_config, load_server_config, ClientConfig, ServerConfig,
};
use tunnel_common::net::resolve_listen_addr;
use tunnel_ice::defaults::{default_ice_transport_tuning, default_nostr_relays, default_stun_servers};
use tunnel_ice::{custom, nostr, secret};

#[derive(Clone, Copy, ValueEnum, Default, Debug, PartialEq)]
pub enum Protocol {
    /// TCP tunneling
    #[default]
    Tcp,
    /// UDP tunneling
    Udp,
}

impl Protocol {
    fn from_str_opt(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "tcp" => Some(Protocol::Tcp),
            "udp" => Some(Protocol::Udp),
            _ => None,
        }
    }
}

fn parse_endpoint(value: &str) -> Result<(Protocol, String)> {
    let (scheme, addr) = value
        .split_once("://")
        .context("Expected endpoint in the form tcp://host:port or udp://host:port")?;
    if addr.is_empty() {
        anyhow::bail!("Endpoint is missing host:port (got '{}')", value);
    }
    if addr.contains('/') {
        anyhow::bail!("Endpoint must not include a path (got '{}')", value);
    }
    let protocol = Protocol::from_str_opt(scheme)
        .context("Invalid scheme. Use tcp://host:port or udp://host:port")?;
    Ok((protocol, addr.to_string()))
}

fn normalize_optional_endpoint(value: Option<String>) -> Option<String> {
    value.and_then(|v| if v.trim().is_empty() { None } else { Some(v) })
}

// STUN precedence: --no-stun disables STUN entirely (CLI only). Otherwise, CLI list wins;
// config list wins next (even if empty, which disables STUN); if nothing specified, fall
// back to default public STUN servers.
fn resolve_stun_servers(
    cli_stun_servers: &[String],
    config_stun_servers: Option<Vec<String>>,
    no_stun: bool,
) -> Result<Vec<String>> {
    if no_stun {
        if !cli_stun_servers.is_empty() {
            anyhow::bail!("Cannot combine --no-stun with --stun-server");
        }
        return Ok(Vec::new());
    }
    if !cli_stun_servers.is_empty() {
        return Ok(cli_stun_servers.to_vec());
    }
    if let Some(servers) = config_stun_servers {
        return Ok(servers);
    }
    Ok(default_stun_servers())
}

fn resolve_nostr_nsec(nsec: Option<String>, nsec_file: Option<PathBuf>) -> Result<Option<String>> {
    match (nsec, nsec_file) {
        (Some(_), Some(_)) => {
            anyhow::bail!(
                "Cannot combine --nsec with --nsec-file (or nsec and nsec_file in config)."
            );
        }
        (Some(nsec), None) => {
            let trimmed = nsec.trim();
            if trimmed.is_empty() {
                anyhow::bail!("nsec is empty. Provide a valid nsec or hex private key.");
            }
            log::info!("Loaded nsec from inline value");
            Ok(Some(trimmed.to_string()))
        }
        (None, Some(path)) => {
            let expanded = expand_tilde(&path);
            let content = fs::read_to_string(&expanded)
                .with_context(|| format!("Failed to read nsec file: {}", expanded.display()))?;
            let trimmed = content.trim();
            if trimmed.is_empty() {
                anyhow::bail!("nsec file is empty: {}", expanded.display());
            }
            log::info!("Loaded nsec from file: {}", expanded.display());
            Ok(Some(trimmed.to_string()))
        }
        (None, None) => Ok(None),
    }
}

#[derive(Parser)]
#[command(name = "tunnel-rs-ice")]
#[command(version)]
#[command(about = "Forward TCP/UDP traffic through ICE/QUIC connections")]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run as server (accepts connections and forwards to source)
    #[command(
        subcommand_negates_reqs = true,
        subcommand_required = false,
        after_help = "Examples:\n  tunnel-rs-ice server -c server_ice.toml\n  tunnel-rs-ice server manual --allowed-tcp 127.0.0.0/8\n  tunnel-rs-ice server -c server_ice.toml manual\n\nNotes:\n  - --config/--default-config can appear before or after the subcommand.\n"
    )]
    Server {
        /// Path to config file
        #[arg(short, long, global = true)]
        config: Option<PathBuf>,

        /// Load config from default location (~/.config/tunnel-rs/server_ice.toml)
        #[arg(long, global = true)]
        default_config: bool,

        #[command(subcommand)]
        mode: Option<ServerMode>,
    },
    /// Run as client (connects to server and exposes local port)
    #[command(
        subcommand_negates_reqs = true,
        subcommand_required = false,
        after_help = "Examples:\n  tunnel-rs-ice client -c client_ice.toml\n  tunnel-rs-ice client manual --source tcp://127.0.0.1:5000 --target 127.0.0.1:6000\n  tunnel-rs-ice client -c client_ice.toml manual --source tcp://127.0.0.1:5000 --target 127.0.0.1:6000\n\nNotes:\n  - --config/--default-config can appear before or after the subcommand.\n  - Manual mode prints an OFFER; paste the server's ANSWER to continue.\n"
    )]
    Client {
        /// Path to config file
        #[arg(short, long, global = true)]
        config: Option<PathBuf>,

        /// Load config from default location (~/.config/tunnel-rs/client_ice.toml)
        #[arg(long, global = true)]
        default_config: bool,

        #[command(subcommand)]
        mode: Option<ClientMode>,
    },
    /// Show the Nostr public key (npub) for an existing nsec file
    ShowNpub {
        /// Path to the nsec key file
        #[arg(short, long)]
        nsec_file: PathBuf,
    },
    /// Generate a Nostr keypair for use with nostr mode
    GenerateNostrKey {
        /// Path where to save the nsec key file
        #[arg(short, long)]
        output: PathBuf,

        /// Overwrite existing file if it exists
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum ServerMode {
    /// Client-initiated mode: Full ICE with manual signaling (str0m+quinn)
    #[command(
        name = "manual",
        after_help = "Manual mode uses copy/paste signaling:\n  1) Client prints an OFFER\n  2) Paste OFFER into server\n  3) Server prints an ANSWER\n  4) Paste ANSWER into client"
    )]
    CustomManual {
        /// Allowed TCP source networks in CIDR notation (repeatable)
        /// E.g., --allowed-tcp 127.0.0.0/8 --allowed-tcp 192.168.0.0/16
        #[arg(long = "allowed-tcp")]
        allowed_tcp: Vec<String>,

        /// Allowed UDP source networks in CIDR notation (repeatable)
        /// E.g., --allowed-udp 10.0.0.0/8 --allowed-udp ::1/128
        #[arg(long = "allowed-udp")]
        allowed_udp: Vec<String>,

        /// STUN server (repeatable, e.g., stun.l.google.com:19302)
        #[arg(long = "stun-server")]
        stun_servers: Vec<String>,

        /// Disable STUN (no external infrastructure)
        #[arg(long)]
        no_stun: bool,
    },
    /// Full ICE with Nostr-based signaling (static-key identity)
    #[command(name = "nostr")]
    Nostr {
        /// Allowed TCP source networks in CIDR notation (repeatable)
        /// E.g., --allowed-tcp 127.0.0.0/8 --allowed-tcp 192.168.0.0/16
        #[arg(long = "allowed-tcp")]
        allowed_tcp: Vec<String>,

        /// Allowed UDP source networks in CIDR notation (repeatable)
        /// E.g., --allowed-udp 10.0.0.0/8 --allowed-udp ::1/128
        #[arg(long = "allowed-udp")]
        allowed_udp: Vec<String>,

        /// Your Nostr private key (nsec or hex format)
        #[arg(long)]
        nsec: Option<String>,

        /// Path to file containing your Nostr private key (nsec or hex format)
        #[arg(long)]
        nsec_file: Option<PathBuf>,

        /// Peer's Nostr public key (npub or hex format)
        #[arg(long)]
        peer_npub: Option<String>,

        /// Nostr relay URL (repeatable)
        #[arg(long = "relay")]
        relays: Vec<String>,

        /// STUN server (repeatable, e.g., stun.l.google.com:19302)
        #[arg(long = "stun-server")]
        stun_servers: Vec<String>,

        /// Disable STUN (no external infrastructure)
        #[arg(long)]
        no_stun: bool,

        /// Interval in seconds to re-publish offer while waiting for answer (default: 10)
        #[arg(long, default_value = "10")]
        republish_interval: u64,

        /// Maximum time in seconds to wait for answer before giving up (default: 120)
        #[arg(long, default_value = "120")]
        max_wait: u64,

        /// Maximum concurrent sessions (0 = unlimited, default: 10)
        #[arg(long, default_value = "10")]
        max_sessions: usize,
    },
}

#[derive(Subcommand)]
enum ClientMode {
    /// Client-initiated mode: Full ICE with manual signaling (str0m+quinn)
    #[command(
        name = "manual",
        after_help = "Manual mode uses copy/paste signaling:\n  1) Client prints an OFFER\n  2) Paste OFFER into server\n  3) Server prints an ANSWER\n  4) Paste ANSWER into client"
    )]
    CustomManual {
        /// Source address to request from server (tcp://host:port or udp://host:port)
        /// The server must have this in its --allowed-tcp or --allowed-udp list
        #[arg(short, long)]
        source: Option<String>,

        /// Local address to listen on (e.g., 127.0.0.1:2222)
        #[arg(short, long)]
        target: Option<String>,

        /// STUN server (repeatable, e.g., stun.l.google.com:19302)
        #[arg(long = "stun-server")]
        stun_servers: Vec<String>,

        /// Disable STUN (no external infrastructure)
        #[arg(long)]
        no_stun: bool,
    },
    /// Full ICE with Nostr-based signaling (static-key identity)
    #[command(name = "nostr")]
    Nostr {
        /// Local address to listen on (e.g., 127.0.0.1:2222 or tcp://127.0.0.1:2222)
        #[arg(short, long)]
        target: Option<String>,

        /// Source address to request from server (tcp://host:port or udp://host:port)
        /// The server must have this in its --allowed-tcp or --allowed-udp list
        #[arg(short, long)]
        source: Option<String>,

        /// STUN server (repeatable, e.g., stun.l.google.com:19302)
        #[arg(long = "stun-server")]
        stun_servers: Vec<String>,

        /// Disable STUN (no external infrastructure)
        #[arg(long)]
        no_stun: bool,

        /// Your Nostr private key (nsec or hex format)
        #[arg(long)]
        nsec: Option<String>,

        /// Path to file containing your Nostr private key (nsec or hex format)
        #[arg(long)]
        nsec_file: Option<PathBuf>,

        /// Peer's Nostr public key (npub or hex format)
        #[arg(long)]
        peer_npub: Option<String>,

        /// Nostr relay URL (repeatable)
        #[arg(long = "relay")]
        relays: Vec<String>,

        /// Interval in seconds to re-publish offer while waiting for an answer (default: 5)
        #[arg(long, default_value = "5")]
        republish_interval: u64,

        /// Maximum time in seconds to wait for an answer before giving up (default: 120)
        #[arg(long, default_value = "120")]
        max_wait: u64,
    },
}

/// Load server config based on flags. Returns (config, was_loaded_from_file).
fn resolve_server_config(
    config: Option<PathBuf>,
    default_config: bool,
) -> Result<(ServerConfig, bool)> {
    if config.is_some() && default_config {
        anyhow::bail!("Cannot use both -c/--config and --default-config");
    }

    if let Some(path) = config {
        Ok((load_server_config(Some(&path))?, true))
    } else if default_config {
        let path = dirs::home_dir()
            .map(|home| {
                home.join(".config")
                    .join("tunnel-rs")
                    .join("server_ice.toml")
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Could not find default config path. Use -c to specify a config file."
                )
            })?;
        Ok((load_server_config(Some(&path))?, true))
    } else {
        Ok((ServerConfig::default(), false))
    }
}

/// Load client config based on flags. Returns (config, was_loaded_from_file).
fn resolve_client_config(
    config: Option<PathBuf>,
    default_config: bool,
) -> Result<(ClientConfig, bool)> {
    if config.is_some() && default_config {
        anyhow::bail!("Cannot use both -c/--config and --default-config");
    }

    if let Some(path) = config {
        Ok((load_client_config(Some(&path))?, true))
    } else if default_config {
        let path = dirs::home_dir()
            .map(|home| {
                home.join(".config")
                    .join("tunnel-rs")
                    .join("client_ice.toml")
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Could not find default config path. Use -c to specify a config file."
                )
            })?;
        Ok((load_client_config(Some(&path))?, true))
    } else {
        Ok((ClientConfig::default(), false))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .filter_module("tunnel_rs_ice", log::LevelFilter::Info)
        .filter_module("tunnel_ice", log::LevelFilter::Info)
        .try_init();
    let args = Args::parse();

    match args.command {
        Command::Server {
            config,
            default_config,
            mode,
        } => {
            let (cfg, from_file) = resolve_server_config(config.clone(), default_config)?;

            // Determine effective mode: CLI mode takes precedence, else read from config
            let effective_mode = match (&mode, &cfg.mode) {
                (Some(_), _) => mode.as_ref().map(|m| match m {
                    ServerMode::CustomManual { .. } => "manual",
                    ServerMode::Nostr { .. } => "nostr",
                }),
                (None, Some(m)) => Some(m.as_str()),
                (None, None) => None,
            };

            let effective_mode = effective_mode.context(
                "No mode specified. Either use a subcommand (manual, nostr) or provide a config file with 'mode' field.",
            )?;

            if from_file {
                cfg.validate(effective_mode)?;
            }

            match effective_mode {
                "manual" => {
                    let custom_cfg = cfg.manual.as_ref();
                    let transport = custom_cfg
                        .map(|c| c.transport.clone())
                        .unwrap_or_else(default_ice_transport_tuning);
                    let (allowed_tcp, allowed_udp, stun_servers) = match &mode {
                        Some(ServerMode::CustomManual {
                            allowed_tcp: at,
                            allowed_udp: au,
                            stun_servers: ss,
                            no_stun,
                        }) => {
                            let cfg_allowed = custom_cfg
                                .and_then(|c| c.allowed_sources.clone())
                                .unwrap_or_default();
                            (
                                if at.is_empty() {
                                    cfg_allowed.tcp.clone()
                                } else {
                                    at.clone()
                                },
                                if au.is_empty() {
                                    cfg_allowed.udp.clone()
                                } else {
                                    au.clone()
                                },
                                resolve_stun_servers(
                                    ss,
                                    custom_cfg.and_then(|c| c.stun_servers.clone()),
                                    *no_stun,
                                )?,
                            )
                        }
                        _ => {
                            let cfg_allowed = custom_cfg
                                .and_then(|c| c.allowed_sources.clone())
                                .unwrap_or_default();
                            (
                                cfg_allowed.tcp.clone(),
                                cfg_allowed.udp.clone(),
                                resolve_stun_servers(
                                    &[],
                                    custom_cfg.and_then(|c| c.stun_servers.clone()),
                                    false,
                                )?,
                            )
                        }
                    };

                    if allowed_tcp.is_empty() && allowed_udp.is_empty() {
                        anyhow::bail!("At least one of --allowed-tcp or --allowed-udp is required for custom-manual server");
                    }

                    custom::run_manual_server(allowed_tcp, allowed_udp, stun_servers, transport)
                        .await
                }
                "nostr" => {
                    let nostr_cfg = cfg.nostr();
                    let transport = nostr_cfg
                        .map(|c| c.transport.clone())
                        .unwrap_or_else(default_ice_transport_tuning);
                    let (
                        allowed_tcp,
                        allowed_udp,
                        stun_servers,
                        nsec,
                        nsec_file,
                        peer_npub,
                        relays,
                        republish_interval,
                        max_wait,
                        max_sessions,
                    ) = match &mode {
                        Some(ServerMode::Nostr {
                            allowed_tcp: at,
                            allowed_udp: au,
                            stun_servers: ss,
                            no_stun,
                            nsec: n,
                            nsec_file: nf,
                            peer_npub: p,
                            relays: r,
                            republish_interval: ri,
                            max_wait: mw,
                            max_sessions: ms,
                        }) => {
                            let cfg_allowed = nostr_cfg
                                .and_then(|c| c.allowed_sources.clone())
                                .unwrap_or_default();
                            let cfg_nsec = nostr_cfg.and_then(|c| c.nsec.clone());
                            let cfg_nsec_file = nostr_cfg.and_then(|c| c.nsec_file.clone());
                            let (nsec, nsec_file) = if n.is_some() || nf.is_some() {
                                (n.clone(), nf.clone())
                            } else {
                                (cfg_nsec, cfg_nsec_file)
                            };
                            (
                                if at.is_empty() {
                                    cfg_allowed.tcp.clone()
                                } else {
                                    at.clone()
                                },
                                if au.is_empty() {
                                    cfg_allowed.udp.clone()
                                } else {
                                    au.clone()
                                },
                                resolve_stun_servers(
                                    ss,
                                    nostr_cfg.and_then(|c| c.stun_servers.clone()),
                                    *no_stun,
                                )?,
                                nsec,
                                nsec_file,
                                p.clone()
                                    .or_else(|| nostr_cfg.and_then(|c| c.peer_npub.clone())),
                                if r.is_empty() {
                                    nostr_cfg.and_then(|c| c.relays.clone()).unwrap_or_default()
                                } else {
                                    r.clone()
                                },
                                *ri,
                                *mw,
                                *ms,
                            )
                        }
                        _ => {
                            let cfg_allowed = nostr_cfg
                                .and_then(|c| c.allowed_sources.clone())
                                .unwrap_or_default();
                            (
                                cfg_allowed.tcp,
                                cfg_allowed.udp,
                                resolve_stun_servers(
                                    &[],
                                    nostr_cfg.and_then(|c| c.stun_servers.clone()),
                                    false,
                                )?,
                                nostr_cfg.and_then(|c| c.nsec.clone()),
                                nostr_cfg.and_then(|c| c.nsec_file.clone()),
                                nostr_cfg.and_then(|c| c.peer_npub.clone()),
                                nostr_cfg.and_then(|c| c.relays.clone()).unwrap_or_default(),
                                10,
                                120,
                                nostr_cfg.and_then(|c| c.max_sessions).unwrap_or(10),
                            )
                        }
                    };

                    if allowed_tcp.is_empty() && allowed_udp.is_empty() {
                        anyhow::bail!(
                            "At least one of --allowed-tcp or --allowed-udp must be specified for nostr server mode."
                        );
                    }

                    let nsec = resolve_nostr_nsec(nsec, nsec_file)?.context(
                        "nsec is required. Provide via --nsec/--nsec-file or in config file (nsec/nsec_file). Use 'tunnel-rs-ice generate-nostr-key' to create one.",
                    )?;
                    let peer_npub = peer_npub.context(
                        "peer-npub is required. Provide via --peer-npub or in config file.",
                    )?;
                    let relays = if relays.is_empty() {
                        default_nostr_relays()
                            .iter()
                            .map(|&relay| relay.to_string())
                            .collect()
                    } else {
                        relays
                    };

                    nostr::run_nostr_server(nostr::NostrServerConfig {
                        allowed_tcp,
                        allowed_udp,
                        stun_servers,
                        nsec,
                        peer_npub,
                        relays,
                        republish_interval_secs: republish_interval,
                        max_wait_secs: max_wait,
                        max_sessions,
                        transport,
                    })
                    .await
                }
                _ => anyhow::bail!("Invalid mode '{}'. Use: manual or nostr", effective_mode),
            }
        }
        Command::Client {
            config,
            default_config,
            mode,
        } => {
            let (cfg, from_file) = resolve_client_config(config, default_config)?;

            let effective_mode = match (&mode, &cfg.mode) {
                (Some(_), _) => mode.as_ref().map(|m| match m {
                    ClientMode::CustomManual { .. } => "manual",
                    ClientMode::Nostr { .. } => "nostr",
                }),
                (None, Some(m)) => Some(m.as_str()),
                (None, None) => None,
            };

            let effective_mode = effective_mode.context(
                "No mode specified. Either use a subcommand (manual, nostr) or provide a config file with 'mode' field.",
            )?;

            if from_file {
                cfg.validate(effective_mode)?;
            }

            match effective_mode {
                "manual" => {
                    let custom_cfg = cfg.manual.as_ref();
                    let transport = custom_cfg
                        .map(|c| c.transport.clone())
                        .unwrap_or_else(default_ice_transport_tuning);
                    let (source, target, stun_servers) = match &mode {
                        Some(ClientMode::CustomManual {
                            source: src,
                            target: t,
                            stun_servers: s,
                            no_stun,
                        }) => (
                            normalize_optional_endpoint(src.clone())
                                .or_else(|| custom_cfg.and_then(|c| c.request_source.clone())),
                            t.clone()
                                .or_else(|| custom_cfg.and_then(|c| c.target.clone())),
                            resolve_stun_servers(
                                s,
                                custom_cfg.and_then(|c| c.stun_servers.clone()),
                                *no_stun,
                            )?,
                        ),
                        _ => (
                            custom_cfg.and_then(|c| c.request_source.clone()),
                            custom_cfg.and_then(|c| c.target.clone()),
                            resolve_stun_servers(
                                &[],
                                custom_cfg.and_then(|c| c.stun_servers.clone()),
                                false,
                            )?,
                        ),
                    };

                    let source: String = source.context(
                        "--source is required for manual client. Specify the source to request from server (e.g., --source tcp://127.0.0.1:22)",
                    )?;
                    let target: String = target.context(
                        "--target is required. Specify local address to listen on (e.g., --target 127.0.0.1:2222)",
                    )?;

                    let _ = parse_endpoint(&source)
                        .with_context(|| format!("Invalid source '{}'. Expected format: tcp://host:port or udp://host:port", source))?;

                    let listen: SocketAddr = resolve_listen_addr(&target)
                        .await
                        .with_context(|| format!("Invalid target '{}'. Expected format: host:port (e.g., localhost:2222 or 127.0.0.1:2222)", target))?;

                    custom::run_manual_client(source, listen, stun_servers, transport).await
                }
                "nostr" => {
                    let nostr_cfg = cfg.nostr();
                    let transport = nostr_cfg
                        .map(|c| c.transport.clone())
                        .unwrap_or_else(default_ice_transport_tuning);
                    let (
                        target,
                        source,
                        stun_servers,
                        nsec,
                        nsec_file,
                        peer_npub,
                        relays,
                        republish_interval,
                        max_wait,
                    ) = match &mode {
                        Some(ClientMode::Nostr {
                            target: t,
                            source: src,
                            stun_servers: ss,
                            no_stun,
                            nsec: n,
                            nsec_file: nf,
                            peer_npub: p,
                            relays: r,
                            republish_interval: ri,
                            max_wait: mw,
                        }) => {
                            let cfg_nsec = nostr_cfg.and_then(|c| c.nsec.clone());
                            let cfg_nsec_file = nostr_cfg.and_then(|c| c.nsec_file.clone());
                            let (nsec, nsec_file) = if n.is_some() || nf.is_some() {
                                (n.clone(), nf.clone())
                            } else {
                                (cfg_nsec, cfg_nsec_file)
                            };
                            (
                                normalize_optional_endpoint(t.clone())
                                    .or_else(|| nostr_cfg.and_then(|c| c.target.clone())),
                                normalize_optional_endpoint(src.clone())
                                    .or_else(|| nostr_cfg.and_then(|c| c.request_source.clone())),
                                resolve_stun_servers(
                                    ss,
                                    nostr_cfg.and_then(|c| c.stun_servers.clone()),
                                    *no_stun,
                                )?,
                                nsec,
                                nsec_file,
                                p.clone()
                                    .or_else(|| nostr_cfg.and_then(|c| c.peer_npub.clone())),
                                if r.is_empty() {
                                    nostr_cfg.and_then(|c| c.relays.clone()).unwrap_or_default()
                                } else {
                                    r.clone()
                                },
                                *ri,
                                *mw,
                            )
                        }
                        _ => (
                            nostr_cfg.and_then(|c| c.target.clone()),
                            nostr_cfg.and_then(|c| c.request_source.clone()),
                            resolve_stun_servers(
                                &[],
                                nostr_cfg.and_then(|c| c.stun_servers.clone()),
                                false,
                            )?,
                            nostr_cfg.and_then(|c| c.nsec.clone()),
                            nostr_cfg.and_then(|c| c.nsec_file.clone()),
                            nostr_cfg.and_then(|c| c.peer_npub.clone()),
                            nostr_cfg.and_then(|c| c.relays.clone()).unwrap_or_default(),
                            5,
                            120,
                        ),
                    };

                    let target = target
                        .context("target is required. Provide via --target or in config file.")?;
                    let source = source.context(
                        "--source is required for nostr client mode. Specify the source to request from server (e.g., --source tcp://127.0.0.1:22)",
                    )?;
                    let nsec = resolve_nostr_nsec(nsec, nsec_file)?.context(
                        "nsec is required. Provide via --nsec/--nsec-file or in config file (nsec/nsec_file). Use 'tunnel-rs-ice generate-nostr-key' to create one.",
                    )?;
                    let peer_npub = peer_npub.context(
                        "peer-npub is required. Provide via --peer-npub or in config file.",
                    )?;
                    let relays = if relays.is_empty() {
                        default_nostr_relays()
                            .iter()
                            .map(|&relay| relay.to_string())
                            .collect()
                    } else {
                        relays
                    };

                    let listen = target;

                    if listen.contains("://") {
                        let (protocol, addr) = parse_endpoint(&listen)
                            .with_context(|| format!("Invalid receiver target '{}'", listen))?;
                        match protocol {
                            Protocol::Udp => {
                                nostr::run_nostr_udp_client(nostr::NostrClientConfig {
                                    listen: addr,
                                    source,
                                    stun_servers,
                                    nsec,
                                    peer_npub,
                                    relays,
                                    republish_interval_secs: republish_interval,
                                    max_wait_secs: max_wait,
                                    transport,
                                })
                                .await
                            }
                            Protocol::Tcp => {
                                nostr::run_nostr_tcp_client(nostr::NostrClientConfig {
                                    listen: addr,
                                    source,
                                    stun_servers,
                                    nsec,
                                    peer_npub,
                                    relays,
                                    republish_interval_secs: republish_interval,
                                    max_wait_secs: max_wait,
                                    transport,
                                })
                                .await
                            }
                        }
                    } else {
                        nostr::run_nostr_tcp_client(nostr::NostrClientConfig {
                            listen,
                            source,
                            stun_servers,
                            nsec,
                            peer_npub,
                            relays,
                            republish_interval_secs: republish_interval,
                            max_wait_secs: max_wait,
                            transport,
                        })
                        .await
                    }
                }
                _ => anyhow::bail!("Invalid mode '{}'. Use: manual or nostr", effective_mode),
            }
        }
        Command::ShowNpub { nsec_file } => secret::show_npub(expand_tilde(&nsec_file)),
        Command::GenerateNostrKey { output, force } => {
            secret::generate_nostr_key(expand_tilde(&output), force)
        }
    }
}
