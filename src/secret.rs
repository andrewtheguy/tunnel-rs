//! Secret key generation and management commands (iroh).

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use iroh::SecretKey;
use log::info;
use serde::Serialize;
use std::path::PathBuf;

use crate::iroh_mode::endpoint::{load_secret, secret_to_endpoint_id};

#[derive(Serialize)]
struct GeneratedServerKey {
    public_key: String,
    private_key: String,
}

fn generate_server_key() -> GeneratedServerKey {
    let secret = SecretKey::generate();
    GeneratedServerKey {
        public_key: secret_to_endpoint_id(&secret).to_string(),
        private_key: BASE64.encode(secret.to_bytes()),
    }
}

fn write_secret_to_output(
    output: &PathBuf,
    secret_content: &str,
    public_info: &str,
    force: bool,
    secret_label: &str,
) -> Result<()> {
    if output.as_os_str() == std::ffi::OsStr::new("-") {
        println!("{}", secret_content);
        eprintln!("{}", public_info);
        return Ok(());
    }

    if output.exists() && !force {
        anyhow::bail!(
            "File already exists: {}. Use --force to overwrite.",
            output.display()
        );
    }

    if let Some(parent) = output.parent() {
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
            .open(output)
            .context("Failed to open secret key file")?;
        file.write_all(secret_content.as_bytes())
            .context("Failed to write secret key file")?;
        // mode() only applies on creation; explicitly set perms for existing files
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .context("Failed to set secret key file permissions")?;
    }

    #[cfg(not(unix))]
    {
        std::fs::write(output, secret_content).context("Failed to write secret key file")?;
    }

    info!("{} saved to: {}", secret_label, output.display());
    println!("{}", public_info);

    Ok(())
}

/// Generate a new secret key file (base64 encoded) and output the EndpointId to stdout
pub fn generate_secret(output: PathBuf, force: bool) -> Result<()> {
    let generated = generate_server_key();
    write_secret_to_output(
        &output,
        &generated.private_key,
        &format!("EndpointId: {}", generated.public_key),
        force,
        "Secret key",
    )
}

/// Generate a server keypair and print both keys as a JSON object.
pub fn generate_secret_json() -> Result<()> {
    println!("{}", serde_json::to_string(&generate_server_key())?);
    Ok(())
}

/// Show the EndpointId for an existing secret key file
pub fn show_id(secret_file: PathBuf) -> Result<()> {
    let secret = load_secret(&secret_file)?;
    let endpoint_id = secret_to_endpoint_id(&secret);
    println!("{}", endpoint_id);
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
}
