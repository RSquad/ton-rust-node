/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::Role;
use crate::runtime_config::RuntimeConfig;
use anyhow::Context;
use common::{app_config::UserEntry, verify_password};
use secrets_vault::{
    crypto::factory::CryptoFactory,
    types::{
        algorithm::Algorithm,
        metadata::Metadata,
        secret::{Secret, SecretInMemoryFactory},
        secret_id::SecretId,
        store_mode::StoreMode,
    },
    vault::SecretVault,
    vault_block::BlockCryptoFactory,
};
use std::sync::Arc;

/// Vault key prefix for all user password-hash secrets.
const USER_PREFIX: &str = "auth.users.";

#[derive(Clone, serde::Serialize)]
pub struct UserInfo {
    pub username: String,
    pub role: Role,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoked_after: Option<u64>,
}

/// Manages authentication users.
///
/// Reads the user list from the live [`AppConfig`] (via [`RuntimeConfig`]) and
/// resolves password hashes either from the vault (`password_name` field) or
/// from inline config (`password_hash` field, testing fallback).
///
/// Note: UserStore reads the config on every call, so newly added users become visible
/// as soon as the config file is reloaded by the service.
pub struct UserStore {
    runtime_cfg: Arc<dyn RuntimeConfig>,
}

impl UserStore {
    pub fn new(runtime_cfg: Arc<dyn RuntimeConfig>) -> Self {
        Self { runtime_cfg }
    }

    /// Verifies a username/password pair and returns the user's role on success.
    ///
    /// Returns `Ok(None)` if the user doesn't exist or the password is wrong.
    /// Returns `Err` only on infrastructure failures (vault I/O, corrupted hash, etc.).
    pub async fn login(&self, username: &str, password: &str) -> anyhow::Result<Option<Role>> {
        let users = self.users();
        let entry = match users.iter().find(|u| u.username == username) {
            Some(e) => e,
            None => return Ok(None),
        };

        let hash = self.resolve_password_hash(entry).await?;
        let hash = match hash {
            Some(h) => h,
            None => anyhow::bail!(
                "user '{}' has neither vault secret nor inline password_hash",
                username
            ),
        };

        if !verify_password(password, &hash)
            .with_context(|| format!("verify password for user {username}"))?
        {
            return Ok(None);
        }

        Ok(Some(entry.role))
    }

    /// Returns all users from the current config (no vault I/O needed).
    pub fn list_users(&self) -> Vec<UserInfo> {
        self.users()
            .iter()
            .map(|e| UserInfo {
                username: e.username.clone(),
                role: e.role,
                revoked_after: e.revoked_after,
            })
            .collect()
    }

    /// Returns authz state used by JWT middleware (current role + revocation cutoff).
    pub fn find_user(&self, username: &str) -> Option<UserInfo> {
        let users = self.users();
        let entry = users.iter().find(|u| u.username == username)?;

        Some(UserInfo {
            username: entry.username.clone(),
            role: entry.role,
            revoked_after: entry.revoked_after,
        })
    }

    /// Creates a new user: hashes the password, stores it in vault, and
    /// appends a [`UserEntry`] to the config (persisted to disk).
    #[cfg(test)]
    pub async fn create_user(
        &self,
        username: &str,
        password: &str,
        role: Role,
    ) -> anyhow::Result<()> {
        use common::hash_password;

        let vault = self
            .vault()
            .ok_or_else(|| anyhow::anyhow!("user management requires vault backend"))?;

        let users = self.users();
        if users.iter().any(|u| u.username == username) {
            anyhow::bail!("user '{username}' already exists");
        }

        let hash = hash_password(password)?;
        let secret_id = user_secret_id(username);
        store_password_blob(&vault, &secret_id, &hash).await?;

        let user_name = username.to_owned();
        let secret_name = secret_id.to_string();
        self.runtime_cfg.update_and_save(Box::new(move |cfg| {
            let auth = cfg.http.auth.get_or_insert_with(Default::default);
            auth.users.push(UserEntry {
                username: user_name,
                role,
                password_name: Some(secret_name),
                password_hash: None,
                revoked_after: None,
            });
        }))?;

        Ok(())
    }

    /// Deletes a user: removes vault secret and the [`UserEntry`] from config.
    #[cfg(test)]
    pub async fn delete_user(&self, username: &str) -> anyhow::Result<()> {
        let vault = self
            .vault()
            .ok_or_else(|| anyhow::anyhow!("user management requires vault backend"))?;

        let users = self.users();
        let entry = users
            .iter()
            .find(|u| u.username == username)
            .ok_or_else(|| anyhow::anyhow!("user '{username}' not found"))?;

        if let Some(ref secret_name) = entry.password_name {
            let sid = SecretId::new(secret_name);
            if vault.exists(&sid).await? {
                vault.delete(&sid).await?;
            }
        }

        let user_name = username.to_owned();
        self.runtime_cfg.update_and_save(Box::new(move |cfg| {
            if let Some(auth) = &mut cfg.http.auth {
                auth.users.retain(|u| u.username != user_name);
            }
        }))?;

        Ok(())
    }

    /// Returns the current user list from the live config snapshot.
    fn users(&self) -> Vec<UserEntry> {
        let config = self.runtime_cfg.get();
        config.http.auth.as_ref().map(|a| a.users.clone()).unwrap_or_default()
    }

    fn vault(&self) -> Option<Arc<SecretVault>> {
        self.runtime_cfg.vault()
    }

    /// Resolves the Argon2 hash string for a user entry.
    ///
    /// Priority: vault secret (`password_name`) > inline (`password_hash`).
    async fn resolve_password_hash(&self, entry: &UserEntry) -> anyhow::Result<Option<String>> {
        if let Some(ref name) = entry.password_name {
            if let Some(vault) = self.vault() {
                let sid = SecretId::new(name);
                if !vault.exists(&sid).await? {
                    anyhow::bail!(
                        "vault secret '{}' not found for user '{}'",
                        name,
                        entry.username
                    );
                }
                let secret = vault.load(&sid).await?;
                let hash_bytes = extract_blob_bytes(&secret)?;
                let hash = String::from_utf8(hash_bytes).map_err(|_| {
                    anyhow::anyhow!(
                        "corrupted password hash in vault for user '{}'",
                        entry.username
                    )
                })?;
                return Ok(Some(hash));
            }
        }

        if let Some(ref hash) = entry.password_hash {
            return Ok(Some(hash.clone()));
        }

        Ok(None)
    }
}

/// Maximum allowed username length (bytes).
pub const MAX_USERNAME_LEN: usize = 64;

/// Validates that a username contains only allowed characters (`a-z`, `A-Z`, `0-9`, `_`, `-`)
/// and is between 1 and [`MAX_USERNAME_LEN`] bytes long.
pub fn validate_username(username: &str) -> anyhow::Result<()> {
    if username.is_empty() || username.len() > MAX_USERNAME_LEN {
        anyhow::bail!("username must be 1-{MAX_USERNAME_LEN} characters long");
    }
    if !username.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        anyhow::bail!(
            "username '{}' contains invalid characters (allowed: a-z, A-Z, 0-9, _, -)",
            username
        );
    }
    Ok(())
}

/// Builds the vault [`SecretId`] for a given username (e.g. `"auth.users.admin"`).
pub fn user_secret_id(username: &str) -> SecretId {
    SecretId::new(format!("{USER_PREFIX}{username}"))
}

/// Stores an Argon2 password hash as a blob secret in the vault.
pub async fn store_password_blob(
    vault: &SecretVault,
    secret_id: &SecretId,
    password_hash: &str,
) -> anyhow::Result<()> {
    let mut metadata = Metadata::new(Some(secret_id), Algorithm::None, true);
    metadata.tags.insert("role".to_owned(), "auth-hash".to_owned());

    let secret = SecretInMemoryFactory::new_raw(
        password_hash.as_bytes(),
        metadata,
        BlockCryptoFactory {}.new_crypto()?,
    )?;
    vault.store(&secret, StoreMode::NewOnly).await
}

/// Extracts raw bytes from a [`Secret::Blob`] variant.
fn extract_blob_bytes(secret: &Secret) -> anyhow::Result<Vec<u8>> {
    match secret {
        Secret::Blob { blob } => Ok(blob.data().lock()?.to_vec()),
        _ => anyhow::bail!("expected blob secret for user record"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_config::RuntimeConfig;
    use common::app_config::{AppConfig, AuthConfig, UserEntry};
    use contracts::{NominatorWrapper, TonWallet};
    use secrets_vault::{
        crypto::{key_material::KeyMaterial, master_key::MasterKey},
        storage::file_json::FileJsonStorage,
        vault::SecretVault,
        vault_builder::SecretVaultBuilder,
    };
    use std::collections::HashMap;
    use ton_block::MsgAddressInt;
    use ton_http_api_client::v2::client_json_rpc::ClientJsonRpc;

    /// A test-only RuntimeConfig backed by an in-memory vault.
    struct TestRuntimeConfig {
        config: std::sync::RwLock<Arc<AppConfig>>,
        vault: Arc<SecretVault>,
    }

    impl TestRuntimeConfig {
        async fn new(config: AppConfig) -> Self {
            let vault = create_test_vault().await;
            Self { config: std::sync::RwLock::new(Arc::new(config)), vault: Arc::new(vault) }
        }
    }

    async fn create_test_master_key() -> MasterKey {
        let key_material = KeyMaterial::generate_new(
            Algorithm::None,
            Some(32),
            BlockCryptoFactory {}.new_crypto().unwrap().as_ref(),
        )
        .unwrap();
        MasterKey::from_key_material(key_material).unwrap()
    }

    async fn create_test_vault() -> SecretVault {
        let master_key = create_test_master_key().await;

        let temp_dir = tempfile::TempDir::new().unwrap();
        let file_path = temp_dir.path().join("secrets.json");
        std::mem::forget(temp_dir); // Leak the temp_dir to keep it alive
        let storage = Arc::new(
            FileJsonStorage::new(
                master_key,
                &file_path,
                false,
                BlockCryptoFactory {}.new_crypto().unwrap(),
            )
            .await
            .unwrap(),
        );
        SecretVaultBuilder::default().with_storage(storage).build().await.unwrap()
    }

    struct NoopWallet;
    #[async_trait::async_trait]
    impl contracts::SmartContract for NoopWallet {
        async fn address(&self) -> anyhow::Result<MsgAddressInt> {
            Ok(MsgAddressInt::with_standart(None, 0, [0u8; 32].into()).unwrap())
        }
        async fn balance(&self) -> anyhow::Result<u64> {
            Ok(0)
        }
    }
    #[async_trait::async_trait]
    impl TonWallet for NoopWallet {
        async fn message(
            &self,
            _: MsgAddressInt,
            _: u64,
            _: ton_block::Cell,
        ) -> anyhow::Result<ton_block::Cell> {
            unimplemented!()
        }
        async fn deploy_message(
            &self,
            _: u64,
            _: ton_block::Cell,
        ) -> anyhow::Result<ton_block::Cell> {
            unimplemented!()
        }
        async fn build_message(
            &self,
            _: MsgAddressInt,
            _: u64,
            _: ton_block::Cell,
            _: bool,
            _: Option<u32>,
            _: Option<ton_block::StateInit>,
            _: Option<ton_block::StateInit>,
        ) -> anyhow::Result<ton_block::Cell> {
            unimplemented!()
        }
    }

    impl RuntimeConfig for TestRuntimeConfig {
        fn get(&self) -> Arc<AppConfig> {
            self.config.read().expect("lock").clone()
        }

        fn master_wallet(&self) -> Arc<dyn TonWallet> {
            Arc::new(NoopWallet)
        }

        fn pools(&self) -> Arc<HashMap<String, Arc<dyn NominatorWrapper>>> {
            Arc::new(HashMap::new())
        }

        fn wallets(&self) -> Arc<HashMap<String, Arc<dyn TonWallet>>> {
            Arc::new(HashMap::new())
        }

        fn rpc_client(&self) -> Arc<ClientJsonRpc> {
            Arc::new(
                ClientJsonRpc::connect_many(
                    vec![("http://127.0.0.1:3301/".to_owned(), None)],
                    None,
                )
                .unwrap(),
            )
        }

        fn vault(&self) -> Option<Arc<SecretVault>> {
            Some(self.vault.clone())
        }

        fn update_and_save(&self, f: Box<dyn FnOnce(&mut AppConfig) + Send>) -> anyhow::Result<()> {
            let mut guard = self.config.write().expect("lock");
            let mut cfg = (**guard).clone();
            f(&mut cfg);
            *guard = Arc::new(cfg);
            Ok(())
        }
    }

    fn test_auth_config() -> AuthConfig {
        let hash = common::hash_password("pass1").unwrap();
        AuthConfig {
            operator_token_ttl: 3600,
            nominator_token_ttl: 7200,
            min_password_length: 8,
            jwt_secret: None,
            users: vec![UserEntry {
                username: "existing".into(),
                role: Role::Operator,
                password_name: None,
                password_hash: Some(hash),
                revoked_after: None,
            }],
        }
    }

    fn test_app_config(auth: AuthConfig) -> AppConfig {
        AppConfig {
            nodes: HashMap::new(),
            wallets: HashMap::new(),
            pools: HashMap::new(),
            bindings: HashMap::new(),
            ton_http_api: Default::default(),
            http: common::app_config::HttpConfig { auth: Some(auth), ..Default::default() },
            elections: None,
            voting: None,
            master_wallet: None,
            tick_interval: 30,
            log: Some(Default::default()),
        }
    }

    async fn setup() -> (Arc<TestRuntimeConfig>, UserStore) {
        let rt = Arc::new(TestRuntimeConfig::new(test_app_config(test_auth_config())).await);
        let store = UserStore::new(rt.clone() as Arc<dyn RuntimeConfig>);
        (rt, store)
    }

    // --- create_user tests ---

    #[tokio::test]
    async fn create_user_adds_to_config_and_vault() {
        let (rt, store) = setup().await;

        store.create_user("alice", "s3cret", Role::Operator).await.unwrap();

        // User appears in config
        let cfg = rt.get();
        let users = &cfg.http.auth.as_ref().unwrap().users;
        let alice = users.iter().find(|u| u.username == "alice").expect("alice not in config");
        assert_eq!(alice.role, Role::Operator);
        assert!(alice.password_name.is_some(), "should have vault secret name");
        assert!(alice.password_hash.is_none(), "inline hash should be None");

        // Password hash is stored in vault
        let sid = user_secret_id("alice");
        assert!(rt.vault.exists(&sid).await.unwrap(), "vault secret should exist");

        // Can log in with the new user
        let role = store.login("alice", "s3cret").await.unwrap();
        assert_eq!(role, Some(Role::Operator));
    }

    #[tokio::test]
    async fn create_user_nominator_role() {
        let (_rt, store) = setup().await;

        store.create_user("bob", "pass", Role::Nominator).await.unwrap();

        let role = store.login("bob", "pass").await.unwrap();
        assert_eq!(role, Some(Role::Nominator));
    }

    #[tokio::test]
    async fn create_user_rejects_duplicate() {
        let (_rt, store) = setup().await;

        // "existing" is already in the test config
        let err = store.create_user("existing", "pass", Role::Operator).await;
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("already exists"));
    }

    #[tokio::test]
    async fn create_user_wrong_password_returns_none() {
        let (_rt, store) = setup().await;

        store.create_user("carol", "correct", Role::Operator).await.unwrap();

        let result = store.login("carol", "wrong").await.unwrap();
        assert_eq!(result, None);
    }

    // --- delete_user tests ---

    #[tokio::test]
    async fn delete_user_removes_from_config_and_vault() {
        let (rt, store) = setup().await;

        // Create then delete
        store.create_user("dave", "pass", Role::Operator).await.unwrap();
        let sid = user_secret_id("dave");
        assert!(rt.vault.exists(&sid).await.unwrap());

        store.delete_user("dave").await.unwrap();

        // Removed from config
        let cfg = rt.get();
        let users = &cfg.http.auth.as_ref().unwrap().users;
        assert!(!users.iter().any(|u| u.username == "dave"), "dave should be removed from config");

        // Removed from vault
        assert!(!rt.vault.exists(&sid).await.unwrap(), "vault secret should be deleted");
    }

    #[tokio::test]
    async fn delete_user_not_found_returns_error() {
        let (_rt, store) = setup().await;

        let err = store.delete_user("ghost").await;
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("not found"));
    }

    #[tokio::test]
    async fn delete_user_without_vault_secret_still_removes_from_config() {
        let (rt, store) = setup().await;

        // "existing" user has inline password_hash, no password_name → no vault secret
        store.delete_user("existing").await.unwrap();

        let cfg = rt.get();
        let users = &cfg.http.auth.as_ref().unwrap().users;
        assert!(!users.iter().any(|u| u.username == "existing"));
    }

    #[tokio::test]
    async fn delete_user_login_fails_after_deletion() {
        let (_rt, store) = setup().await;

        store.create_user("eve", "pass", Role::Nominator).await.unwrap();
        assert_eq!(store.login("eve", "pass").await.unwrap(), Some(Role::Nominator));

        store.delete_user("eve").await.unwrap();

        // Login should return None (user not found)
        assert_eq!(store.login("eve", "pass").await.unwrap(), None);
    }

    #[tokio::test]
    async fn create_then_delete_then_recreate() {
        let (_rt, store) = setup().await;

        store.create_user("frank", "pass1", Role::Operator).await.unwrap();
        store.delete_user("frank").await.unwrap();

        // Recreate with different role and password
        store.create_user("frank", "pass2", Role::Nominator).await.unwrap();

        assert_eq!(store.login("frank", "pass2").await.unwrap(), Some(Role::Nominator));
        assert_eq!(store.login("frank", "pass1").await.unwrap(), None);
    }

    // --- list_users / find_user integration ---

    #[tokio::test]
    async fn list_users_reflects_create_and_delete() {
        let (_rt, store) = setup().await;

        let initial = store.list_users();
        assert_eq!(initial.len(), 1); // "existing"

        store.create_user("u1", "p", Role::Operator).await.unwrap();
        store.create_user("u2", "p", Role::Nominator).await.unwrap();
        assert_eq!(store.list_users().len(), 3);

        store.delete_user("u1").await.unwrap();
        let after = store.list_users();
        assert_eq!(after.len(), 2);
        assert!(after.iter().any(|u| u.username == "u2"));
        assert!(!after.iter().any(|u| u.username == "u1"));
    }

    #[tokio::test]
    async fn find_user_returns_correct_info_after_create() {
        let (_rt, store) = setup().await;

        store.create_user("gina", "p", Role::Nominator).await.unwrap();

        let info = store.find_user("gina").expect("should find gina");
        assert_eq!(info.username, "gina");
        assert_eq!(info.role, Role::Nominator);
        assert!(info.revoked_after.is_none());
    }

    #[tokio::test]
    async fn find_user_returns_none_after_delete() {
        let (_rt, store) = setup().await;

        store.create_user("hank", "p", Role::Operator).await.unwrap();
        store.delete_user("hank").await.unwrap();

        assert!(store.find_user("hank").is_none());
    }

    // --- validate_username tests ---

    #[test]
    fn validate_username_accepts_valid_names() {
        assert!(validate_username("alice").is_ok());
        assert!(validate_username("Bob_123").is_ok());
        assert!(validate_username("node-operator").is_ok());
        assert!(validate_username("a").is_ok());
        assert!(validate_username(&"a".repeat(MAX_USERNAME_LEN)).is_ok());
    }

    #[test]
    fn validate_username_rejects_empty() {
        assert!(validate_username("").is_err());
    }

    #[test]
    fn validate_username_rejects_too_long() {
        assert!(validate_username(&"a".repeat(MAX_USERNAME_LEN + 1)).is_err());
    }

    #[test]
    fn validate_username_rejects_special_chars() {
        assert!(validate_username("user.name").is_err());
        assert!(validate_username("user\\name").is_err());
        assert!(validate_username("user name").is_err());
        assert!(validate_username("user@host").is_err());
    }
}
