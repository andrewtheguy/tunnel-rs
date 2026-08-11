use std::fmt;

/// Error category for tunnel-rs errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    /// Configuration or usage error — do not retry.
    Config,
    /// Authentication failure — do not retry (wrong credentials).
    Auth,
    /// Connection establishment failed — retry only if it worked before.
    Connection,
    /// Connection lost after tunnel was established — always retry.
    ConnectionLost,
}

/// An error wrapper that carries both an `anyhow::Error` and an error category.
#[derive(Debug)]
pub struct TunnelError {
    pub category: ErrorCategory,
    pub source: anyhow::Error,
}

impl TunnelError {
    pub fn config(err: impl Into<anyhow::Error>) -> Self {
        Self {
            category: ErrorCategory::Config,
            source: err.into(),
        }
    }

    pub fn auth(err: impl Into<anyhow::Error>) -> Self {
        Self {
            category: ErrorCategory::Auth,
            source: err.into(),
        }
    }

    pub fn connection(err: impl Into<anyhow::Error>) -> Self {
        Self {
            category: ErrorCategory::Connection,
            source: err.into(),
        }
    }

    pub fn connection_lost(err: impl Into<anyhow::Error>) -> Self {
        Self {
            category: ErrorCategory::ConnectionLost,
            source: err.into(),
        }
    }
}

impl fmt::Display for TunnelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.source)
    }
}

impl std::error::Error for TunnelError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        // `Display` already renders the top of `self.source`'s message chain, so
        // expose only the remainder as the source. Returning `self.source` itself
        // would duplicate its top message when formatted with `{:#}`.
        self.source.source()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_categories() {
        assert_eq!(
            TunnelError::config(anyhow::anyhow!("test")).category,
            ErrorCategory::Config
        );
        assert_eq!(
            TunnelError::auth(anyhow::anyhow!("test")).category,
            ErrorCategory::Auth
        );
        assert_eq!(
            TunnelError::connection(anyhow::anyhow!("test")).category,
            ErrorCategory::Connection
        );
        assert_eq!(
            TunnelError::connection_lost(anyhow::anyhow!("test")).category,
            ErrorCategory::ConnectionLost
        );
    }

    #[test]
    fn test_downcast_from_anyhow() {
        let err: anyhow::Error = TunnelError::auth(anyhow::anyhow!("bad signature")).into();
        let tunnel_err = err.downcast_ref::<TunnelError>().unwrap();
        assert_eq!(tunnel_err.category, ErrorCategory::Auth);
    }

    #[test]
    fn test_display() {
        let err = TunnelError::config(anyhow::anyhow!("missing --source"));
        assert_eq!(err.to_string(), "missing --source");
    }

    #[test]
    fn test_alternate_format_does_not_duplicate_message() {
        // A leaf error with no further source must render exactly once under `{:#}`.
        let err: anyhow::Error =
            TunnelError::config(anyhow::anyhow!("Private key is required.")).into();
        assert_eq!(format!("{:#}", err), "Private key is required.");
    }

    #[test]
    fn test_alternate_format_preserves_context_chain() {
        // Context added on the inner error is still surfaced exactly once each.
        use anyhow::Context;
        let inner = Err::<(), _>(anyhow::anyhow!("invalid base64"))
            .context("Invalid authentication key format")
            .unwrap_err();
        let err: anyhow::Error = TunnelError::config(inner).into();
        assert_eq!(
            format!("{:#}", err),
            "Invalid authentication key format: invalid base64"
        );
    }
}
