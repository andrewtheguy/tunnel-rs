//! tunnel-rs Ed25519 challenge-response authentication.
//!
//! The app-independent key format, files, and raw signing primitives live in
//! `flexaccess-keys`. This module owns only tunnel-rs's domain-separated
//! challenge transcript and its authorization decision.

use std::io::IsTerminal;
use std::path::Path;
use std::time::SystemTime;

use anyhow::Result;
use flexaccess_keys::{PrivateKey, PublicKey};

pub const CHALLENGE_LENGTH: usize = 32;
pub const PUBLIC_KEY_LENGTH: usize = flexaccess_keys::KEY_LENGTH;
pub const SIGNATURE_LENGTH: usize = flexaccess_keys::SIGNATURE_LENGTH;

const AUTH_DOMAIN: &[u8] = b"tunnel-rs public-key authentication v1\0";

pub type Challenge = [u8; CHALLENGE_LENGTH];
pub type PublicKeyBytes = [u8; PUBLIC_KEY_LENGTH];
pub type SignatureBytes = [u8; SIGNATURE_LENGTH];

/// Ed25519 public keys accepted by the server.
#[derive(Clone, Default, Debug)]
pub struct AuthorizedKeys(flexaccess_keys::AuthorizedKeys);

impl AuthorizedKeys {
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
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
        let public_key = PublicKey::from_bytes(public_key)?;

        let Some(comment) = self.0.comment(&public_key) else {
            return Ok(None);
        };
        if public_key.verify(&signed_message(challenge), &signature) {
            Ok(Some(comment))
        } else {
            Ok(None)
        }
    }
}

/// Client signing key using the shared key format and tunnel-rs transcript.
#[derive(Clone)]
pub struct ClientAuthKey(PrivateKey);

impl ClientAuthKey {
    pub fn public_key(&self) -> PublicKeyBytes {
        self.0.public_key().to_bytes()
    }

    pub fn sign_challenge(&self, challenge: &Challenge) -> SignatureBytes {
        self.0.sign(&signed_message(challenge))
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

pub fn load_authorized_keys(path: &Path) -> Result<AuthorizedKeys> {
    Ok(AuthorizedKeys(flexaccess_keys::load_authorized_keys(path)?))
}

pub fn parse_authorized_keys_entries(entries: &[String], origin: &str) -> Result<AuthorizedKeys> {
    Ok(AuthorizedKeys(
        flexaccess_keys::parse_authorized_key_entries(entries, origin)?,
    ))
}

pub fn load_private_key(path: &Path) -> Result<ClientAuthKey> {
    Ok(ClientAuthKey(flexaccess_keys::load_private_key(path)?))
}

pub fn parse_private_key(content: &str, origin: &str) -> Result<ClientAuthKey> {
    Ok(ClientAuthKey(flexaccess_keys::parse_private_key(
        content, origin,
    )?))
}

pub fn generate_auth_key(
    output: Option<&Path>,
    force: bool,
    comment: &str,
    json: bool,
) -> Result<()> {
    Ok(flexaccess_keys::generate_auth_key_command(
        output, force, comment, json,
    )?)
}

pub fn show_auth_key(path: &Path, comment: Option<&str>, json: bool) -> Result<()> {
    Ok(flexaccess_keys::show_auth_key_command(
        path, comment, json,
    )?)
}

/// Report a generated server identity's public half on stderr unless stdout is
/// a terminal.
pub fn report_public_half(line: &str) {
    if !std::io::stdout().is_terminal() {
        eprintln!("{}", line);
    }
}

pub fn rfc3339_utc(time: SystemTime) -> String {
    flexaccess_keys::rfc3339_utc(time)
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
    use std::time::UNIX_EPOCH;

    use tempfile::NamedTempFile;

    use super::*;

    fn client(seed: u8) -> ClientAuthKey {
        ClientAuthKey(PrivateKey::from_seed([seed; PUBLIC_KEY_LENGTH]))
    }

    fn private_key_file(seed: u8, comment: &str) -> NamedTempFile {
        let mut file = NamedTempFile::new().unwrap();
        let content = PrivateKey::from_seed([seed; PUBLIC_KEY_LENGTH])
            .to_key_file(comment, UNIX_EPOCH)
            .unwrap();
        file.write_all(content.as_bytes()).unwrap();
        file
    }

    fn authorized_keys_file(seed: u8, comment: &str) -> NamedTempFile {
        let mut file = NamedTempFile::new().unwrap();
        let key = PrivateKey::from_seed([seed; PUBLIC_KEY_LENGTH]);
        writeln!(file, "# tunnel-rs clients").unwrap();
        writeln!(file).unwrap();
        writeln!(file, "{}", key.authorized_key(comment).unwrap()).unwrap();
        file
    }

    #[test]
    fn shared_key_files_authenticate_with_the_tunnel_transcript() {
        let private_file = private_key_file(42, "alice laptop");
        let authorized_file = authorized_keys_file(42, "alice laptop");
        let private_key = load_private_key(private_file.path()).unwrap();
        let authorized_keys = load_authorized_keys(authorized_file.path()).unwrap();
        let challenge = [7; CHALLENGE_LENGTH];
        let signature = private_key.sign_challenge(&challenge);

        assert_eq!(
            authorized_keys
                .verify_proof(&challenge, &private_key.public_key(), &signature)
                .unwrap(),
            Some("alice laptop")
        );
    }

    #[test]
    fn inline_key_file_and_entries_use_the_shared_parser() {
        let key = PrivateKey::from_seed([42; PUBLIC_KEY_LENGTH]);
        let key_file = key.to_key_file("alice laptop", UNIX_EPOCH).unwrap();
        let entry = key.authorized_key("alice laptop").unwrap().to_string();
        let private_key = parse_private_key(&key_file, "[iroh].private_key").unwrap();
        let authorized_keys =
            parse_authorized_keys_entries(&[entry], "[iroh].authorized_keys").unwrap();
        let challenge = [11; CHALLENGE_LENGTH];
        let signature = private_key.sign_challenge(&challenge);

        assert_eq!(authorized_keys.len(), 1);
        assert_eq!(
            authorized_keys
                .verify_proof(&challenge, &private_key.public_key(), &signature)
                .unwrap(),
            Some("alice laptop")
        );
    }

    #[test]
    fn errors_retain_inline_config_origins() {
        let error = parse_authorized_keys_entries(
            &["ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQC comment".to_string()],
            "[iroh].authorized_keys",
        )
        .unwrap_err();
        assert!(error.to_string().contains("[iroh].authorized_keys:1"));

        let error = parse_private_key("not-a-key", "[iroh].private_key").unwrap_err();
        assert!(error.to_string().contains("[iroh].private_key"));
    }

    #[test]
    fn rejects_signature_for_another_challenge() {
        let private_key = client(42);
        let authorized_file = authorized_keys_file(42, "alice");
        let authorized_keys = load_authorized_keys(authorized_file.path()).unwrap();
        let signature = private_key.sign_challenge(&[1; CHALLENGE_LENGTH]);

        assert!(
            authorized_keys
                .verify_proof(&[2; CHALLENGE_LENGTH], &private_key.public_key(), &signature)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn rejects_proof_from_an_unauthorized_key() {
        let authorized_file = authorized_keys_file(42, "alice");
        let authorized_keys = load_authorized_keys(authorized_file.path()).unwrap();
        let unauthorized_key = client(99);
        let challenge = [3; CHALLENGE_LENGTH];
        let signature = unauthorized_key.sign_challenge(&challenge);

        assert!(
            authorized_keys
                .verify_proof(&challenge, &unauthorized_key.public_key(), &signature)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn rejects_malformed_proof_lengths() {
        let authorized_file = authorized_keys_file(42, "alice");
        let authorized_keys = load_authorized_keys(authorized_file.path()).unwrap();
        let challenge = [0; CHALLENGE_LENGTH];

        assert!(
            authorized_keys
                .verify_proof(&challenge, &[0; 31], &[0; 64])
                .is_err()
        );
        assert!(
            authorized_keys
                .verify_proof(&challenge, &[0; 32], &[0; 63])
                .is_err()
        );
    }

    #[test]
    fn empty_authorized_keys_file_is_reported_to_the_call_site() {
        let file = NamedTempFile::new().unwrap();
        assert!(load_authorized_keys(file.path()).unwrap().is_empty());
    }

    #[test]
    fn formats_server_key_timestamps_through_the_shared_crate() {
        assert_eq!(rfc3339_utc(UNIX_EPOCH), "1970-01-01T00:00:00Z");
    }
}
