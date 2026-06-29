//! Token-based authentication for iroh tunnel connections.
//!
//! Provides pre-shared token authentication for the iroh multi-source server.
//!
//! ## Token Format
//! - Exactly 47 characters
//! - Starts with lowercase `i` (for iroh)
//! - Remaining 46 characters are Base64URL (no padding)
//! - Decoded payload is exactly 34 bytes:
//!   - First 32 bytes: random entropy
//!   - Last 2 bytes: CRC16-CCITT-FALSE checksum (big-endian) of the 32 random bytes
//!
//! Generate tokens with: `tunnel-rs generate-auth-token`

use anyhow::{Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::Rng;
use std::collections::HashSet;
use std::path::Path;

/// Required length for authentication tokens.
pub const TOKEN_LENGTH: usize = 47;

/// Required prefix character for tokens.
pub const TOKEN_PREFIX: char = 'i';

/// Number of random bytes in token payload.
const RANDOM_BYTES_LEN: usize = 32;

/// Number of checksum bytes in token payload.
const CHECKSUM_BYTES_LEN: usize = 2;

/// Number of decoded bytes in token payload.
const TOKEN_PAYLOAD_LEN: usize = RANDOM_BYTES_LEN + CHECKSUM_BYTES_LEN;

/// Compute CRC16-CCITT-FALSE.
///
/// Parameters:
/// - Poly: 0x1021
/// - Init: 0xFFFF
/// - RefIn: false
/// - RefOut: false
/// - XorOut: 0x0000
fn crc16_ccitt_false(data: &[u8]) -> u16 {
    let mut crc = 0xFFFFu16;

    for &byte in data {
        crc ^= (byte as u16) << 8;
        for _ in 0..8 {
            if (crc & 0x8000) != 0 {
                crc = (crc << 1) ^ 0x1021;
            } else {
                crc <<= 1;
            }
        }
    }

    crc
}

/// Generate a new authentication token.
///
/// Format: `i` + base64url_no_pad(32 random bytes + 2-byte CRC16) = 47 characters total.
pub fn generate_token() -> String {
    let mut random = [0u8; RANDOM_BYTES_LEN];
    let mut rng = rand::rng();
    rng.fill_bytes(&mut random);

    let checksum = crc16_ccitt_false(&random).to_be_bytes();
    let mut payload = [0u8; TOKEN_PAYLOAD_LEN];
    payload[..RANDOM_BYTES_LEN].copy_from_slice(&random);
    payload[RANDOM_BYTES_LEN..].copy_from_slice(&checksum);

    format!("{}{}", TOKEN_PREFIX, URL_SAFE_NO_PAD.encode(payload))
}

/// Validate token format.
///
/// Returns Ok(()) if valid, Err with description if invalid.
pub fn validate_token(token: &str) -> Result<()> {
    // Early ASCII check - all valid tokens are ASCII
    if !token.is_ascii() {
        anyhow::bail!("Token must contain only ASCII characters");
    }

    if token.len() != TOKEN_LENGTH {
        anyhow::bail!(
            "Token must be exactly {} characters, got {} characters",
            TOKEN_LENGTH,
            token.len()
        );
    }

    // Check prefix
    if !token.starts_with(TOKEN_PREFIX) {
        anyhow::bail!(
            "Token must start with '{}', got '{}'",
            TOKEN_PREFIX,
            token.chars().next().unwrap_or('?')
        );
    }

    let encoded_payload = &token[TOKEN_PREFIX.len_utf8()..];
    let payload = URL_SAFE_NO_PAD
        .decode(encoded_payload)
        .context("Token payload is not valid base64url without padding")?;

    if payload.len() != TOKEN_PAYLOAD_LEN {
        anyhow::bail!(
            "Token payload must decode to exactly {} bytes, got {} bytes",
            TOKEN_PAYLOAD_LEN,
            payload.len()
        );
    }

    let random = &payload[..RANDOM_BYTES_LEN];
    let checksum = &payload[RANDOM_BYTES_LEN..];
    let expected_checksum = crc16_ccitt_false(random).to_be_bytes();

    if checksum != expected_checksum {
        anyhow::bail!("Token checksum is invalid");
    }

    Ok(())
}

/// Load auth tokens from CLI arguments and/or a file.
///
/// # Arguments
/// * `cli_tokens` - Tokens from env var or other sources
/// * `file` - Optional path to a file containing tokens (one per line)
///
/// # Returns
/// A HashSet of all valid authentication tokens
///
/// # Errors
/// Returns an error if the file cannot be read or any token is invalid
pub fn load_auth_tokens(cli_tokens: &[String], file: Option<&Path>) -> Result<HashSet<String>> {
    let mut tokens = HashSet::new();

    // Load from CLI arguments
    for token in cli_tokens {
        let trimmed = token.trim();
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            validate_token(trimmed)
                .with_context(|| format!("Invalid token from CLI: '{}'", trimmed))?;
            tokens.insert(trimmed.to_string());
        }
    }

    // Load from file if specified
    if let Some(file_path) = file {
        let file_tokens = load_auth_tokens_from_file(file_path)?;
        tokens.extend(file_tokens);
    }

    Ok(tokens)
}

/// Parse token entries from file content.
///
/// Yields `(line_number, token)` for each non-empty, non-comment token.
/// Handles comment lines (`#`), empty lines, inline comments, and whitespace trimming.
fn parse_token_lines(content: &str) -> impl Iterator<Item = (usize, &str)> {
    content.lines().enumerate().filter_map(|(i, line)| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return None;
        }
        let token = line.split('#').next().unwrap_or(line).trim();
        if token.is_empty() {
            return None;
        }
        Some((i + 1, token))
    })
}

/// Load authentication tokens from a file.
///
/// # File Format
/// - One token per line (`i` + 46 Base64URL chars, no padding)
/// - Lines starting with `#` are treated as comments
/// - Empty lines are ignored
/// - Inline comments (after token) are supported with `#`
///
/// # Example file:
/// ```text
/// # Authentication tokens (generate with: tunnel-rs generate-auth-token)
/// imfNFxTPDKB3jsM1Q8kzAvZnQHbmJ1W49Rk8i1S2Jzrze9Q
/// ih9SwOUD1nHkQpl4Gf0fQrVrRIt6QctNfPzIlcwkPhzv0ig
/// ```
pub fn load_auth_tokens_from_file(path: &Path) -> Result<HashSet<String>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read auth tokens file: {}", path.display()))?;

    let mut tokens = HashSet::new();

    for (line_num, token) in parse_token_lines(&content) {
        validate_token(token).with_context(|| {
            format!("Invalid token at {}:{}: '{}'", path.display(), line_num, token)
        })?;
        tokens.insert(token.to_string());
    }

    Ok(tokens)
}

/// Load a single auth token from a file.
///
/// # File Format
/// - First non-empty, non-comment line is the token (`i` + 46 Base64URL chars, no padding)
/// - Lines starting with `#` are treated as comments
/// - Empty lines are ignored
/// - Inline comments (after token) are supported with `#`
pub fn load_auth_token_from_file(path: &Path) -> Result<String> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read auth token file: {}", path.display()))?;

    if let Some((line_num, token)) = parse_token_lines(&content).next() {
        validate_token(token).with_context(|| {
            format!("Invalid token at {}:{}: '{}'", path.display(), line_num, token)
        })?;
        return Ok(token.to_string());
    }

    anyhow::bail!("No valid token found in file: {}", path.display())
}

/// Check if a token is in the valid tokens set.
///
/// Returns true if the token is valid, false otherwise.
/// An empty valid_tokens set means no tokens are authorized.
#[inline]
pub fn is_token_valid(token: &str, valid_tokens: &HashSet<String>) -> bool {
    valid_tokens.contains(token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn make_test_token(random: [u8; RANDOM_BYTES_LEN]) -> String {
        let checksum = crc16_ccitt_false(&random).to_be_bytes();
        let mut payload = [0u8; TOKEN_PAYLOAD_LEN];
        payload[..RANDOM_BYTES_LEN].copy_from_slice(&random);
        payload[RANDOM_BYTES_LEN..].copy_from_slice(&checksum);
        format!("{}{}", TOKEN_PREFIX, URL_SAFE_NO_PAD.encode(payload))
    }

    fn decode_payload(token: &str) -> Vec<u8> {
        URL_SAFE_NO_PAD
            .decode(&token[TOKEN_PREFIX.len_utf8()..])
            .unwrap()
    }

    #[test]
    fn test_crc16_ccitt_false_known_vector() {
        // Standard check value for CRC16-CCITT-FALSE with "123456789".
        assert_eq!(crc16_ccitt_false(b"123456789"), 0x29B1);
    }

    #[test]
    fn test_generate_token_format() {
        let token = generate_token();
        assert_eq!(token.len(), TOKEN_LENGTH);
        assert!(token.starts_with(TOKEN_PREFIX));
        assert!(validate_token(&token).is_ok());
    }

    #[test]
    fn test_generate_token_uniqueness() {
        let token1 = generate_token();
        let token2 = generate_token();
        assert_ne!(token1, token2);
    }

    #[test]
    fn test_validate_token_valid() {
        let token = make_test_token([0xAB; RANDOM_BYTES_LEN]);
        assert!(validate_token(&token).is_ok());
    }

    #[test]
    fn test_validate_token_too_short() {
        let result = validate_token("ishort");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("exactly 47 characters"));
    }

    #[test]
    fn test_validate_token_too_long() {
        let token = format!("{}{}", TOKEN_PREFIX, "A".repeat(TOKEN_LENGTH));
        let result = validate_token(&token);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("exactly 47 characters"));
    }

    #[test]
    fn test_validate_token_wrong_prefix() {
        let mut token = generate_token().chars().collect::<Vec<_>>();
        token[0] = 'x';
        let token: String = token.into_iter().collect();

        let result = validate_token(&token);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("must start with 'i'"));
    }

    #[test]
    fn test_validate_token_invalid_base64url_chars() {
        let token = format!("{}{}", TOKEN_PREFIX, "!".repeat(TOKEN_LENGTH - 1));
        let result = validate_token(&token);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("base64url"));
    }

    #[test]
    fn test_validate_token_non_ascii() {
        let result = validate_token("i🔐notascii");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("ASCII"));
    }

    #[test]
    fn test_validate_token_bad_checksum() {
        let mut payload = decode_payload(&generate_token());
        payload[RANDOM_BYTES_LEN] ^= 0x01;
        let bad = format!("{}{}", TOKEN_PREFIX, URL_SAFE_NO_PAD.encode(payload));

        let result = validate_token(&bad);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("checksum"));
    }

    #[test]
    fn test_validate_token_rejects_mutated_random_byte() {
        let mut payload = decode_payload(&generate_token());
        payload[0] ^= 0x80;
        let bad = format!("{}{}", TOKEN_PREFIX, URL_SAFE_NO_PAD.encode(payload));

        let result = validate_token(&bad);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("checksum"));
    }

    #[test]
    fn test_validate_token_rejects_mutated_checksum_byte() {
        let mut payload = decode_payload(&generate_token());
        payload[TOKEN_PAYLOAD_LEN - 1] ^= 0x01;
        let bad = format!("{}{}", TOKEN_PREFIX, URL_SAFE_NO_PAD.encode(payload));

        let result = validate_token(&bad);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("checksum"));
    }

    #[test]
    fn test_load_from_file_with_comments() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "# This is a comment").unwrap();
        writeln!(file).unwrap();
        writeln!(file, "  # Another comment with leading space").unwrap();

        let result = load_auth_tokens_from_file(file.path()).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_load_from_file_with_tokens() {
        let token_a = generate_token();
        let token_b = generate_token();
        let token_c = generate_token();

        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "# Auth tokens").unwrap();
        writeln!(file, "{}", token_a).unwrap();
        writeln!(file, "{}  # inline comment", token_b).unwrap();
        writeln!(file, "  {}  ", token_c).unwrap();

        let result = load_auth_tokens_from_file(file.path()).unwrap();
        assert_eq!(result.len(), 3);
        assert!(result.contains(&token_a));
        assert!(result.contains(&token_b));
        assert!(result.contains(&token_c));
    }

    #[test]
    fn test_load_from_file_invalid_token() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "short").unwrap();

        let result = load_auth_tokens_from_file(file.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_load_single_token_from_file() {
        let token_a = generate_token();
        let token_b = generate_token();

        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "# My auth token").unwrap();
        writeln!(file).unwrap();
        writeln!(file, "{}  # comment", token_a).unwrap();
        writeln!(file, "{}", token_b).unwrap(); // ignored

        let result = load_auth_token_from_file(file.path()).unwrap();
        assert_eq!(result, token_a);
    }

    #[test]
    fn test_load_single_token_invalid() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "bad").unwrap();

        let result = load_auth_token_from_file(file.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_is_token_valid_empty_set_rejects_all() {
        let token = generate_token();
        let valid_tokens = HashSet::new();
        assert!(!is_token_valid(&token, &valid_tokens));
    }

    #[test]
    fn test_is_token_valid_in_set() {
        let token = generate_token();
        let mut valid_tokens = HashSet::new();
        valid_tokens.insert(token.clone());

        assert!(is_token_valid(&token, &valid_tokens));
    }

    #[test]
    fn test_is_token_valid_not_in_set() {
        let token_a = generate_token();
        let token_b = generate_token();
        let mut valid_tokens = HashSet::new();
        valid_tokens.insert(token_a.clone());

        assert!(!is_token_valid(&token_b, &valid_tokens));
    }

    #[test]
    fn test_load_auth_tokens_cli_and_file() {
        let token_a = generate_token();
        let token_b = generate_token();

        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "{}", token_a).unwrap();

        let cli_tokens = vec![token_b.clone()];
        let result = load_auth_tokens(&cli_tokens, Some(file.path())).unwrap();

        assert_eq!(result.len(), 2);
        assert!(result.contains(&token_a));
        assert!(result.contains(&token_b));
    }

    #[test]
    fn test_load_auth_tokens_cli_invalid() {
        let cli_tokens = vec!["short".to_string()];
        let result = load_auth_tokens(&cli_tokens, None);
        assert!(result.is_err());
    }

}
