/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::participant::AuditActor;
use crate::runtime_config::{RuntimeConfig, RuntimeConfigStore};
use std::{net::IpAddr, sync::Arc};

/// Builds [`AuditActor`] values with REST PII policy (`record_client_ip`, `ip_anonymize`)
/// applied in one place. Handlers must not construct [`AuditActor::User`] directly.
pub struct AuditActorBuilder {
    runtime_cfg: Arc<RuntimeConfigStore>,
}

impl AuditActorBuilder {
    pub fn new(runtime_cfg: Arc<RuntimeConfigStore>) -> Self {
        Self { runtime_cfg }
    }

    /// REST-authenticated user; client IP is attached only when enabled in live config.
    /// This is the sole entry point for `actor.ip`, so the PII policy lives in one place.
    pub fn rest_user(
        &self,
        username: impl Into<String>,
        role: impl Into<String>,
        client_ip: Option<IpAddr>,
    ) -> AuditActor {
        AuditActor::user(username, Some(role.into()), client_ip.and_then(|ip| self.record_ip(ip)))
    }

    fn record_ip(&self, ip: IpAddr) -> Option<String> {
        let cfg = self.runtime_cfg.get();
        if !cfg.audit_log.record_client_ip {
            return None;
        }
        Some(if cfg.audit_log.ip_anonymize { anonymize_ip(ip) } else { ip.to_string() })
    }
}

/// Parses the client IP from `x-forwarded-for` (first hop) when present and valid.
pub fn client_ip_from_headers(headers: &axum::http::HeaderMap) -> Option<IpAddr> {
    let ip_str = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|v| !v.is_empty())?;
    ip_str.parse().ok()
}

fn anonymize_ip(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            format!("{}.{}.{}.0", o[0], o[1], o[2])
        }
        IpAddr::V6(v6) => {
            let segs = v6.segments();
            format!("{:x}:{:x}:{:x}:0:0:0:0:0", segs[0], segs[1], segs[2])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::app_config::AppConfig;
    use std::collections::HashMap;

    fn test_app_cfg(record_client_ip: bool, ip_anonymize: bool) -> Arc<AppConfig> {
        Arc::new(AppConfig {
            nodes: HashMap::new(),
            wallets: HashMap::new(),
            pools: HashMap::new(),
            bindings: HashMap::new(),
            ton_http_api: Default::default(),
            http: Default::default(),
            elections: None,
            voting: None,
            master_wallet: None,
            tick_interval: 30,
            automation: Default::default(),
            log: None,
            audit_log: common::app_config::AuditLogConfig {
                record_client_ip,
                ip_anonymize,
                ..common::app_config::AuditLogConfig::default()
            },
        })
    }

    fn builder(record_client_ip: bool, ip_anonymize: bool) -> AuditActorBuilder {
        AuditActorBuilder::new(Arc::new(RuntimeConfigStore::from_app_config(test_app_cfg(
            record_client_ip,
            ip_anonymize,
        ))))
    }

    fn user_ip(actor: &AuditActor) -> Option<&str> {
        let AuditActor::User { ip, .. } = actor else {
            panic!("expected user actor");
        };
        ip.as_deref()
    }

    #[test]
    fn service_actor_has_no_ip() {
        // Non-REST actors never carry an IP; they bypass the builder entirely.
        assert_eq!(
            AuditActor::service("http-task"),
            AuditActor::Service { id: "http-task".into() }
        );
    }

    #[test]
    fn rest_user_omits_ip_when_record_disabled() {
        let b = builder(false, true);
        let actor = b.rest_user("alice", "operator", Some("203.0.113.10".parse().unwrap()));
        assert_eq!(user_ip(&actor), None);
    }

    #[test]
    fn rest_user_keeps_ip_when_record_enabled_no_anonymize() {
        let b = builder(true, false);
        let actor = b.rest_user("alice", "operator", Some("203.0.113.10".parse().unwrap()));
        assert_eq!(user_ip(&actor), Some("203.0.113.10"));
    }

    #[test]
    fn rest_user_anonymizes_ipv4_last_octet() {
        let b = builder(true, true);
        let actor = b.rest_user("alice", "operator", Some("203.0.113.10".parse().unwrap()));
        assert_eq!(user_ip(&actor), Some("203.0.113.0"));
    }

    #[test]
    fn rest_user_anonymizes_ipv6_last_segments() {
        let b = builder(true, true);
        let ip: IpAddr = "2001:db8:85a3:8d3:1319:8a2e:370:7348".parse().unwrap();
        let actor = b.rest_user("alice", "operator", Some(ip));
        assert_eq!(user_ip(&actor), Some("2001:db8:85a3:0:0:0:0:0"));
    }

    #[test]
    fn client_ip_from_headers_parses_forwarded_for() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("x-forwarded-for", "10.0.0.5, 10.0.0.6".parse().unwrap());
        assert_eq!(client_ip_from_headers(&headers), Some("10.0.0.5".parse().unwrap()));
    }

    #[test]
    fn client_ip_from_headers_missing_returns_none() {
        assert_eq!(client_ip_from_headers(&axum::http::HeaderMap::new()), None);
    }
}
