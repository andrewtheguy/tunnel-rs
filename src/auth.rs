//! Transport-independent Ed25519 public-key authentication.
//!
//! The server issues a fresh random challenge. The client signs a
//! domain-separated transcript with a compact Ed25519 private key, and the
//! server verifies the proof against an authorized-keys file.
//!
//! Both key kinds are a single prefixed token holding unpadded URL-safe base64
//! of 32 raw bytes. An authorized-keys line is the public token, optionally
//! followed by a single space and a free-form comment that runs to end of line:
//!
//! ```text
//! tunnelrsv1authpub:<urlsafe-base64> alice laptop
//! ```

use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as BASE64, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::Serialize;

pub const CHALLENGE_LENGTH: usize = 32;
pub const PUBLIC_KEY_LENGTH: usize = 32;
pub const SIGNATURE_LENGTH: usize = 64;

const AUTH_DOMAIN: &[u8] = b"tunnel-rs public-key authentication v1\0";
const PUBLIC_KEY_PREFIX: &str = "tunnelrsv1authpub:";
const PRIVATE_KEY_PREFIX: &str = "tunnelrsv1authsecret:";
/// Separates the public-key token from its comment. A single space only: every
/// byte after it, including further spaces, belongs to the comment.
const COMMENT_DELIMITER: char = ' ';

pub type Challenge = [u8; CHALLENGE_LENGTH];
pub type PublicKeyBytes = [u8; PUBLIC_KEY_LENGTH];
pub type SignatureBytes = [u8; SIGNATURE_LENGTH];

/// Ed25519 public keys accepted by the server, indexed by their raw key bytes.
#[derive(Clone, Default)]
pub struct AuthorizedKeys {
    keys: HashMap<PublicKeyBytes, String>,
}

impl AuthorizedKeys {
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Verify a client's proof of possession.
    ///
    /// Returns the authorized-key comment when the proof is valid, `None` for
    /// an unknown key or invalid signature, and an error for malformed wire
    /// data.
    pub fn verify_proof<'a>(
        &'a self,
        challenge: &Challenge,
        public_key: &[u8],
        signature: &[u8],
    ) -> Result<Option<&'a str>> {
        let public_key: PublicKeyBytes = public_key.try_into().map_err(|_| {
            anyhow::anyhow!(
                "Ed25519 public key must be exactly {} bytes",
                PUBLIC_KEY_LENGTH
            )
        })?;
        let signature: SignatureBytes = signature.try_into().map_err(|_| {
            anyhow::anyhow!(
                "Ed25519 signature must be exactly {} bytes",
                SIGNATURE_LENGTH
            )
        })?;

        let Some(comment) = self.keys.get(&public_key) else {
            return Ok(None);
        };

        let verifying_key = VerifyingKey::from_bytes(&public_key)
            .context("Invalid Ed25519 public key in authentication request")?;
        let signature = Signature::from_bytes(&signature);
        if verifying_key
            .verify_strict(&signed_message(challenge), &signature)
            .is_ok()
        {
            Ok(Some(comment))
        } else {
            Ok(None)
        }
    }
}

impl std::fmt::Debug for AuthorizedKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthorizedKeys")
            .field("key_count", &self.keys.len())
            .finish()
    }
}

/// Client Ed25519 signing key loaded from a compact, versioned private-key file.
#[derive(Clone)]
pub struct ClientAuthKey {
    signing_key: SigningKey,
}

impl ClientAuthKey {
    pub fn public_key(&self) -> PublicKeyBytes {
        self.signing_key.verifying_key().to_bytes()
    }

    pub fn sign_challenge(&self, challenge: &Challenge) -> SignatureBytes {
        self.signing_key.sign(&signed_message(challenge)).to_bytes()
    }
}

impl std::fmt::Debug for ClientAuthKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ClientAuthKey([REDACTED])")
    }
}

/// Generate a fresh server challenge for one authentication attempt.
pub fn generate_challenge() -> Challenge {
    rand::random()
}

/// Load Ed25519 keys from an authorized-keys file.
///
/// Each non-comment line must have the form:
/// `tunnelrsv1authpub:URLSAFE_BASE64[ comment]`
pub fn load_authorized_keys(path: &Path) -> Result<AuthorizedKeys> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read authorized keys file: {}", path.display()))?;
    parse_authorized_keys(&content, &path.display().to_string())
}

/// Parse authorized-key entries from inline config values.
///
/// Each entry holds one authorized-keys line; blank lines and `#` comments are
/// skipped just like in a file. `origin` names the config field in errors.
pub fn parse_authorized_keys_entries(entries: &[String], origin: &str) -> Result<AuthorizedKeys> {
    parse_authorized_keys(&entries.join("\n"), origin)
}

/// Parse authorized-key lines, labelling errors with `origin` (a file path or a
/// config field name).
fn parse_authorized_keys(content: &str, origin: &str) -> Result<AuthorizedKeys> {
    let mut keys = HashMap::new();

    for (index, line) in content.lines().enumerate() {
        let line_number = index + 1;
        let line = line.trim_end_matches(['\r', '\n']);
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }

        let (token, comment) = match line.split_once(COMMENT_DELIMITER) {
            Some((token, comment)) => (token, comment),
            None => (line, ""),
        };
        let encoded = token.strip_prefix(PUBLIC_KEY_PREFIX).ok_or_else(|| {
            anyhow::anyhow!(
                "Unsupported authorized key at {}:{}; expected '{}URLSAFE_BASE64[ comment]'",
                origin,
                line_number,
                PUBLIC_KEY_PREFIX
            )
        })?;

        let key_bytes = decode_public_key(encoded)
            .with_context(|| format!("Invalid authorized key at {}:{}", origin, line_number))?;
        VerifyingKey::from_bytes(&key_bytes)
            .with_context(|| format!("Invalid Ed25519 public key at {}:{}", origin, line_number))?;

        keys.insert(key_bytes, comment.to_string());
    }

    Ok(AuthorizedKeys { keys })
}

/// Load a compact Ed25519 private key from a file.
///
/// The file contains a short, versioned prefix followed by URL-safe base64
/// encoding of the 32-byte private seed.
pub fn load_private_key(path: &Path) -> Result<ClientAuthKey> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read private key file: {}", path.display()))?;
    parse_private_key(&content, &path.display().to_string())
}

/// Parse a compact Ed25519 private key, labelling errors with `origin` (a file
/// path or a config field name).
///
/// Accepts the same content as a private-key file, so an inline value may be
/// either the bare token or the full generated file including its `#` comments.
pub fn parse_private_key(content: &str, origin: &str) -> Result<ClientAuthKey> {
    let mut private_key_line = None;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if private_key_line.is_some() {
            anyhow::bail!(
                "Invalid authentication private key in {}: multiple key lines",
                origin
            );
        }
        private_key_line = Some(line);
    }
    let private_key_line = private_key_line
        .ok_or_else(|| anyhow::anyhow!("No authentication private key found in {}", origin))?;
    let encoded = private_key_line
        .strip_prefix(PRIVATE_KEY_PREFIX)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Invalid authentication private key in {}: expected '{}' prefix",
                origin,
                PRIVATE_KEY_PREFIX
            )
        })?;
    let bytes = BASE64
        .decode(encoded)
        .with_context(|| format!("Invalid base64 private key: {}", origin))?;
    let bytes: [u8; 32] = bytes.try_into().map_err(|bytes: Vec<u8>| {
        anyhow::anyhow!(
            "Invalid private key in {}: expected 32 bytes, got {}",
            origin,
            bytes.len()
        )
    })?;
    let signing_key = SigningKey::from_bytes(&bytes);

    Ok(ClientAuthKey { signing_key })
}

/// Generate a compact private key and its matching authorized-key entry.
///
/// Fails when the comment contains a line break, which would split the entry
/// across lines and forge a second authorized key.
pub fn generate_keypair(comment: &str) -> Result<(String, String)> {
    // The comment runs to end of line, so a line break is the one thing it
    // cannot hold; everything else (including inner spaces) is preserved.
    let comment = comment.trim();
    if comment.contains(['\n', '\r']) {
        anyhow::bail!("Authorized-key comment must not contain line breaks");
    }

    let signing_key = SigningKey::from_bytes(&rand::random());
    let private_key = format!(
        "{}{}",
        PRIVATE_KEY_PREFIX,
        BASE64.encode(signing_key.to_bytes())
    );
    let public_key = format!(
        "{}{}",
        PUBLIC_KEY_PREFIX,
        BASE64.encode(signing_key.verifying_key().to_bytes())
    );
    let authorized_key = if comment.is_empty() {
        public_key
    } else {
        format!("{}{}{}", public_key, COMMENT_DELIMITER, comment)
    };
    Ok((private_key, authorized_key))
}

/// Render the private-key file for a freshly generated keypair.
///
/// The first header names the key kind, so a stray key file is identifiable
/// without knowing the token prefixes apart.
fn private_key_file(authorized_key: &str, private_key: &str) -> String {
    format!(
        "# tunnel-rs client authentication key (Ed25519 private key)\n\
         # Created: {}\n\
         # Public key: {}\n\
         {}\n",
        rfc3339_utc(SystemTime::now()),
        authorized_key,
        private_key
    )
}

/// Report a generated key's public half on stderr, unless stdout is a terminal.
///
/// Shared with [`crate::secret`]. When the key file goes to stdout it already
/// names its public half in a header, so on a terminal this copy would only
/// print the same value twice. When stdout is redirected
/// (`generate-auth-key > client.key`) the copy is the only place the entry
/// appears, and `2> authorized_keys` still captures it.
pub fn report_public_half(line: &str) {
    if !std::io::stdout().is_terminal() {
        eprintln!("{}", line);
    }
}

/// Generate a keypair, writing the private key to `path` (or to stdout when
/// `path` is `None` or `-`) and reporting the matching authorized-key entry.
///
/// With a file destination the private key lands in the file with `0600`
/// permissions and the authorized-key entry goes to stdout, so
/// `generate-auth-key --output client.key > authorized_keys` works. Without one
/// the whole private-key file goes to stdout and the authorized-key entry to
/// stderr, so `generate-auth-key > client.key` works and still shows the entry.
/// On a terminal the stderr line is dropped; see [`report_public_half`].
pub fn generate_auth_key(path: Option<&Path>, comment: &str, force: bool) -> Result<()> {
    let (private_key, authorized_key) = generate_keypair(comment)?;
    let private_key_file = private_key_file(&authorized_key, &private_key);

    let Some(path) = path.filter(|path| path.as_os_str() != "-") else {
        print!("{}", private_key_file);
        report_public_half(&authorized_key);
        return Ok(());
    };

    if path.exists() && !force {
        anyhow::bail!(
            "File already exists: {}. Use --force to overwrite.",
            path.display()
        );
    }
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).context("Failed to create parent directory")?;
    }

    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .context("Failed to open authentication private key file")?;
        // `mode(0o600)` only applies when the file is created, so tighten the
        // permissions of a pre-existing (--force) file before writing the key.
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .context("Failed to set authentication private key file permissions")?;
        file.write_all(private_key_file.as_bytes())
            .context("Failed to write authentication private key file")?;
    }

    #[cfg(not(unix))]
    std::fs::write(path, private_key_file)
        .context("Failed to write authentication private key file")?;

    log::info!("Authentication private key saved to: {}", path.display());
    println!("{}", authorized_key);
    Ok(())
}

/// A generated client authentication keypair, as printed by
/// `generate-auth-key --json`.
#[derive(Serialize)]
struct GeneratedAuthKey {
    /// The authorized-keys line, comment included, for the server's file.
    authorized_key: String,
    /// The client's private key, in the same form as the key file's key line.
    private_key: String,
}

/// Generate a keypair and print both halves as a single JSON object, for
/// automation that captures the key material instead of writing a file.
pub fn generate_auth_key_json(comment: &str) -> Result<()> {
    let (private_key, authorized_key) = generate_keypair(comment)?;
    println!(
        "{}",
        serde_json::to_string(&GeneratedAuthKey {
            authorized_key,
            private_key,
        })?
    );
    Ok(())
}

fn decode_public_key(encoded: &str) -> Result<PublicKeyBytes> {
    let bytes = BASE64
        .decode(encoded)
        .context("Public key is not valid standard base64")?;
    bytes.try_into().map_err(|bytes: Vec<u8>| {
        anyhow::anyhow!(
            "Ed25519 public key must decode to {} bytes, got {}",
            PUBLIC_KEY_LENGTH,
            bytes.len()
        )
    })
}

/// Format a timestamp as an RFC 3339 UTC instant, e.g. `2024-09-13T22:22:33Z`.
///
/// Shared with [`crate::secret`], whose generated server keys carry the same
/// `# Created:` header.
pub fn rfc3339_utc(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_secs())
        .unwrap_or(0);
    let (year, month, day) = civil_from_days((seconds / 86_400) as i64);
    let seconds_of_day = seconds % 86_400;
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year,
        month,
        day,
        seconds_of_day / 3600,
        (seconds_of_day % 3600) / 60,
        seconds_of_day % 60
    )
}

/// Convert days since the Unix epoch to a proleptic Gregorian calendar date
/// (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 { shifted } else { shifted - 146_096 } / 146_097;
    let day_of_era = (shifted - era * 146_097) as u64; // [0, 146096]
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365; // [0, 399]
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100); // [0, 365]
    let month_index = (5 * day_of_year + 2) / 153; // [0, 11], March-based
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32; // [1, 31]
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as u32; // [1, 12]
    let year = year_of_era as i64 + era * 400;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

fn signed_message(challenge: &Challenge) -> Vec<u8> {
    let mut message = Vec::with_capacity(AUTH_DOMAIN.len() + challenge.len());
    message.extend_from_slice(AUTH_DOMAIN);
    message.extend_from_slice(challenge);
    message
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::NamedTempFile;

    use super::*;

    fn private_key_file() -> NamedTempFile {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "# public key: test fixture").unwrap();
        write!(file, "{}{}", PRIVATE_KEY_PREFIX, BASE64.encode([42; 32])).unwrap();
        file
    }

    fn authorized_keys_file() -> NamedTempFile {
        let mut file = NamedTempFile::new().unwrap();
        let signing_key = SigningKey::from_bytes(&[42; 32]);
        let public_key = BASE64.encode(signing_key.verifying_key().to_bytes());
        writeln!(file, "# tunnel-rs clients").unwrap();
        writeln!(file).unwrap();
        writeln!(
            file,
            "{}{} user@example.com",
            PUBLIC_KEY_PREFIX, public_key
        )
        .unwrap();
        file
    }

    #[test]
    fn loads_ssh_keys_and_preserves_comment() {
        let private_file = private_key_file();
        let authorized_file = authorized_keys_file();
        let private_key = load_private_key(private_file.path()).unwrap();
        let authorized_keys = load_authorized_keys(authorized_file.path()).unwrap();
        let challenge = [7; CHALLENGE_LENGTH];
        let signature = private_key.sign_challenge(&challenge);

        let comment = authorized_keys
            .verify_proof(&challenge, &private_key.public_key(), &signature)
            .unwrap();
        assert_eq!(comment, Some("user@example.com"));
    }

    #[test]
    fn loads_inline_keys_and_preserves_comment() {
        let signing_key = SigningKey::from_bytes(&[42; 32]);
        let public_key = BASE64.encode(signing_key.verifying_key().to_bytes());
        let authorized_keys = parse_authorized_keys_entries(
            &[
                "# tunnel-rs clients".to_string(),
                String::new(),
                format!("{}{} user@example.com", PUBLIC_KEY_PREFIX, public_key),
            ],
            "[iroh].authorized_keys",
        )
        .unwrap();
        // A bare token, without the surrounding key-file comments.
        let private_key = parse_private_key(
            &format!("{}{}", PRIVATE_KEY_PREFIX, BASE64.encode([42; 32])),
            "[iroh].private_key",
        )
        .unwrap();
        let challenge = [11; CHALLENGE_LENGTH];
        let signature = private_key.sign_challenge(&challenge);

        assert_eq!(authorized_keys.len(), 1);
        assert_eq!(
            authorized_keys
                .verify_proof(&challenge, &private_key.public_key(), &signature)
                .unwrap(),
            Some("user@example.com")
        );
    }

    #[test]
    fn inline_private_key_accepts_a_whole_key_file() {
        let (private_key, authorized_key) = generate_keypair("alice laptop").unwrap();
        // `super::` — the test module has its own `private_key_file` fixture.
        let parsed = parse_private_key(
            &super::private_key_file(&authorized_key, &private_key),
            "[iroh].private_key",
        )
        .unwrap();
        let authorized_keys =
            parse_authorized_keys_entries(&[authorized_key], "[iroh].authorized_keys").unwrap();
        let challenge = [12; CHALLENGE_LENGTH];
        let signature = parsed.sign_challenge(&challenge);

        assert_eq!(
            authorized_keys
                .verify_proof(&challenge, &parsed.public_key(), &signature)
                .unwrap(),
            Some("alice laptop")
        );
    }

    #[test]
    fn inline_key_errors_name_the_config_field() {
        let error = parse_authorized_keys_entries(
            &["ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQC comment".to_string()],
            "[iroh].authorized_keys",
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("at [iroh].authorized_keys:1"));

        let error = parse_private_key(&BASE64.encode([42; 32]), "[iroh].private_key").unwrap_err();
        assert!(error.to_string().contains("in [iroh].private_key"));

        let error = parse_private_key("", "[iroh].private_key").unwrap_err();
        assert!(error.to_string().contains("No authentication private key"));
    }

    #[test]
    fn rejects_signature_for_another_challenge() {
        let private_file = private_key_file();
        let authorized_file = authorized_keys_file();
        let private_key = load_private_key(private_file.path()).unwrap();
        let authorized_keys = load_authorized_keys(authorized_file.path()).unwrap();
        let signature = private_key.sign_challenge(&[1; CHALLENGE_LENGTH]);

        let result = authorized_keys
            .verify_proof(&[2; CHALLENGE_LENGTH], &private_key.public_key(), &signature)
            .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn rejects_proof_from_an_unauthorized_key() {
        let authorized_file = authorized_keys_file();
        let authorized_keys = load_authorized_keys(authorized_file.path()).unwrap();
        let unauthorized_key = ClientAuthKey {
            signing_key: SigningKey::from_bytes(&[99; 32]),
        };
        let challenge = [3; CHALLENGE_LENGTH];
        let signature = unauthorized_key.sign_challenge(&challenge);

        let result = authorized_keys
            .verify_proof(&challenge, &unauthorized_key.public_key(), &signature)
            .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn rejects_malformed_proof_lengths() {
        let authorized_file = authorized_keys_file();
        let authorized_keys = load_authorized_keys(authorized_file.path()).unwrap();
        let challenge = [0; CHALLENGE_LENGTH];

        assert!(authorized_keys.verify_proof(&challenge, &[0; 31], &[0; 64]).is_err());
        assert!(authorized_keys.verify_proof(&challenge, &[0; 32], &[0; 63]).is_err());
    }

    #[test]
    fn rejects_authorized_key_without_the_versioned_prefix() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQC comment").unwrap();

        let error = load_authorized_keys(file.path()).unwrap_err();
        assert!(error.to_string().contains("expected 'tunnelrsv1authpub:"));
    }

    #[test]
    fn authorized_key_comment_is_everything_after_the_first_space() {
        let signing_key = SigningKey::from_bytes(&[42; 32]);
        let public_key = BASE64.encode(signing_key.verifying_key().to_bytes());
        let mut file = NamedTempFile::new().unwrap();
        // Tabs and repeated spaces are comment content, not delimiters.
        writeln!(
            file,
            "{}{}  alice\tlaptop  (spare)",
            PUBLIC_KEY_PREFIX, public_key
        )
        .unwrap();

        let authorized_keys = load_authorized_keys(file.path()).unwrap();
        let client = ClientAuthKey { signing_key };
        let challenge = [5; CHALLENGE_LENGTH];
        let signature = client.sign_challenge(&challenge);
        assert_eq!(
            authorized_keys
                .verify_proof(&challenge, &client.public_key(), &signature)
                .unwrap(),
            Some(" alice\tlaptop  (spare)")
        );
    }

    #[test]
    fn authorized_key_without_a_comment_is_accepted() {
        let signing_key = SigningKey::from_bytes(&[42; 32]);
        let public_key = BASE64.encode(signing_key.verifying_key().to_bytes());
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "{}{}", PUBLIC_KEY_PREFIX, public_key).unwrap();

        let authorized_keys = load_authorized_keys(file.path()).unwrap();
        let client = ClientAuthKey { signing_key };
        let challenge = [6; CHALLENGE_LENGTH];
        let signature = client.sign_challenge(&challenge);
        assert_eq!(
            authorized_keys
                .verify_proof(&challenge, &client.public_key(), &signature)
                .unwrap(),
            Some("")
        );
    }

    #[test]
    fn keys_use_unpadded_url_safe_base64() {
        let (private_key, authorized_key) = generate_keypair("").unwrap();
        let private_encoded = private_key.strip_prefix(PRIVATE_KEY_PREFIX).unwrap();
        let public_encoded = authorized_key.strip_prefix(PUBLIC_KEY_PREFIX).unwrap();

        for encoded in [private_encoded, public_encoded] {
            assert_eq!(encoded.len(), 43, "32 bytes unpadded base64 is 43 chars");
            assert!(!encoded.contains('='));
            assert!(!encoded.contains('+'));
            assert!(!encoded.contains('/'));
            assert!(encoded
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'));
        }
    }

    #[test]
    fn rfc3339_utc_formats_the_epoch_and_a_known_instant() {
        assert_eq!(rfc3339_utc(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(
            rfc3339_utc(UNIX_EPOCH + std::time::Duration::from_secs(1_726_291_353)),
            "2024-09-14T05:22:33Z"
        );
        // Leap day, to exercise the era/leap-year arithmetic.
        assert_eq!(
            rfc3339_utc(UNIX_EPOCH + std::time::Duration::from_secs(1_709_164_800)),
            "2024-02-29T00:00:00Z"
        );
    }

    #[test]
    fn rejects_empty_authorized_keys_file_at_call_site() {
        let file = NamedTempFile::new().unwrap();
        let authorized_keys = load_authorized_keys(file.path()).unwrap();
        assert!(authorized_keys.is_empty());
    }

    #[test]
    fn private_key_requires_versioned_prefix() {
        let mut file = NamedTempFile::new().unwrap();
        write!(file, "{}", BASE64.encode([42; 32])).unwrap();

        let error = load_private_key(file.path()).unwrap_err();
        assert!(error.to_string().contains("expected 'tunnelrsv1authsecret:'"));
    }

    #[test]
    fn generated_keypair_uses_compact_base64_format() {
        let (private_key, authorized_key) = generate_keypair("alice laptop").unwrap();
        let encoded = private_key.strip_prefix(PRIVATE_KEY_PREFIX).unwrap();
        let private_bytes = BASE64.decode(encoded).unwrap();
        assert_eq!(private_bytes.len(), 32);
        assert!(!private_key.contains("BEGIN"));
        assert!(private_key.starts_with("tunnelrsv1authsecret:"));
        assert!(authorized_key.starts_with("tunnelrsv1authpub:"));
        assert!(authorized_key.ends_with(" alice laptop"));
    }

    #[test]
    fn rejects_a_comment_that_would_forge_a_second_authorized_key() {
        let injected = format!(
            "alice\n{}{}",
            PUBLIC_KEY_PREFIX,
            BASE64.encode(SigningKey::from_bytes(&[7; 32]).verifying_key().to_bytes())
        );

        for comment in [injected.as_str(), "alice\r\nbob", "alice\rbob"] {
            let error = generate_keypair(comment).unwrap_err();
            assert!(error.to_string().contains("must not contain line breaks"));
        }

        // Surrounding whitespace is still trimmed rather than rejected.
        let (_, authorized_key) = generate_keypair("\n  alice laptop \n").unwrap();
        assert!(authorized_key.ends_with(" alice laptop"));
        assert_eq!(authorized_key.lines().count(), 1);
    }

    #[test]
    fn line_break_comment_writes_no_key_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("client.key");

        assert!(generate_auth_key(Some(&path), "alice\nbob", false).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn generated_key_file_follows_the_created_public_secret_template() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("client.key");
        generate_auth_key(Some(&path), "alice laptop", false).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let mut lines = content.lines();
        assert_eq!(
            lines.next(),
            Some("# tunnel-rs client authentication key (Ed25519 private key)")
        );
        let created = lines.next().unwrap();
        assert!(created.starts_with("# Created: "));
        assert!(created.ends_with('Z'));
        let authorized_entry = lines.next().unwrap().strip_prefix("# Public key: ").unwrap();
        assert!(authorized_entry.starts_with(PUBLIC_KEY_PREFIX));
        assert!(authorized_entry.ends_with(" alice laptop"));
        assert!(lines.next().unwrap().starts_with(PRIVATE_KEY_PREFIX));
        assert_eq!(lines.next(), None);

        let authorized_path = directory.path().join("authorized_keys");
        std::fs::write(&authorized_path, authorized_entry).unwrap();
        let private_key = load_private_key(&path).unwrap();
        let authorized_keys = load_authorized_keys(&authorized_path).unwrap();
        let challenge = generate_challenge();
        let signature = private_key.sign_challenge(&challenge);
        assert_eq!(
            authorized_keys
                .verify_proof(&challenge, &private_key.public_key(), &signature)
                .unwrap(),
            Some("alice laptop")
        );
    }
}
