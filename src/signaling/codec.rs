//! Iroh signaling payload types and encoding/decoding.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Version 4: Ed25519 challenge-response authentication.
pub const IROH_MULTI_VERSION: u16 = 4;

/// Maximum length for rejection reason to prevent excessively large messages.
pub const MAX_REJECT_REASON_LENGTH: usize = 512;

/// Truncate a rejection reason to the maximum allowed length.
/// If truncation is needed, appends "..." suffix at a valid UTF-8 boundary.
fn truncate_reason(reason: String, max_len: usize) -> String {
    const TRUNCATION_SUFFIX: &str = "...";
    if reason.len() > max_len {
        let max_content_len = max_len.saturating_sub(TRUNCATION_SUFFIX.len());
        let truncated = &reason[..reason.floor_char_boundary(max_content_len)];
        format!("{}{}", truncated, TRUNCATION_SUFFIX)
    } else {
        reason
    }
}

// ============================================================================
// Iroh Multi-Source Handshake Protocol
// ============================================================================

/// Source request sent by receiver after iroh connection established.
/// Used in iroh multi-source mode to request a specific forwarding target.
/// Note: Authentication is handled at connection level via AuthRequest/AuthResponse.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceRequest {
    pub version: u16,
    /// Requested source endpoint (e.g., "tcp://127.0.0.1:22" or "udp://127.0.0.1:53")
    pub source: String,
}

impl SourceRequest {
    pub fn new(source: String) -> Self {
        Self {
            version: IROH_MULTI_VERSION,
            source,
        }
    }
}

/// Source response from sender to receiver.
/// Indicates whether the requested source was accepted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceResponse {
    pub version: u16,
    /// Whether the source request was accepted
    pub accepted: bool,
    /// Reason for rejection (if rejected)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl SourceResponse {
    pub fn accepted() -> Self {
        Self {
            version: IROH_MULTI_VERSION,
            accepted: true,
            reason: None,
        }
    }

    /// Create a rejection response with the given reason.
    /// The reason will be truncated if it exceeds [`MAX_REJECT_REASON_LENGTH`].
    pub fn rejected(reason: impl Into<String>) -> Self {
        Self {
            version: IROH_MULTI_VERSION,
            accepted: false,
            reason: Some(truncate_reason(reason.into(), MAX_REJECT_REASON_LENGTH)),
        }
    }
}

/// Fresh authentication challenge sent by the server on the auth stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthChallenge {
    pub version: u16,
    pub challenge: Vec<u8>,
}

impl AuthChallenge {
    pub fn new(challenge: Vec<u8>) -> Self {
        Self {
            version: IROH_MULTI_VERSION,
            challenge,
        }
    }
}

/// Ed25519 proof sent by the client after receiving an authentication challenge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthRequest {
    pub version: u16,
    pub public_key: Vec<u8>,
    pub signature: Vec<u8>,
}

impl AuthRequest {
    pub fn new(public_key: Vec<u8>, signature: Vec<u8>) -> Self {
        Self {
            version: IROH_MULTI_VERSION,
            public_key,
            signature,
        }
    }
}

/// Authentication response from server to client.
/// Sent in response to AuthRequest on the auth stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthResponse {
    pub version: u16,
    /// Whether authentication was accepted
    pub accepted: bool,
    /// Reason for rejection (if rejected)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl AuthResponse {
    pub fn accepted() -> Self {
        Self {
            version: IROH_MULTI_VERSION,
            accepted: true,
            reason: None,
        }
    }

    /// Create a rejection response with the given reason.
    /// The reason will be truncated if it exceeds [`MAX_REJECT_REASON_LENGTH`].
    pub fn rejected(reason: impl Into<String>) -> Self {
        Self {
            version: IROH_MULTI_VERSION,
            accepted: false,
            reason: Some(truncate_reason(reason.into(), MAX_REJECT_REASON_LENGTH)),
        }
    }
}

// ============================================================================
// Stream-based Encoding/Decoding for Iroh Multi-Source
// ============================================================================

/// Maximum size for source request/response messages (16KB)
pub const MAX_SOURCE_MESSAGE_SIZE: usize = 16 * 1024;

// ============================================================================
// Length-Prefixed JSON Helpers
// ============================================================================

/// Encode a value as length-prefixed JSON bytes.
fn encode_length_prefixed<T: Serialize>(value: &T, type_name: &str) -> Result<Vec<u8>> {
    let json =
        serde_json::to_vec(value).with_context(|| format!("Failed to serialize {}", type_name))?;
    if json.len() > MAX_SOURCE_MESSAGE_SIZE {
        anyhow::bail!("{} too large: {} bytes", type_name, json.len());
    }
    let len = (json.len() as u32).to_be_bytes();
    let mut buf = Vec::with_capacity(4 + json.len());
    buf.extend_from_slice(&len);
    buf.extend_from_slice(&json);
    Ok(buf)
}

/// Decode a length-prefixed JSON value with version validation.
fn decode_length_prefixed<T: for<'de> Deserialize<'de>>(
    data: &[u8],
    expected_version: u16,
    get_version: impl FnOnce(&T) -> u16,
    type_name: &str,
) -> Result<T> {
    if data.len() < 4 {
        anyhow::bail!("{} too short: {} bytes", type_name, data.len());
    }
    let len = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
    if len > MAX_SOURCE_MESSAGE_SIZE {
        anyhow::bail!("{} length too large: {} bytes", type_name, len);
    }
    if data.len() < 4 + len {
        anyhow::bail!(
            "{} incomplete: expected {} bytes, got {}",
            type_name,
            4 + len,
            data.len()
        );
    }
    let value: T = serde_json::from_slice(&data[4..4 + len])
        .with_context(|| format!("Invalid {} JSON", type_name))?;
    let version = get_version(&value);
    if version != expected_version {
        anyhow::bail!(
            "{} version mismatch: expected {}, got {}",
            type_name,
            expected_version,
            version
        );
    }
    Ok(value)
}

/// Encode a SourceRequest as length-prefixed JSON bytes.
pub fn encode_source_request(req: &SourceRequest) -> Result<Vec<u8>> {
    encode_length_prefixed(req, "SourceRequest")
}

/// Decode a SourceRequest from length-prefixed JSON bytes.
pub fn decode_source_request(data: &[u8]) -> Result<SourceRequest> {
    decode_length_prefixed(
        data,
        IROH_MULTI_VERSION,
        |r: &SourceRequest| r.version,
        "SourceRequest",
    )
}

/// Encode a SourceResponse as length-prefixed JSON bytes.
pub fn encode_source_response(resp: &SourceResponse) -> Result<Vec<u8>> {
    encode_length_prefixed(resp, "SourceResponse")
}

/// Decode a SourceResponse from length-prefixed JSON bytes.
pub fn decode_source_response(data: &[u8]) -> Result<SourceResponse> {
    decode_length_prefixed(
        data,
        IROH_MULTI_VERSION,
        |r: &SourceResponse| r.version,
        "SourceResponse",
    )
}

/// Encode an AuthRequest as length-prefixed JSON bytes.
pub fn encode_auth_request(req: &AuthRequest) -> Result<Vec<u8>> {
    encode_length_prefixed(req, "AuthRequest")
}

/// Encode an AuthChallenge as length-prefixed JSON bytes.
pub fn encode_auth_challenge(challenge: &AuthChallenge) -> Result<Vec<u8>> {
    encode_length_prefixed(challenge, "AuthChallenge")
}

/// Decode an AuthChallenge from length-prefixed JSON bytes.
pub fn decode_auth_challenge(data: &[u8]) -> Result<AuthChallenge> {
    decode_length_prefixed(
        data,
        IROH_MULTI_VERSION,
        |challenge: &AuthChallenge| challenge.version,
        "AuthChallenge",
    )
}

/// Decode an AuthRequest from length-prefixed JSON bytes.
pub fn decode_auth_request(data: &[u8]) -> Result<AuthRequest> {
    decode_length_prefixed(
        data,
        IROH_MULTI_VERSION,
        |r: &AuthRequest| r.version,
        "AuthRequest",
    )
}

/// Encode an AuthResponse as length-prefixed JSON bytes.
pub fn encode_auth_response(resp: &AuthResponse) -> Result<Vec<u8>> {
    encode_length_prefixed(resp, "AuthResponse")
}

/// Decode an AuthResponse from length-prefixed JSON bytes.
pub fn decode_auth_response(data: &[u8]) -> Result<AuthResponse> {
    decode_length_prefixed(
        data,
        IROH_MULTI_VERSION,
        |r: &AuthResponse| r.version,
        "AuthResponse",
    )
}

/// Read a length-prefixed message from a stream.
/// Returns the raw bytes including the length prefix.
pub async fn read_length_prefixed<R: tokio::io::AsyncReadExt + Unpin>(
    reader: &mut R,
) -> Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    reader
        .read_exact(&mut len_buf)
        .await
        .context("Failed to read message length")?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_SOURCE_MESSAGE_SIZE {
        anyhow::bail!("Message length too large: {} bytes", len);
    }
    let mut buf = Vec::with_capacity(4 + len);
    buf.extend_from_slice(&len_buf);
    buf.resize(4 + len, 0);
    reader
        .read_exact(&mut buf[4..])
        .await
        .context("Failed to read message body")?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_source_request_serde_roundtrip() {
        let request = SourceRequest::new("tcp://127.0.0.1:22".to_string());
        let encoded = encode_source_request(&request).unwrap();
        let decoded = decode_source_request(&encoded).unwrap();
        assert_eq!(decoded.source, "tcp://127.0.0.1:22");
        assert_eq!(decoded.version, IROH_MULTI_VERSION);
    }

    #[test]
    fn test_truncate_reason_no_truncation() {
        let reason = "short reason".to_string();
        let result = truncate_reason(reason.clone(), 100);
        assert_eq!(result, reason);
    }

    #[test]
    fn test_truncate_reason_exact_limit() {
        let reason = "x".repeat(10);
        let result = truncate_reason(reason.clone(), 10);
        assert_eq!(result, reason); // No truncation at exact limit
    }

    #[test]
    fn test_truncate_reason_ascii_truncation() {
        let reason = "a".repeat(20);
        let result = truncate_reason(reason, 10);
        assert_eq!(result, "aaaaaaa..."); // 7 chars + "..."
        assert_eq!(result.len(), 10);
    }

    #[test]
    fn test_truncate_reason_utf8_safe_truncation() {
        // "é" is 2 bytes in UTF-8
        let reason = "ééééé".to_string(); // 10 bytes
        let result = truncate_reason(reason, 8);
        // Should truncate at valid UTF-8 boundary
        // max_content_len = 8 - 3 = 5, floor_char_boundary(5) = 4 (2 chars)
        assert_eq!(result, "éé...");
        assert!(result.len() <= 8);
    }

    #[test]
    fn test_truncate_reason_emoji_safe_truncation() {
        // "🔐" is 4 bytes in UTF-8
        let reason = "🔐🔐🔐".to_string(); // 12 bytes
        let result = truncate_reason(reason, 10);
        // max_content_len = 10 - 3 = 7, floor_char_boundary(7) = 4 (1 emoji)
        assert_eq!(result, "🔐...");
        assert!(result.len() <= 10);
    }

    #[test]
    fn test_truncate_reason_suffix_only_edge_case() {
        let reason = "abcdef".to_string();
        let result = truncate_reason(reason, 3);
        // max_content_len = 3 - 3 = 0, so just suffix
        assert_eq!(result, "...");
    }

    // ========================================================================
    // Decode error path tests
    // ========================================================================

    #[test]
    fn test_decode_source_request_too_short() {
        assert!(decode_source_request(&[0, 0]).is_err());
    }

    #[test]
    fn test_decode_source_request_incomplete() {
        // Length prefix says 100 bytes but only 4 bytes of body follow
        let mut buf = vec![0, 0, 0, 100];
        buf.extend_from_slice(b"abcd");
        assert!(decode_source_request(&buf).is_err());
    }

    #[test]
    fn test_decode_source_request_invalid_json() {
        // Length prefix matches body length, but body is not valid JSON
        let body = b"not json";
        let len = (body.len() as u32).to_be_bytes();
        let mut buf = Vec::from(len);
        buf.extend_from_slice(body);
        assert!(decode_source_request(&buf).is_err());
    }

    #[test]
    fn test_decode_source_request_wrong_version() {
        let bad = SourceRequest {
            version: IROH_MULTI_VERSION + 1,
            source: "tcp://127.0.0.1:22".into(),
        };
        let json = serde_json::to_vec(&bad).unwrap();
        let len = (json.len() as u32).to_be_bytes();
        let mut buf = Vec::from(len);
        buf.extend_from_slice(&json);
        let err = decode_source_request(&buf).unwrap_err();
        assert!(err.to_string().contains("version mismatch"));
    }

    #[test]
    fn test_decode_source_request_exceeds_max_size() {
        // Length prefix claims a size larger than MAX_SOURCE_MESSAGE_SIZE
        let len = ((MAX_SOURCE_MESSAGE_SIZE + 1) as u32).to_be_bytes();
        let buf = Vec::from(len);
        let err = decode_source_request(&buf).unwrap_err();
        assert!(err.to_string().contains("too large"));
    }

    #[test]
    fn test_encode_source_request_exceeds_max_size() {
        let req = SourceRequest::new("x".repeat(MAX_SOURCE_MESSAGE_SIZE));
        let err = encode_source_request(&req).unwrap_err();
        assert!(err.to_string().contains("too large"));
    }

    // ========================================================================
    // AuthRequest / AuthResponse roundtrip tests
    // ========================================================================

    #[test]
    fn test_auth_request_roundtrip() {
        let req = AuthRequest::new(vec![1; 32], vec![2; 64]);
        let encoded = encode_auth_request(&req).unwrap();
        let decoded = decode_auth_request(&encoded).unwrap();
        assert_eq!(decoded.version, IROH_MULTI_VERSION);
        assert_eq!(decoded.public_key, vec![1; 32]);
        assert_eq!(decoded.signature, vec![2; 64]);
    }

    #[test]
    fn test_auth_challenge_roundtrip() {
        let challenge = AuthChallenge::new(vec![3; 32]);
        let encoded = encode_auth_challenge(&challenge).unwrap();
        let decoded = decode_auth_challenge(&encoded).unwrap();
        assert_eq!(decoded.version, IROH_MULTI_VERSION);
        assert_eq!(decoded.challenge, vec![3; 32]);
    }

    #[test]
    fn test_auth_response_accepted_roundtrip() {
        let resp = AuthResponse::accepted();
        let encoded = encode_auth_response(&resp).unwrap();
        let decoded = decode_auth_response(&encoded).unwrap();
        assert_eq!(decoded.version, IROH_MULTI_VERSION);
        assert!(decoded.accepted);
        assert!(decoded.reason.is_none());
    }

    #[test]
    fn test_auth_response_rejected_roundtrip() {
        let resp = AuthResponse::rejected("bad signature");
        let encoded = encode_auth_response(&resp).unwrap();
        let decoded = decode_auth_response(&encoded).unwrap();
        assert_eq!(decoded.version, IROH_MULTI_VERSION);
        assert!(!decoded.accepted);
        assert_eq!(decoded.reason.as_deref(), Some("bad signature"));
    }

    #[test]
    fn test_source_response_rejected_roundtrip() {
        let resp = SourceResponse::rejected("not allowed");
        let encoded = encode_source_response(&resp).unwrap();
        let decoded = decode_source_response(&encoded).unwrap();
        assert_eq!(decoded.version, IROH_MULTI_VERSION);
        assert!(!decoded.accepted);
        assert_eq!(decoded.reason.as_deref(), Some("not allowed"));
    }
}
