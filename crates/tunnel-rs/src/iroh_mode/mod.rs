//! Iroh-based networking components.
//!
//! This module provides:
//! - `endpoint`: Iroh endpoint creation and connection helpers
//! - `multi_source`: Multi-source mode with Iroh relays (client requests source)
//! - `helpers`: Shared stream and connection helpers (internal)

pub mod endpoint;
mod helpers;
mod multi_source;

// Re-export public API
pub use multi_source::{
    run_multi_source_client, run_multi_source_server, MultiSourceClientConfig,
    MultiSourceServerConfig,
};
