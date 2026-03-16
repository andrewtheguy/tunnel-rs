//! Age encryption support for inline secrets in config files.
//!
//! Encrypted values use the `ageenc:` prefix followed by base64-encoded binary
//! age ciphertext. This keeps values on a single line in TOML:
//!
//! ```toml
//! auth_token = "ageenc:YWdlLWVuY3J5cHRpb24ub3Jn..."
//! ```
//!
//! Values are decrypted at startup using an age identity (private key) file.

use age::secrecy::ExposeSecret;
use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use std::path::Path;

const AGEENC_PREFIX: &str = "ageenc:";

/// Check if a string value is an age-encrypted value (has `ageenc:` prefix).
pub fn is_age_encrypted(value: &str) -> bool {
    value.trim().starts_with(AGEENC_PREFIX)
}

/// Parse an x25519 identity (private key) from an age identity file.
///
/// The file format is the standard age identity file:
/// ```text
/// # public key: age1...
/// AGE-SECRET-KEY-1...
/// ```
fn load_identity(path: &Path) -> Result<age::x25519::Identity> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read encryption key file: {}", path.display()))?;

    let key_line = contents
        .lines()
        .find(|l| l.starts_with("AGE-SECRET-KEY-"))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "No AGE-SECRET-KEY found in {}. Generate one with: tunnel-rs config-encryption generate-key",
                path.display()
            )
        })?;

    key_line
        .parse::<age::x25519::Identity>()
        .map_err(|e| anyhow::anyhow!("Failed to parse age identity from {}: {}", path.display(), e))
}

/// Decrypt an `ageenc:`-prefixed value using an identity file.
///
/// The value format is `ageenc:<base64-encoded-binary-ciphertext>`.
pub fn decrypt_value(value: &str, identity_path: &Path) -> Result<String> {
    let encoded = value
        .trim()
        .strip_prefix(AGEENC_PREFIX)
        .ok_or_else(|| anyhow::anyhow!("Value does not start with '{}'", AGEENC_PREFIX))?;
    let ciphertext = BASE64
        .decode(encoded)
        .context("Invalid base64 in ageenc: value")?;
    let identity = load_identity(identity_path)?;
    let plaintext = age::decrypt(&identity, &ciphertext)
        .map_err(|e| anyhow::anyhow!("Age decryption failed: {}", e))?;
    String::from_utf8(plaintext).context("Decrypted value is not valid UTF-8")
}

/// Encrypt a plaintext string for the given age recipient (public key).
///
/// Returns `ageenc:<base64>` — a single-line string suitable for TOML values.
pub fn encrypt_value(plaintext: &str, recipient_str: &str) -> Result<String> {
    let recipient: age::x25519::Recipient = recipient_str
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid age recipient '{}': {}", recipient_str, e))?;
    let ciphertext = age::encrypt(&recipient, plaintext.as_bytes())
        .map_err(|e| anyhow::anyhow!("Encryption failed: {}", e))?;
    Ok(format!("{}{}", AGEENC_PREFIX, BASE64.encode(&ciphertext)))
}

/// Generate a new age x25519 keypair.
///
/// Returns `(secret_key_string, public_key_string)`.
pub fn generate_keypair() -> (String, String) {
    let identity = age::x25519::Identity::generate();
    let secret = identity.to_string();
    let recipient = identity.to_public().to_string();
    (secret.expose_secret().to_string(), recipient)
}

/// Write an age identity file with restricted permissions.
///
/// If the file already exists and `force` is false, the new keypair is
/// appended (separated by a blank line), supporting key rotation.
/// With `force`, the file is overwritten from scratch.
///
/// The file format matches the standard `age-keygen` output:
/// ```text
/// # public key: age1...
/// AGE-SECRET-KEY-1...
/// ```
pub fn write_identity_file(
    path: &Path,
    secret_key: &str,
    public_key: &str,
    force: bool,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("Failed to create parent directory")?;
    }

    let append = path.exists() && !force;
    let content = if append {
        format!("\n# public key: {}\n{}\n", public_key, secret_key)
    } else {
        format!("# public key: {}\n{}\n", public_key, secret_key)
    };

    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .mode(0o600)
            .open(path)
            .context("Failed to open encryption key file")?;
        file.write_all(content.as_bytes())
            .context("Failed to write encryption key file")?;
        // mode() only applies on creation; explicitly set perms for existing files
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .context("Failed to set encryption key file permissions")?;
    }

    #[cfg(not(unix))]
    {
        if append {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(path)
                .context("Failed to open encryption key file")?;
            file.write_all(content.as_bytes())
                .context("Failed to append to encryption key file")?;
        } else {
            std::fs::write(path, &content).context("Failed to write encryption key file")?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_age_encrypted() {
        assert!(is_age_encrypted("ageenc:YWdl..."));
        assert!(is_age_encrypted("  ageenc:YWdl...  "));
        assert!(!is_age_encrypted("plaintext_token"));
        assert!(!is_age_encrypted("AGE-SECRET-KEY-1..."));
        assert!(!is_age_encrypted(""));
        assert!(!is_age_encrypted("-----BEGIN AGE ENCRYPTED FILE-----"));
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let (secret_key, public_key) = generate_keypair();

        let plaintext = "my-secret-token-value";
        let encrypted = encrypt_value(plaintext, &public_key).unwrap();

        assert!(is_age_encrypted(&encrypted));
        assert!(encrypted.starts_with("ageenc:"));
        assert!(!encrypted.contains('\n'));

        // Write identity to temp file and decrypt
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("age.key");
        write_identity_file(&key_path, &secret_key, &public_key, false).unwrap();

        let decrypted = decrypt_value(&encrypted, &key_path).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_decrypt_wrong_key() {
        let (secret_key1, public_key1) = generate_keypair();
        let (_secret_key2, public_key2) = generate_keypair();

        // Encrypt with key2's public key
        let encrypted = encrypt_value("secret", &public_key2).unwrap();

        // Try to decrypt with key1's private key — should fail
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("age.key");
        write_identity_file(&key_path, &secret_key1, &public_key1, false).unwrap();

        let result = decrypt_value(&encrypted, &key_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_decrypt_invalid_base64() {
        let (secret_key, public_key) = generate_keypair();
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("age.key");
        write_identity_file(&key_path, &secret_key, &public_key, false).unwrap();

        let result = decrypt_value("ageenc:not-valid-base64!!!", &key_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_decrypt_invalid_ciphertext() {
        let (secret_key, public_key) = generate_keypair();
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("age.key");
        write_identity_file(&key_path, &secret_key, &public_key, false).unwrap();

        // Valid base64 but not valid age ciphertext
        let result = decrypt_value("ageenc:aGVsbG8=", &key_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_generate_keypair() {
        let (secret, public) = generate_keypair();
        assert!(secret.starts_with("AGE-SECRET-KEY-"));
        assert!(public.starts_with("age1"));
    }

    #[test]
    fn test_write_identity_file_appends() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("age.key");

        let (secret1, public1) = generate_keypair();
        write_identity_file(&key_path, &secret1, &public1, false).unwrap();

        let (secret2, public2) = generate_keypair();
        write_identity_file(&key_path, &secret2, &public2, false).unwrap();

        let contents = std::fs::read_to_string(&key_path).unwrap();
        let secret_lines: Vec<&str> = contents
            .lines()
            .filter(|l| l.starts_with("AGE-SECRET-KEY-"))
            .collect();
        assert_eq!(secret_lines.len(), 2);
        assert_eq!(secret_lines[0], secret1);
        assert_eq!(secret_lines[1], secret2);

        let pub_lines: Vec<&str> = contents
            .lines()
            .filter(|l| l.starts_with("# public key: "))
            .collect();
        assert_eq!(pub_lines.len(), 2);
    }

    #[test]
    fn test_write_identity_file_force_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("age.key");

        let (secret1, public1) = generate_keypair();
        write_identity_file(&key_path, &secret1, &public1, false).unwrap();

        // Append a second key
        let (_secret2, _public2) = generate_keypair();
        write_identity_file(&key_path, &_secret2, &_public2, false).unwrap();

        // Force overwrite with a third key — should replace everything
        let (secret3, public3) = generate_keypair();
        write_identity_file(&key_path, &secret3, &public3, true).unwrap();

        let contents = std::fs::read_to_string(&key_path).unwrap();
        let secret_lines: Vec<&str> = contents
            .lines()
            .filter(|l| l.starts_with("AGE-SECRET-KEY-"))
            .collect();
        assert_eq!(secret_lines.len(), 1);
        assert_eq!(secret_lines[0], secret3);
        assert!(contents.contains(&public3));
    }
}
