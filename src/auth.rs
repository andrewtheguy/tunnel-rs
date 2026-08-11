//! Transport-independent Ed25519 public-key authentication.
//!
//! The server issues a fresh random challenge. The client signs a
//! domain-separated transcript with a compact Ed25519 private key, and the
//! server verifies the proof against an SSH-like authorized-keys file.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

pub const CHALLENGE_LENGTH: usize = 32;
pub const PUBLIC_KEY_LENGTH: usize = 32;
pub const SIGNATURE_LENGTH: usize = 64;

const AUTH_DOMAIN: &[u8] = b"tunnel-rs public-key authentication v1\0";
const AUTHORIZED_KEY_TYPE: &str = "ed25519";
const PRIVATE_KEY_PREFIX: &str = "tunnel-rs-ed25519-private-key-v1:";

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

/// Load Ed25519 keys from an SSH-like authorized-keys file.
///
/// Each non-comment line must have the form:
/// `ed25519 BASE64_KEY optional comment`
pub fn load_authorized_keys(path: &Path) -> Result<AuthorizedKeys> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read authorized keys file: {}", path.display()))?;
    let mut keys = HashMap::new();

    for (index, line) in content.lines().enumerate() {
        let line_number = index + 1;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.first().copied() != Some(AUTHORIZED_KEY_TYPE) || fields.len() < 2 {
            anyhow::bail!(
                "Unsupported authorized key at {}:{}; expected 'ed25519 BASE64_KEY [comment]'",
                path.display(),
                line_number
            );
        }

        let key_bytes = decode_public_key(fields[1]).with_context(|| {
            format!("Invalid authorized key at {}:{}", path.display(), line_number)
        })?;
        VerifyingKey::from_bytes(&key_bytes).with_context(|| {
            format!(
                "Invalid Ed25519 public key at {}:{}",
                path.display(),
                line_number
            )
        })?;

        keys.insert(key_bytes, fields[2..].join(" "));
    }

    Ok(AuthorizedKeys { keys })
}

/// Load a compact Ed25519 private key.
///
/// The file contains a short, versioned prefix followed by standard base64
/// encoding of the 32-byte private seed.
pub fn load_private_key(path: &Path) -> Result<ClientAuthKey> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read private key file: {}", path.display()))?;
    let mut private_key_line = None;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if private_key_line.is_some() {
            anyhow::bail!(
                "Invalid authentication private key in {}: multiple key lines",
                path.display()
            );
        }
        private_key_line = Some(line);
    }
    let private_key_line = private_key_line.ok_or_else(|| {
        anyhow::anyhow!("No authentication private key found in {}", path.display())
    })?;
    let encoded = private_key_line
        .strip_prefix(PRIVATE_KEY_PREFIX)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Invalid authentication private key in {}: expected '{}' prefix",
                path.display(),
                PRIVATE_KEY_PREFIX
            )
        })?;
    let bytes = BASE64
        .decode(encoded)
        .with_context(|| format!("Invalid base64 private key: {}", path.display()))?;
    let bytes: [u8; 32] = bytes.try_into().map_err(|bytes: Vec<u8>| {
        anyhow::anyhow!(
            "Invalid private key in {}: expected 32 bytes, got {}",
            path.display(),
            bytes.len()
        )
    })?;
    let signing_key = SigningKey::from_bytes(&bytes);

    Ok(ClientAuthKey { signing_key })
}

/// Generate a compact private key and its matching authorized-key entry.
pub fn generate_keypair(comment: &str) -> (String, String) {
    let signing_key = SigningKey::from_bytes(&rand::random());
    let private_key = format!(
        "{}{}",
        PRIVATE_KEY_PREFIX,
        BASE64.encode(signing_key.to_bytes())
    );
    let public_key = BASE64.encode(signing_key.verifying_key().to_bytes());
    let comment = comment.trim();
    let authorized_key = if comment.is_empty() {
        format!("{} {}", AUTHORIZED_KEY_TYPE, public_key)
    } else {
        format!("{} {} {}", AUTHORIZED_KEY_TYPE, public_key, comment)
    };
    (private_key, authorized_key)
}

/// Write a newly generated compact private key with restricted permissions and
/// print the matching authorized-key entry to stdout.
pub fn generate_key_file(path: &Path, comment: &str, force: bool) -> Result<()> {
    if path.exists() && !force {
        anyhow::bail!(
            "File already exists: {}. Use --force to overwrite.",
            path.display()
        );
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("Failed to create parent directory")?;
    }

    let (private_key, authorized_key) = generate_keypair(comment);
    let private_key_file = format!("# public key: {}\n{}\n", authorized_key, private_key);

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
        file.write_all(private_key_file.as_bytes())
            .context("Failed to write authentication private key file")?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .context("Failed to set authentication private key file permissions")?;
    }

    #[cfg(not(unix))]
    std::fs::write(path, private_key_file)
        .context("Failed to write authentication private key file")?;

    log::info!("Authentication private key saved to: {}", path.display());
    println!("{}", authorized_key);
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
        writeln!(file, "ed25519 {} user@example.com", public_key).unwrap();
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
    fn rejects_non_ed25519_authorized_key() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQC comment").unwrap();

        let error = load_authorized_keys(file.path()).unwrap_err();
        assert!(error.to_string().contains("expected 'ed25519"));
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
        assert!(error.to_string().contains("expected 'tunnel-rs-ed25519-private-key-v1:'"));
    }

    #[test]
    fn generated_keypair_uses_compact_base64_format() {
        let (private_key, authorized_key) = generate_keypair("alice laptop");
        let encoded = private_key.strip_prefix(PRIVATE_KEY_PREFIX).unwrap();
        let private_bytes = BASE64.decode(encoded).unwrap();
        assert_eq!(private_bytes.len(), 32);
        assert!(!private_key.contains("BEGIN"));
        assert!(private_key.starts_with("tunnel-rs-ed25519-private-key-v1:"));
        assert!(authorized_key.starts_with("ed25519 "));
        assert!(authorized_key.ends_with(" alice laptop"));
    }

    #[test]
    fn generated_key_file_includes_public_key_comment() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("client.key");
        generate_key_file(&path, "alice laptop", false).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.starts_with("# public key: ed25519 "));
        let authorized_entry = content
            .lines()
            .next()
            .unwrap()
            .strip_prefix("# public key: ")
            .unwrap();
        assert!(authorized_entry.ends_with(" alice laptop"));
        assert!(content.lines().nth(1).unwrap().starts_with(PRIVATE_KEY_PREFIX));

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
