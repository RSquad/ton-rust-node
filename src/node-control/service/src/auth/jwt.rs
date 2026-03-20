/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::{Claims, Role};
use common::app_config::AuthConfig;
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation};
use secrets_vault::{
    types::{algorithm::Algorithm, secret::Secret, secret_id::SecretId, secret_spec::SecretSpec},
    vault::SecretVault,
};
use std::sync::Arc;

const JWT_KEY_SECRET_ID: &str = "auth.jwt-signing-key";

/// Handles JWT token signing and verification for the REST API.
///
/// The signing key is either loaded from the vault (`auth.jwt-signing-key`)
/// or taken from the config (`jwt_secret` field, base64-encoded, for testing).
/// When a vault key doesn't exist yet, a random 32-byte key is generated
/// and persisted so that tokens survive service restarts.
pub struct JwtAuth {
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
    auth_config: AuthConfig,
}

impl JwtAuth {
    /// Creates a new instance, resolving the HMAC-SHA256 signing key
    /// from vault (preferred) or config fallback.
    pub async fn new(
        vault: Option<Arc<SecretVault>>,
        auth_config: &AuthConfig,
    ) -> anyhow::Result<Self> {
        let secret_bytes = if let Some(vault) = vault {
            Self::load_or_create_key(&vault).await?
        } else if let Some(jwt_secret) = &auth_config.jwt_secret {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .decode(jwt_secret)
                .map_err(|e| anyhow::anyhow!("invalid base64 jwt_secret: {e}"))?
        } else {
            anyhow::bail!("auth is enabled but neither vault nor jwt_secret is configured");
        };

        if secret_bytes.len() < 32 {
            anyhow::bail!("JWT signing key must be at least 32 bytes, got {}", secret_bytes.len());
        }

        Ok(Self {
            encoding_key: EncodingKey::from_secret(&secret_bytes),
            decoding_key: DecodingKey::from_secret(&secret_bytes),
            auth_config: auth_config.clone(),
        })
    }

    /// generates a new JWT for the given user. Returns `(token, ttl_seconds)`.
    /// TTL depends on the role (configured in [`AuthConfig`]).
    pub fn generate(&self, username: &str, role: Role) -> anyhow::Result<(String, u64)> {
        let now =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();

        let ttl = match role {
            Role::Operator => self.auth_config.operator_token_ttl,
            Role::Nominator => self.auth_config.nominator_token_ttl,
        };

        let claims = Claims { sub: username.to_owned(), role, iat: now, exp: now + ttl };

        let token = jsonwebtoken::encode(&Header::default(), &claims, &self.encoding_key)?;
        Ok((token, ttl))
    }

    /// Verifies a JWT returning the decoded [`Claims`].
    pub fn verify(&self, token: &str) -> anyhow::Result<Claims> {
        let data =
            jsonwebtoken::decode::<Claims>(token, &self.decoding_key, &Validation::default())?;
        Ok(data.claims)
    }

    /// Loads the signing key from vault, or generates and stores a new one.
    async fn load_or_create_key(vault: &SecretVault) -> anyhow::Result<Vec<u8>> {
        let secret_id = SecretId::new(JWT_KEY_SECRET_ID);

        if vault.exists(&secret_id).await? {
            let secret = vault.get(&secret_id).await?;
            return Self::extract_blob_bytes(&secret).await;
        }

        tracing::info!(
            target: "auth",
            event = "auth_jwt_key_generated",
            secret_id = JWT_KEY_SECRET_ID,
            "generating JWT signing key in vault"
        );

        let spec = SecretSpec::new(Algorithm::None).extractable(true).size(32);
        let secret = vault.generate_secret(&spec, &secret_id).await?;

        Ok(secret.as_blob()?.data().await?.lock().await?.to_vec())
    }

    async fn extract_blob_bytes(secret: &Secret) -> anyhow::Result<Vec<u8>> {
        match secret {
            Secret::Blob { blob } => {
                let pm = blob.data().await?;
                let locked = pm.lock().await?;
                Ok(locked.to_vec())
            }
            _ => anyhow::bail!("expected blob secret for JWT key, got different type"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn test_config() -> AuthConfig {
        AuthConfig {
            operator_token_ttl: 3600,
            nominator_token_ttl: 7200,
            jwt_secret: Some(base64::engine::general_purpose::STANDARD.encode([42u8; 32])),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn sign_and_verify_roundtrip() {
        let mgr = JwtAuth::new(None, &test_config()).await.unwrap();
        let (token, ttl) = mgr.generate("admin", Role::Operator).unwrap();
        assert_eq!(ttl, 3600);

        let claims = mgr.verify(&token).unwrap();
        assert_eq!(claims.sub, "admin");
        assert_eq!(claims.role, Role::Operator);
    }

    #[tokio::test]
    async fn verify_rejects_invalid_token() {
        let mgr = JwtAuth::new(None, &test_config()).await.unwrap();
        assert!(mgr.verify("not-a-valid-token").is_err());
    }

    #[tokio::test]
    async fn verify_rejects_wrong_secret() {
        let cfg1 = test_config();
        let mgr1 = JwtAuth::new(None, &cfg1).await.unwrap();
        let (token, _) = mgr1.generate("admin", Role::Operator).unwrap();

        let cfg2 = AuthConfig {
            jwt_secret: Some(base64::engine::general_purpose::STANDARD.encode([99u8; 32])),
            ..cfg1
        };
        let mgr2 = JwtAuth::new(None, &cfg2).await.unwrap();
        assert!(mgr2.verify(&token).is_err());
    }

    #[tokio::test]
    async fn verify_rejects_expired_token() {
        let mgr = JwtAuth::new(None, &test_config()).await.unwrap();
        let claims = Claims {
            sub: "admin".to_owned(),
            role: Role::Operator,
            iat: 1000,
            exp: 1001, // far in the past, well beyond any leeway
        };
        let token = jsonwebtoken::encode(&Header::default(), &claims, &mgr.encoding_key).unwrap();
        assert!(mgr.verify(&token).is_err());
    }

    #[tokio::test]
    async fn no_vault_no_secret_fails() {
        let cfg = AuthConfig { jwt_secret: None, ..test_config() };
        assert!(JwtAuth::new(None, &cfg).await.is_err());
    }

    #[tokio::test]
    async fn short_secret_fails() {
        let cfg = AuthConfig {
            jwt_secret: Some(base64::engine::general_purpose::STANDARD.encode([1u8; 16])),
            ..test_config()
        };
        assert!(JwtAuth::new(None, &cfg).await.is_err());
    }
}
