//! ICE-specific default values for manual and nostr modes.

use tunnel_common::config::{CongestionController, TransportTuning};

/// Default QUIC receive window size for ICE modes (8 MB).
pub const DEFAULT_ICE_RECEIVE_WINDOW: u32 = 8 * 1024 * 1024;

/// Default QUIC send window size for ICE modes (8 MB).
pub const DEFAULT_ICE_SEND_WINDOW: u32 = 8 * 1024 * 1024;

/// Default transport tuning for ICE modes (manual/nostr).
pub fn default_ice_transport_tuning() -> TransportTuning {
    TransportTuning {
        congestion_controller: CongestionController::Cubic,
        receive_window: Some(DEFAULT_ICE_RECEIVE_WINDOW),
        send_window: Some(DEFAULT_ICE_SEND_WINDOW),
    }
}

/// Default public STUN servers for ICE mode.
pub fn default_stun_servers() -> Vec<String> {
    vec![
        "stun.l.google.com:19302".to_string(),
        "stun1.l.google.com:19302".to_string(),
        "stun.cloudflare.com:3478".to_string(),
    ]
}

/// Default public Nostr relays for signaling.
pub const DEFAULT_NOSTR_RELAYS: &[&str] = &[
    "wss://nos.lol",
    //"wss://relay.damus.io", // acceptable for index queries; not recommended for high-volume operations due to rate limiting
    //"wss://relay.nostr.band",
    "wss://relay.nostr.net",
    "wss://relay.primal.net",
    "wss://relay.snort.social",
];

/// Default public Nostr relays for signaling.
pub fn default_nostr_relays() -> &'static [&'static str] {
    DEFAULT_NOSTR_RELAYS
}
