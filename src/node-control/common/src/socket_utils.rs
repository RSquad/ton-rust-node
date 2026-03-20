/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use anyhow::Context;
use std::net::{SocketAddr, ToSocketAddrs};

pub fn resolve_ip(addr: &str) -> anyhow::Result<SocketAddr> {
    if let Ok(socket_addr) = addr.parse::<SocketAddr>() {
        return Ok(socket_addr);
    }

    let mut resolved =
        addr.to_socket_addrs().with_context(|| format!("failed to resolve address: {addr}"))?;

    resolved.next().with_context(|| format!("resolver returned no addresses for: {addr}"))
}

#[cfg(test)]
mod tests {
    use super::resolve_ip;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

    #[test]
    fn parses_ipv4_socket_addr() {
        let addr = resolve_ip("127.0.0.1:8080").unwrap();
        assert_eq!(addr, SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080));
    }

    #[test]
    fn parses_ipv6_socket_addr() {
        let addr = resolve_ip("[::1]:8080").unwrap();
        assert_eq!(addr, SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 8080));
    }

    #[test]
    fn resolves_localhost() {
        let addr = resolve_ip("localhost:8080").unwrap();
        assert_eq!(addr.port(), 8080);
        assert!(addr.ip().is_loopback());
    }

    #[test]
    fn fails_when_port_is_missing() {
        let err = resolve_ip("127.0.0.1").unwrap_err();

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

    #[test]
    fn fails_on_invalid_port() {
        assert!(resolve_ip("127.0.0.1:notaport").is_err());
        assert!(resolve_ip("localhost:notaport").is_err());
    }

    #[test]
    fn fails_on_invalid_host() {
        assert!(resolve_ip("not a host:8080").is_err());
    }

    #[test]
    fn fails_on_empty_input() {
        assert!(resolve_ip("").is_err());
    }

    #[test]
    fn resolves_real_domain_google_com() {
        let addr = resolve_ip("google.com:443").unwrap();

        assert_eq!(addr.port(), 443);
        assert!(!addr.ip().is_unspecified(), "resolved to unspecified IP: {addr}");
    }
}
