//! Signaling layer for tunnel connection establishment.
//!
//! This module provides signaling mechanisms for exchanging connection information:
//! - `codec`: Manual/Nostr signaling payloads + encode/decode
//! - `manual`: Stdin/stdout helpers for manual copy-paste signaling
//! - `nostr`: Nostr relay-based automated signaling

pub mod codec;
pub mod manual;
pub mod nostr;

// Re-export manual/nostr signaling types from local codec
pub use codec::{
    decode_answer, decode_offer, encode_answer, encode_offer, ManualAnswer, ManualOffer,
    ManualReject, ManualRequest, MANUAL_SIGNAL_VERSION,
};

pub use manual::{display_answer, display_offer, read_answer_from_stdin, read_offer_from_stdin};
pub use nostr::{NostrSignaling, OfferWaitError, SignalingError};
