//! Secret key generation and management commands (iroh).

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use iroh::SecretKey;
use log::info;
use serde::Serialize;
use std::path::Path;
use std::time::SystemTime;

use crate::auth::{report_public_half, rfc3339_utc};
use crate::iroh_mode::endpoint::{load_secret, secret_to_endpoint_id};

#[derive(Serialize)]
struct GeneratedServerKey {
    public_key: String,
    private_key: String,
}

/// The server's public EndpointId, as printed by `show-server-id --json`.
#[derive(Serialize)]
struct ServerId {
    public_key: String,
}

fn generate_server_key() -> GeneratedServerKey {
    let secret = SecretKey::generate();
    GeneratedServerKey {
        public_key: secret_to_endpoint_id(&secret).to_string(),
        private_key: BASE64.encode(secret.to_bytes()),
    }
}

/// Render the secret key file for a freshly generated server key.
///
/// The headers mirror the client authentication key file, so `head` on either
/// one shows which identity the key belongs to and when it was created. Key
/// loading skips `#` lines, so the file stays usable as an inline `secret`.
fn secret_key_file(generated: &GeneratedServerKey) -> String {
    format!(
        "# created: {}\n# EndpointId: {}\n{}\n",
        rfc3339_utc(SystemTime::now()),
        generated.public_key,
        generated.private_key
    )
}

/// Write the secret key file to `path`, owner-only (`0600` on Unix).
fn write_secret_file(path: &Path, secret_content: &str, force: bool) -> Result<()> {
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
            .context("Failed to open secret key file")?;
        // mode() only applies on creation; explicitly set perms for existing files
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .context("Failed to set secret key file permissions")?;
        file.write_all(secret_content.as_bytes())
            .context("Failed to write secret key file")?;
    }

    #[cfg(not(unix))]
    {
        std::fs::write(path, secret_content).context("Failed to write secret key file")?;
    }

    Ok(())
}

/// Generate a server secret key, writing it to `output` (or to stdout when
/// `output` is `None` or `-`) and reporting the matching EndpointId.
///
/// With a file destination the key file lands on disk with `0600` permissions
/// and the EndpointId goes to stdout. Without one the whole key file goes to
/// stdout and the EndpointId to stderr, so `generate-server-key > server.key`
/// works and still shows the id — the same split as `generate-auth-key`. On a
/// terminal the stderr line is dropped; see [`report_public_half`].
pub fn generate_secret(output: Option<&Path>, force: bool) -> Result<()> {
    let generated = generate_server_key();
    let key_file = secret_key_file(&generated);
    let public_info = format!("EndpointId: {}", generated.public_key);

    let Some(path) = output.filter(|path| path.as_os_str() != "-") else {
        print!("{}", key_file);
        report_public_half(&public_info);
        return Ok(());
    };

    write_secret_file(path, &key_file, force)?;
    info!("Secret key saved to: {}", path.display());
    println!("{}", public_info);
    Ok(())
}

/// Generate a server keypair and print both keys as a JSON object.
pub fn generate_secret_json() -> Result<()> {
    println!("{}", serde_json::to_string(&generate_server_key())?);
    Ok(())
}

/// Show the EndpointId for an existing secret key file
pub fn show_id(secret_file: &Path, json: bool) -> Result<()> {
    let secret = load_secret(secret_file)?;
    let public_key = secret_to_endpoint_id(&secret).to_string();
    if json {
        println!("{}", serde_json::to_string(&ServerId { public_key })?);
    } else {
        println!("{}", public_key);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iroh_mode::endpoint::load_secret_from_string;

    #[test]
    fn generated_server_key_contains_matching_public_and_private_keys() {
        let generated = generate_server_key();
        let secret = load_secret_from_string(&generated.private_key).unwrap();

        assert_eq!(generated.public_key, secret_to_endpoint_id(&secret).to_string());
    }

    #[test]
    fn generated_server_key_serializes_with_public_and_private_keys() {
        let value = serde_json::to_value(generate_server_key()).unwrap();

        assert!(value["public_key"].is_string());
        assert!(value["private_key"].is_string());
    }

    #[test]
    fn generated_key_file_headers_precede_the_key_and_name_the_endpoint_id() {
        let generated = generate_server_key();
        let file = secret_key_file(&generated);
        let lines: Vec<&str> = file.lines().collect();

        assert!(lines[0].starts_with("# created: "));
        assert_eq!(lines[1], format!("# EndpointId: {}", generated.public_key));
        assert_eq!(lines[2], generated.private_key);
        // The commented file still loads as a key, so it works as an inline secret.
        let secret = load_secret_from_string(&file).unwrap();
        assert_eq!(secret_to_endpoint_id(&secret).to_string(), generated.public_key);
    }

    #[test]
    fn server_id_serializes_with_the_public_key() {
        let value = serde_json::to_value(ServerId {
            public_key: "abc".to_string(),
        })
        .unwrap();

        assert_eq!(value["public_key"], "abc");
    }
}
