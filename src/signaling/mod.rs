//! Iroh signaling codecs.

pub mod codec;

// Auth-related (iroh multi-source authentication)
pub use codec::{
    decode_auth_challenge, decode_auth_request, decode_auth_response, encode_auth_challenge,
    encode_auth_request, encode_auth_response, AuthChallenge, AuthRequest, AuthResponse,
};

// Source-related (iroh multi-source requests)
pub use codec::{
    decode_source_request, decode_source_response, encode_source_request, encode_source_response,
    read_length_prefixed, SourceRequest, SourceResponse,
};
