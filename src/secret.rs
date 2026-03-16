//! Secret key generation and management commands (iroh).

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use iroh::SecretKey;
use log::info;
use std::path::PathBuf;

use crate::iroh_mode::endpoint::{load_secret, secret_to_endpoint_id};

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
        use std::os::unix::fs::OpenOptionsExt;

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(output)
            .context("Failed to open secret key file")?;
        file.write_all(secret_content.as_bytes())
            .context("Failed to write secret key file")?;
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
    let secret = SecretKey::generate(&mut rand::rng());
    let secret_base64 = BASE64.encode(secret.to_bytes());
    let endpoint_id = secret_to_endpoint_id(&secret);
    write_secret_to_output(
        &output,
        &secret_base64,
        &format!("EndpointId: {}", endpoint_id),
        force,
        "Secret key",
    )
}

/// Show the EndpointId for an existing secret key file
pub fn show_id(secret_file: PathBuf) -> Result<()> {
    let secret = load_secret(&secret_file)?;
    let endpoint_id = secret_to_endpoint_id(&secret);
    println!("{}", endpoint_id);
    Ok(())
}
