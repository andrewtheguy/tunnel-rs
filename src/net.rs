//! Shared networking utilities for tunnel-rs.

use anyhow::{Context, Result};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{lookup_host, TcpStream, UdpSocket};

/// Delay between starting connection attempts (Happy Eyeballs style).
pub const CONNECTION_ATTEMPT_DELAY: Duration = Duration::from_millis(250);

/// Maximum attempts for opening QUIC streams
pub const STREAM_OPEN_MAX_ATTEMPTS: u32 = 3;

/// Base delay for exponential backoff (doubles each attempt)
pub const STREAM_OPEN_BASE_DELAY_MS: u64 = 100;

/// Maximum multiplier for exponential backoff to keep delays bounded.
/// With base delay of 100ms, this caps max delay at ~102 seconds.
pub const BACKOFF_MAX_MULTIPLIER: u64 = 1024;

// ============================================================================
// Address Ordering (Happy Eyeballs)
// ============================================================================

/// Interleave addresses for Happy Eyeballs style connection attempts.
/// Returns addresses with IPv6 preferred first, then alternates IPv4 and IPv6.
/// Preserves original counts and alternates until all addresses are consumed.
///
/// This implements RFC 8305 address sorting for dual-stack connections,
/// giving IPv6 a slight head start while still trying IPv4 quickly.
pub fn interleave_addresses(addrs: &[SocketAddr]) -> Vec<SocketAddr> {
    let (ipv6, ipv4): (Vec<SocketAddr>, Vec<SocketAddr>) =
        addrs.iter().copied().partition(|a| a.is_ipv6());

    let mut ordered = Vec::with_capacity(addrs.len());
    let mut v6_iter = ipv6.into_iter();
    let mut v4_iter = ipv4.into_iter();

    // Interleave addresses: IPv6 first, then alternate
    loop {
        let v6 = v6_iter.next();
        let v4 = v4_iter.next();
        if let Some(addr) = v6 {
            ordered.push(addr);
        }
        if let Some(addr) = v4 {
            ordered.push(addr);
        }
        if v6.is_none() && v4.is_none() {
            break;
        }
    }
    ordered
}

// ============================================================================
// Address Ordering Helpers
// ============================================================================

/// Orders socket addresses by loopback preference.
///
/// If all addresses are loopback (127.x.x.x or ::1), sorts IPv4 before IPv6.
/// This is because most local services bind to 127.0.0.1 only, and macOS
/// resolves "localhost" to ::1 first, causing connection failures or 250ms delays.
///
/// For non-loopback addresses, preserves the original order to allow Happy Eyeballs
/// to work as designed (resolver typically returns IPv6 first per RFC 6724).
pub fn order_by_loopback_preference(addrs: Vec<SocketAddr>) -> Vec<SocketAddr> {
    let is_loopback = addrs.iter().all(|a| a.ip().is_loopback());
    if is_loopback {
        let mut sorted = addrs;
        sorted.sort_by_key(|a| if a.is_ipv4() { 0 } else { 1 });
        sorted
    } else {
        addrs
    }
}

// ============================================================================
// Address Resolution
// ============================================================================

/// Resolve a target address to all available socket addresses.
/// Returns all IPv4 and IPv6 addresses for the hostname.
///
/// For localhost/loopback addresses, IPv4 is preferred because most local services
/// bind to 127.0.0.1 only. This avoids the 250ms Happy Eyeballs delay when IPv6
/// fails on macOS (which returns ::1 before 127.0.0.1 by default).
///
/// For non-local addresses, the resolver's native order is preserved (typically
/// IPv6 first per RFC 6724), allowing Happy Eyeballs to work as designed.
pub async fn resolve_all_target_addrs(target: &str) -> Result<Vec<SocketAddr>> {
    let addrs: Vec<SocketAddr> = lookup_host(target)
        .await
        .with_context(|| format!("Failed to resolve '{}'", target))?
        .collect();
    if addrs.is_empty() {
        anyhow::bail!("No addresses found for host '{}'", target);
    }

    Ok(order_by_loopback_preference(addrs))
}

/// Resolve a listen address to all available socket addresses.
///
/// For localhost/loopback, returns BOTH IPv4 (127.0.0.1) and IPv6 (::1) addresses.
/// This is necessary because dual-stack sockets don't work for loopback addresses -
/// they are distinct addresses requiring separate listeners.
///
/// On macOS, clients connecting to "localhost" try IPv6 (::1) first. If the server
/// only listens on IPv4 (127.0.0.1), connections fail. Binding to both addresses
/// ensures compatibility with all clients regardless of their IPv4/IPv6 preference.
///
/// For non-loopback hostnames, returns a single address (preferring IPv4).
pub async fn resolve_listen_addrs(target: &str) -> Result<Vec<SocketAddr>> {
    // First try direct parse for IP addresses (fast path) - single address
    if let Ok(addr) = target.parse::<SocketAddr>() {
        return Ok(vec![addr]);
    }

    // Resolve hostname
    let addrs: Vec<SocketAddr> = lookup_host(target)
        .await
        .with_context(|| format!("Failed to resolve listen address '{}'", target))?
        .collect();

    if addrs.is_empty() {
        anyhow::bail!("No addresses found for listen address '{}'", target);
    }

    // Check if all resolved addresses are loopback
    let is_loopback = addrs.iter().all(|a| a.ip().is_loopback());

    if is_loopback {
        // For loopback, return ALL addresses (both IPv4 and IPv6) with IPv4 first.
        // Dual-stack sockets don't work for loopback - need separate listeners.
        let mut sorted = order_by_loopback_preference(addrs);
        // Deduplicate (in case resolver returns duplicates)
        sorted.dedup();
        log::debug!(
            "Resolved loopback listen address '{}' to {} addresses: {:?}",
            target,
            sorted.len(),
            sorted
        );
        Ok(sorted)
    } else {
        // For non-loopback, return single address (prefer IPv4)
        let addr = addrs
            .iter()
            .find(|a| a.is_ipv4())
            .or_else(|| addrs.first())
            .copied()
            .expect("no listen addresses available after resolution");
        log::debug!(
            "Resolved listen address '{}' to {} (from {} candidates)",
            target,
            addr,
            addrs.len()
        );
        Ok(vec![addr])
    }
}

// ============================================================================
// Happy Eyeballs TCP Connection
// ============================================================================

/// Try to connect to any of the given addresses using Happy Eyeballs algorithm (RFC 8305).
/// - For non-loopback: Prefers IPv6 addresses (tried first), interleaves with IPv4
/// - For loopback: Prefers IPv4 addresses (most local services bind to 127.0.0.1 only)
/// - Staggers connection attempts with a small delay
/// - Returns first successful connection, cancels remaining attempts
///
/// Note: For loopback addresses, IPv4 is preferred because most local services
/// bind to 127.0.0.1 only. This avoids the 250ms Happy Eyeballs delay when IPv6
/// fails on macOS (which returns ::1 before 127.0.0.1 by default).
pub async fn try_connect_tcp(addrs: &[SocketAddr]) -> Result<TcpStream> {
    use tokio::sync::mpsc;

    if addrs.is_empty() {
        anyhow::bail!("No addresses to connect to");
    }

    // Check if all addresses are loopback
    let is_loopback = addrs.iter().all(|a| a.ip().is_loopback());

    let ordered = if is_loopback {
        // For loopback, prefer IPv4 since most local services bind to 127.0.0.1.
        // This is self-contained and does not depend on caller's ordering.
        let mut sorted = addrs.to_vec();
        sorted.sort_by_key(|a| if a.is_ipv4() { 0 } else { 1 });
        sorted
    } else {
        // For non-loopback, apply Happy Eyeballs: IPv6 first, interleaved with IPv4
        interleave_addresses(addrs)
    };

    // Channel for connection results
    let (tx, mut rx) =
        mpsc::channel::<(SocketAddr, Result<TcpStream, std::io::Error>)>(ordered.len());

    // Spawn staggered connection attempts
    let mut handles = Vec::with_capacity(ordered.len());
    for (i, addr) in ordered.into_iter().enumerate() {
        let tx = tx.clone();
        let delay = CONNECTION_ATTEMPT_DELAY * i as u32;
        handles.push(tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let res = TcpStream::connect(addr).await;
            let _ = tx.send((addr, res)).await;
        }));
    }
    drop(tx);

    // Return the first successful connection
    let mut errors: Vec<String> = Vec::new();
    while let Some((addr, result)) = rx.recv().await {
        match result {
            Ok(stream) => {
                // Cancel outstanding tasks
                for handle in handles {
                    handle.abort();
                }
                return Ok(stream);
            }
            Err(e) => {
                log::debug!("Connection attempt to {} failed: {}", addr, e);
                errors.push(format!("{}: {}", addr, e));
            }
        }
    }

    anyhow::bail!(
        "Failed to connect to any address:\n  {}",
        errors.join("\n  ")
    );
}

// ============================================================================
// UDP address ordering
// ============================================================================

/// Order UDP target addresses for connection attempts (Happy Eyeballs style).
/// This is an alias for [`interleave_addresses`] for API compatibility.
#[inline]
pub fn order_udp_addresses(addrs: &[SocketAddr]) -> Vec<SocketAddr> {
    interleave_addresses(addrs)
}

// ============================================================================
// URL Parsing Helpers
// ============================================================================

/// Extract address (host:port) from a source URL (protocol://host:port).
pub fn extract_addr_from_source(source: &str) -> Option<String> {
    let url = url::Url::parse(source).ok()?;
    let host = url.host_str()?;
    let port = url.port()?;
    // Re-add brackets for IPv6 addresses
    if host.contains(':') {
        Some(format!("[{}]:{}", host, port))
    } else {
        Some(format!("{}:{}", host, port))
    }
}

/// Extract host from a source URL (protocol://host:port).
pub fn extract_host_from_source(source: &str) -> Option<String> {
    let url = url::Url::parse(source).ok()?;
    url.host_str().map(|s| s.to_string())
}

/// Extract port from a source URL (protocol://host:port).
pub fn extract_port_from_source(source: &str) -> Option<u16> {
    url::Url::parse(source).ok()?.port()
}

// ============================================================================
// CIDR Network Validation + checks
// ============================================================================

/// Validate that all entries in allowed_networks are valid CIDR notation.
/// Returns an error with context if any entry fails to parse.
pub fn validate_allowed_networks(allowed_networks: &[String], label: &str) -> Result<()> {
    for network_str in allowed_networks {
        network_str.parse::<ipnet::IpNet>().with_context(|| {
            format!(
                "Invalid CIDR '{}' in {}. Expected format: 192.168.0.0/16 or ::1/128",
                network_str, label
            )
        })?;
    }
    Ok(())
}

/// Result of checking if a source is allowed.
#[derive(Debug)]
pub struct SourceCheckResult {
    /// Whether the source is allowed
    pub allowed: bool,
    /// The resolved IP addresses (if any)
    pub resolved_ips: Vec<std::net::IpAddr>,
    /// Error message if not allowed (includes resolved IPs for debugging)
    pub reason: Option<String>,
}

impl SourceCheckResult {
    /// Format the rejection reason with resolved IPs for detailed error messages.
    pub fn rejection_reason(&self, source: &str, allowed_networks: &[String]) -> String {
        // Use the explicit reason if available (for parse/resolution errors)
        if let Some(ref reason) = self.reason {
            return format!("Source '{}': {}", source, reason);
        }

        if self.resolved_ips.is_empty() {
            format!("Source '{}' could not be resolved or parsed", source)
        } else {
            let ips_str: Vec<String> = self.resolved_ips.iter().map(|ip| ip.to_string()).collect();
            format!(
                "Source '{}' (resolved to {}) not in allowed networks {:?}",
                source,
                ips_str.join(", "),
                allowed_networks
            )
        }
    }
}

/// Check if a source address is allowed by any network in the CIDR list.
/// Returns detailed information about the check including resolved IPs.
///
/// The source format is `protocol://host:port` (e.g., `tcp://192.168.1.100:22` or `tcp://myserver.local:22`).
/// The allowed_networks list contains CIDR notation (e.g., `192.168.0.0/16`, `::1/128`).
pub async fn check_source_allowed(source: &str, allowed_networks: &[String]) -> SourceCheckResult {
    if allowed_networks.is_empty() {
        return SourceCheckResult {
            allowed: true,
            resolved_ips: vec![],
            reason: None,
        };
    }

    // Extract host from source (protocol://host:port)
    let Some(host) = extract_host_from_source(source) else {
        return SourceCheckResult {
            allowed: false,
            resolved_ips: vec![],
            reason: Some("Failed to parse source URL".to_string()),
        };
    };

    // Collect all IPs to check (either the literal IP or all resolved addresses)
    let source_ips: Vec<std::net::IpAddr> = match host.parse::<std::net::IpAddr>() {
        Ok(ip) => vec![ip],
        Err(_) => {
            // Not an IP - try DNS resolution
            let Some(port) = extract_port_from_source(source) else {
                return SourceCheckResult {
                    allowed: false,
                    resolved_ips: vec![],
                    reason: Some("Failed to parse port from source URL".to_string()),
                };
            };
            let lookup_target = format!("{}:{}", host, port);
            let addrs = match lookup_host(&lookup_target).await {
                Ok(addrs) => addrs,
                Err(e) => {
                    return SourceCheckResult {
                        allowed: false,
                        resolved_ips: vec![],
                        reason: Some(format!("DNS resolution failed: {}", e)),
                    };
                }
            };
            let ips: Vec<_> = addrs.map(|a| a.ip()).collect();
            if ips.is_empty() {
                return SourceCheckResult {
                    allowed: false,
                    resolved_ips: vec![],
                    reason: Some("DNS resolution returned no IPs".to_string()),
                };
            }
            ips
        }
    };

    // Parse allowed networks (validated at startup in validate_allowed_networks)
    let mut allowed = false;
    for ip in &source_ips {
        for network_str in allowed_networks {
            if let Ok(network) = network_str.parse::<ipnet::IpNet>() {
                if network.contains(ip) {
                    allowed = true;
                    break;
                }
            }
        }
        if allowed {
            break;
        }
    }

    SourceCheckResult {
        allowed,
        resolved_ips: source_ips,
        reason: None,
    }
}

// ============================================================================
// Exponential backoff helper
// ============================================================================

/// Retry an async operation with exponential backoff.
pub async fn retry_with_backoff<T, E, F, Fut>(
    mut operation: F,
    max_attempts: u32,
    base_delay_ms: u64,
) -> Result<T, E>
where
    F: FnMut(u32) -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    let mut attempt = 0;
    loop {
        attempt += 1;
        match operation(attempt).await {
            Ok(value) => return Ok(value),
            Err(err) => {
                if attempt >= max_attempts {
                    return Err(err);
                }
                let multiplier = 1_u64.checked_shl(attempt.saturating_sub(1)).unwrap_or(u64::MAX);
                let bounded = multiplier.min(BACKOFF_MAX_MULTIPLIER);
                let delay = Duration::from_millis(base_delay_ms.saturating_mul(bounded));
                tokio::time::sleep(delay).await;
            }
        }
    }
}

// ============================================================================
// Stream copy helper
// ============================================================================

/// Buffer size for stream copies (64 KB).
const COPY_BUFFER_SIZE: usize = 64 * 1024;

/// Copy a stream until EOF.
pub async fn copy_stream<R, W>(reader: &mut R, writer: &mut W) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = vec![0u8; COPY_BUFFER_SIZE];
    loop {
        let read_len = reader
            .read(&mut buf)
            .await
            .context("Failed to read from stream")?;
        if read_len == 0 {
            break;
        }
        writer
            .write_all(&buf[..read_len])
            .await
            .context("Failed to write to stream")?;
    }
    writer.flush().await.context("Failed to flush stream")?;
    Ok(())
}

// ============================================================================
// UDP bind helper (kept for iroh compatibility)
// ============================================================================

/// Bind a UDP socket for a set of target addresses, preferring dual-stack.
pub async fn bind_udp_for_targets(target_addrs: &[SocketAddr]) -> Result<UdpSocket> {
    // Prefer IPv6 wildcard if we have any IPv6 targets; it can accept IPv4 via v6-mapped.
    let bind_addr = if target_addrs.iter().any(|addr| addr.is_ipv6()) {
        "[::]:0"
    } else {
        "0.0.0.0:0"
    };

    let socket = UdpSocket::bind(bind_addr)
        .await
        .with_context(|| format!("Failed to bind UDP socket at {}", bind_addr))?;

    Ok(socket)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    #[test]
    fn test_loopback_addresses_prefer_ipv4() {
        // Simulate macOS resolver order: IPv6 first
        let addrs = vec![
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 8080), // ::1
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080), // 127.0.0.1
        ];

        // All loopback, should prefer IPv4
        let is_loopback = addrs.iter().all(|a| a.ip().is_loopback());
        assert!(is_loopback);

        // Use the shared ordering helper
        let result = order_by_loopback_preference(addrs);

        // IPv4 should be first after sorting
        assert!(result[0].is_ipv4(), "IPv4 should be preferred for loopback");
        assert!(result[1].is_ipv6(), "IPv6 should be second for loopback");
    }

    #[test]
    fn test_non_loopback_addresses_preserve_order() {
        // Non-loopback addresses should preserve input order (no sorting)
        let addrs = vec![
            SocketAddr::new(
                IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1)),
                80,
            ),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)), 80),
        ];

        let is_loopback = addrs.iter().all(|a| a.ip().is_loopback());
        assert!(!is_loopback);

        // Use the shared ordering helper
        let result = order_by_loopback_preference(addrs.clone());

        // Order should be preserved exactly (IPv6 still first, as input)
        assert_eq!(
            result, addrs,
            "Non-loopback addresses should preserve input order"
        );
        assert!(result[0].is_ipv6(), "First address should remain IPv6");
        assert!(result[1].is_ipv4(), "Second address should remain IPv4");
    }

    #[test]
    fn test_mixed_loopback_non_loopback_not_treated_as_loopback() {
        // If there's a mix, it's not pure loopback
        let addrs = [
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 8080),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), 8080),
        ];

        let is_loopback = addrs.iter().all(|a| a.ip().is_loopback());
        assert!(!is_loopback);
    }

    // =========================================================================
    // interleave_addresses tests (shared by TCP and UDP Happy Eyeballs)
    // =========================================================================

    #[test]
    fn test_interleave_addresses_empty() {
        let result = interleave_addresses(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_interleave_addresses_only_ipv4() {
        let addrs = vec![
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), 8080),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 8080),
        ];
        let result = interleave_addresses(&addrs);
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|a| a.is_ipv4()));
    }

    #[test]
    fn test_interleave_addresses_only_ipv6() {
        let addrs = vec![
            SocketAddr::new(
                IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1)),
                8080,
            ),
            SocketAddr::new(
                IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 2)),
                8080,
            ),
        ];
        let result = interleave_addresses(&addrs);
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|a| a.is_ipv6()));
    }

    #[test]
    fn test_interleave_addresses_ipv6_first() {
        let v4_1 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), 8080);
        let v4_2 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 8080);
        let v6_1 = SocketAddr::new(
            IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1)),
            8080,
        );
        let v6_2 = SocketAddr::new(
            IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 2)),
            8080,
        );

        // Input: v4, v4, v6, v6
        let addrs = vec![v4_1, v4_2, v6_1, v6_2];
        let result = interleave_addresses(&addrs);

        // Expected: v6, v4, v6, v4 (interleaved with IPv6 first)
        assert_eq!(result.len(), 4);
        assert!(result[0].is_ipv6(), "First should be IPv6");
        assert!(result[1].is_ipv4(), "Second should be IPv4");
        assert!(result[2].is_ipv6(), "Third should be IPv6");
        assert!(result[3].is_ipv4(), "Fourth should be IPv4");
    }

    #[test]
    fn test_interleave_addresses_unequal_counts() {
        let v4_1 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), 8080);
        let v6_1 = SocketAddr::new(
            IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1)),
            8080,
        );
        let v6_2 = SocketAddr::new(
            IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 2)),
            8080,
        );
        let v6_3 = SocketAddr::new(
            IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 3)),
            8080,
        );

        // Input: 1 IPv4, 3 IPv6
        let addrs = vec![v4_1, v6_1, v6_2, v6_3];
        let result = interleave_addresses(&addrs);

        // Expected: v6, v4, v6, v6 (all addresses consumed)
        assert_eq!(result.len(), 4);
        assert!(result[0].is_ipv6(), "First should be IPv6");
        assert!(result[1].is_ipv4(), "Second should be IPv4");
        assert!(result[2].is_ipv6(), "Third should be IPv6");
        assert!(result[3].is_ipv6(), "Fourth should be IPv6");
    }

    #[test]
    fn test_interleave_addresses_preserves_all() {
        let v4_1 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), 8080);
        let v4_2 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 8081);
        let v6_1 = SocketAddr::new(
            IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1)),
            8082,
        );

        let addrs = vec![v4_1, v4_2, v6_1];
        let result = interleave_addresses(&addrs);

        // All original addresses should be present
        assert_eq!(result.len(), 3);
        assert!(result.contains(&v4_1));
        assert!(result.contains(&v4_2));
        assert!(result.contains(&v6_1));
    }

    #[test]
    fn test_order_udp_addresses_is_alias() {
        // Verify order_udp_addresses returns same result as interleave_addresses
        let addrs = vec![
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), 8080),
            SocketAddr::new(
                IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1)),
                8080,
            ),
        ];
        assert_eq!(order_udp_addresses(&addrs), interleave_addresses(&addrs));
    }
}
