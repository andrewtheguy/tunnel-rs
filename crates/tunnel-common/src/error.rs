use std::fmt;

/// Error category for tunnel-rs errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    /// Configuration or usage error — do not retry.
    Config,
    /// Authentication failure — do not retry (wrong credentials).
    Auth,
    /// Network/connection error — safe to retry.
    Connection,
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
}

impl fmt::Display for TunnelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.source)
    }
}

impl std::error::Error for TunnelError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&*self.source)
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
    }

    #[test]
    fn test_downcast_from_anyhow() {
        let err: anyhow::Error = TunnelError::auth(anyhow::anyhow!("bad token")).into();
        let tunnel_err = err.downcast_ref::<TunnelError>().unwrap();
        assert_eq!(tunnel_err.category, ErrorCategory::Auth);
    }

    #[test]
    fn test_display() {
        let err = TunnelError::config(anyhow::anyhow!("missing --source"));
        assert_eq!(err.to_string(), "missing --source");
    }
}
