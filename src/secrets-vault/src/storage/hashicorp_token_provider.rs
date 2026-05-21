/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    errors::error::VaultError,
    memory::protected_memory::{ProtectedMemory, ProtectedMemoryInner},
};
use std::sync::Arc;
use zeroize::Zeroize;

/// Authentication configuration for HashiCorp Vault.
pub enum AuthConfig {
    /// Static Vault token (existing behavior).
    StaticToken(ProtectedMemory),
    /// Kubernetes service account auth.
    Kubernetes {
        role: String,
        /// Auth backend mount path (default: `"kubernetes"`).
        mount_path: String,
        /// Path to the projected service account JWT
        /// (default: `/var/run/secrets/kubernetes.io/serviceaccount/token`).
        jwt_path: String,
    },
}

#[async_trait::async_trait]
pub(crate) trait TokenProvider: Send + Sync {
    /// Returns a valid Vault token, performing login/renewal if necessary.
    async fn token(&self) -> anyhow::Result<ProtectedMemory>;
}

pub(crate) struct StaticTokenProvider {
    inner: ProtectedMemory,
}

impl StaticTokenProvider {
    pub fn new(token: ProtectedMemory) -> Self {
        Self { inner: token }
    }
}

#[async_trait::async_trait]
impl TokenProvider for StaticTokenProvider {
    async fn token(&self) -> anyhow::Result<ProtectedMemory> {
        self.inner.try_clone()
    }
}

struct CachedToken {
    token: ProtectedMemory,
    lease_duration_secs: u64,
    obtained_at: std::time::Instant,
}

impl CachedToken {
    fn is_expired(&self) -> bool {
        if self.lease_duration_secs == 0 {
            return false; // TTL=0 means the token never expires
        }
        let elapsed = self.obtained_at.elapsed().as_secs();
        // Refresh proactively when 75% of the TTL has elapsed.
        elapsed >= self.lease_duration_secs * 3 / 4
    }
}

pub(crate) struct KubernetesTokenProvider {
    http_client: reqwest::Client,
    vault_addr: String,
    role: String,
    mount_path: String,
    jwt_path: String,
    cache: tokio::sync::Mutex<Option<CachedToken>>,
}

#[derive(serde::Deserialize)]
struct LoginResponse {
    auth: LoginAuth,
}

#[derive(serde::Deserialize)]
struct LoginAuth {
    client_token: String,
    lease_duration: u64,
}

impl KubernetesTokenProvider {
    pub fn new(
        http_client: reqwest::Client,
        vault_addr: String,
        role: String,
        mount_path: String,
        jwt_path: String,
    ) -> Self {
        Self {
            http_client,
            vault_addr,
            role,
            mount_path,
            jwt_path,
            cache: tokio::sync::Mutex::new(None),
        }
    }

    /// Authenticates against Vault's Kubernetes auth backend and returns a
    /// fresh cached token.
    async fn login(&self) -> anyhow::Result<CachedToken> {
        let jwt = tokio::fs::read_to_string(&self.jwt_path).await.map_err(|e| {
            VaultError::backend_auth_failed(format!(
                "failed to read service account JWT from '{}': {e}",
                self.jwt_path
            ))
        })?;

        let url = format!("{}/auth/{}/login", self.vault_addr, self.mount_path);
        let payload = serde_json::json!({
            "jwt": jwt,
            "role": self.role,
        });

        let response = self.http_client.post(&url).json(&payload).send().await.map_err(|e| {
            VaultError::backend_auth_failed(format!("kubernetes login failed: {e}"))
        })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!(VaultError::backend_auth_failed(format!(
                "kubernetes login returned {status}: {body}"
            )));
        }

        let mut login: LoginResponse = response.json().await.map_err(|e| {
            VaultError::backend_auth_failed(format!("failed to parse login response: {e}"))
        })?;

        let token_pm = ProtectedMemoryInner::from_slice(login.auth.client_token.as_bytes())?.into();
        login.auth.client_token.zeroize();

        Ok(CachedToken {
            token: token_pm,
            lease_duration_secs: login.auth.lease_duration,
            obtained_at: std::time::Instant::now(),
        })
    }
}

#[async_trait::async_trait]
impl TokenProvider for KubernetesTokenProvider {
    async fn token(&self) -> anyhow::Result<ProtectedMemory> {
        let mut cache = self.cache.lock().await;
        if let Some(cached) = &*cache {
            if !cached.is_expired() {
                return cached.token.try_clone();
            }
        }
        let fresh = self.login().await?;
        let result = fresh.token.try_clone();
        *cache = Some(fresh);
        result
    }
}

/// Creates a new token provider based on the authentication configuration.
pub(crate) fn create_token_provider(
    auth: AuthConfig,
    http_client: reqwest::Client,
    vault_addr: &str,
) -> Arc<dyn TokenProvider> {
    match auth {
        AuthConfig::StaticToken(token) => Arc::new(StaticTokenProvider::new(token)),
        AuthConfig::Kubernetes { role, mount_path, jwt_path } => {
            Arc::new(KubernetesTokenProvider::new(
                http_client,
                vault_addr.to_string(),
                role,
                mount_path,
                jwt_path,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn static_provider_returns_token() {
        let pm: ProtectedMemory = ProtectedMemoryInner::from_slice(b"test-token").unwrap().into();
        let provider = StaticTokenProvider::new(pm);

        let t = provider.token().await.unwrap();
        assert_eq!(&*t.lock().unwrap(), b"test-token");
    }

    #[tokio::test]
    async fn cached_token_expiry() {
        let cached = CachedToken {
            token: ProtectedMemoryInner::from_slice(b"tok").unwrap().into(),
            lease_duration_secs: 100,
            obtained_at: std::time::Instant::now() - std::time::Duration::from_secs(80),
        };
        assert!(cached.is_expired()); // 80% elapsed > 75% threshold

        let fresh = CachedToken {
            token: ProtectedMemoryInner::from_slice(b"tok").unwrap().into(),
            lease_duration_secs: 100,
            obtained_at: std::time::Instant::now() - std::time::Duration::from_secs(50),
        };
        assert!(!fresh.is_expired()); // 50% elapsed < 75% threshold
    }

    #[tokio::test]
    async fn cached_token_zero_ttl_never_expires() {
        let cached = CachedToken {
            token: ProtectedMemoryInner::from_slice(b"tok").unwrap().into(),
            lease_duration_secs: 0,
            obtained_at: std::time::Instant::now() - std::time::Duration::from_secs(9999),
        };
        assert!(!cached.is_expired());
    }
}
