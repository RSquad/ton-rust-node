/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use anyhow::Context;
use std::net::SocketAddr;

/// Resolve `addr` to a `SocketAddr`.
///
/// Accepts either a literal `SocketAddr` (e.g. `"127.0.0.1:8080"`,
/// `"[::1]:8080"`) or a `host:port` string that is resolved via
/// `tokio::net::lookup_host`. Returns the first resolved address.
///
/// Unlike `std::net::ToSocketAddrs::to_socket_addrs`, this function does not
/// block the calling tokio worker on a slow DNS server: name resolution is
/// dispatched to the blocking thread pool by tokio.
pub async fn resolve_ip(addr: &str) -> anyhow::Result<SocketAddr> {
    if let Ok(socket_addr) = addr.parse::<SocketAddr>() {
        return Ok(socket_addr);
    }

    tokio::net::lookup_host(addr)
        .await
        .with_context(|| format!("failed to resolve address: {addr}"))?
        .next()
        .with_context(|| format!("resolver returned no addresses for: {addr}"))
}

#[cfg(test)]
mod tests {
    use super::resolve_ip;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

    #[tokio::test]
    async fn parses_ipv4_socket_addr() {
        let addr = resolve_ip("127.0.0.1:8080").await.unwrap();
        assert_eq!(addr, SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080));
    }

    #[tokio::test]
    async fn parses_ipv6_socket_addr() {
        let addr = resolve_ip("[::1]:8080").await.unwrap();
        assert_eq!(addr, SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 8080));
    }

    #[tokio::test]
    async fn resolves_localhost() {
        let addr = resolve_ip("localhost:8080").await.unwrap();
        assert_eq!(addr.port(), 8080);
        assert!(addr.ip().is_loopback());
    }

    #[tokio::test]
    async fn fails_when_port_is_missing() {
        let err = resolve_ip("127.0.0.1").await.unwrap_err();

        let io_kind =
            err.chain().find_map(|cause| cause.downcast_ref::<std::io::Error>()).map(|e| e.kind());

        assert!(
            matches!(
                io_kind,
                Some(std::io::ErrorKind::InvalidInput) | Some(std::io::ErrorKind::AddrNotAvailable)
            ),
            "unexpected error chain: {err:#}"
        );
    }

    #[tokio::test]
    async fn fails_on_invalid_port() {
        assert!(resolve_ip("127.0.0.1:notaport").await.is_err());
        assert!(resolve_ip("localhost:notaport").await.is_err());
    }

    #[tokio::test]
    async fn fails_on_invalid_host() {
        assert!(resolve_ip("not a host:8080").await.is_err());
    }

    #[tokio::test]
    async fn fails_on_empty_input() {
        assert!(resolve_ip("").await.is_err());
    }

    #[tokio::test]
    async fn resolves_real_domain_google_com() {
        let addr = resolve_ip("google.com:443").await.unwrap();

        assert_eq!(addr.port(), 443);
        assert!(!addr.ip().is_unspecified(), "resolved to unspecified IP: {addr}");
    }
}
