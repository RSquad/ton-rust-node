/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    crypto::{crypto_trait::Crypto, key_material::KeyMaterial, master_key::MasterKey},
    errors::error::VaultError,
    storage::{
        file_json_migrator::migrate_tree_node_v1_to_v2,
        storage_trait::Storage,
        utils::{decrypt, generate_secret_in_memory, hex_string, prepare_to_store},
    },
    types::{
        metadata::Metadata,
        secret::{Secret, SecretInMemoryFactory},
        secret_id::SecretId,
        secret_spec::SecretSpec,
        store_mode::StoreMode,
    },
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct StoredSecret {
    #[serde(with = "hex_string")]
    encrypted_data: Vec<u8>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
struct SecretNode {
    #[serde(skip_serializing_if = "Option::is_none")]
    secret: Option<StoredSecret>,
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    children: HashMap<String, SecretNode>,
}

impl SecretNode {
    fn new() -> Self {
        Self { secret: None, children: HashMap::new() }
    }

    fn insert(&mut self, path_parts: &[String], encrypted_data: Vec<u8>) {
        if path_parts.is_empty() {
            self.secret = Some(StoredSecret { encrypted_data });
            return;
        }

        let key = &path_parts[0];
        let child = self.children.entry(key.clone()).or_default();
        child.insert(&path_parts[1..], encrypted_data);
    }

    fn get(&self, path_parts: &[String]) -> Option<&StoredSecret> {
        if path_parts.is_empty() {
            return self.secret.as_ref();
        }

        let key = &path_parts[0];
        self.children.get(key)?.get(&path_parts[1..])
    }

    fn remove(&mut self, path_parts: &[String]) -> bool {
        if path_parts.is_empty() {
            if self.secret.is_some() {
                self.secret = None;
                return true;
            }
            return false;
        }

        let key = &path_parts[0];
        if let Some(child) = self.children.get_mut(key) {
            let removed = child.remove(&path_parts[1..]);

            if removed && child.secret.is_none() && child.children.is_empty() {
                self.children.remove(key);
            }

            return removed;
        }

        false
    }

    fn exists(&self, path_parts: &[String]) -> bool {
        if path_parts.is_empty() {
            return self.secret.is_some();
        }

        let key = &path_parts[0];
        self.children.get(key).map(|child| child.exists(&path_parts[1..])).unwrap_or(false)
    }

    fn collect_all(
        &self,
        current_path: Vec<String>,
        result: &mut Vec<(Vec<String>, StoredSecret)>,
    ) {
        if let Some(ref secret) = self.secret {
            result.push((current_path.clone(), secret.clone()));
        }

        for (key, child) in &self.children {
            let mut new_path = current_path.clone();
            new_path.push(key.clone());
            child.collect_all(new_path, result);
        }
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct StorageFile {
    version: u32,
    tree: SecretNode,
}

pub struct FileJsonStorage {
    master_key: MasterKey,
    file_path: PathBuf,
    tree: tokio::sync::RwLock<SecretNode>,
    crypto: Arc<dyn Crypto>,
}

impl FileJsonStorage {
    const FORMAT_VERSION: u32 = 2;

    pub async fn new(
        master_key: MasterKey,
        file_path: &Path,
        auto_migrate: bool,
        crypto: Arc<dyn Crypto>,
    ) -> anyhow::Result<Self> {
        if let Some(parent) = file_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let tree = if tokio::fs::metadata(&file_path).await.is_ok() {
            let mut json = tokio::fs::read_to_string(&file_path).await?;
            let mut storage_file: StorageFile = serde_json::from_str(&json)?;

            if storage_file.version != Self::FORMAT_VERSION {
                if !auto_migrate {
                    anyhow::bail!(
                        "Wrong vault format version. Expected {}, got {}. Run `secrets-vault-cli migrate` to update the vault file to the new format version",
                        Self::FORMAT_VERSION,
                        storage_file.version
                    );
                }

                Self::migrate(file_path, master_key.key_material(), crypto.clone()).await?;
                json = tokio::fs::read_to_string(&file_path).await?;
                storage_file = serde_json::from_str(&json)?;
            }

            storage_file.tree
        } else {
            SecretNode::new()
        };

        Ok(Self {
            master_key,
            file_path: file_path.to_path_buf(),
            tree: tokio::sync::RwLock::new(tree),
            crypto,
        })
    }

    pub fn file_path(&self) -> &Path {
        &self.file_path
    }

    /// Migrate a secret ID from the legacy `/`-separated format (pre-v0.2.0)
    /// to the current `.`-separated format.
    ///
    /// Legacy: `private_keys/base64key\/with\/slashes` (sep `/`, escape `\/`)
    /// Current: `private_keys.base64key/with/slashes` (sep `.`, escape `\.`)
    fn migrate_legacy_secret_id(id: &SecretId) -> SecretId {
        let s = id.as_str();

        // Detect format by finding the first unescaped separator.
        // Old format uses `/`, current format uses `.`.
        let is_legacy = {
            let mut chars = s.chars().peekable();
            let mut found_legacy = false;
            while let Some(ch) = chars.next() {
                match ch {
                    '\\' => {
                        chars.next();
                    }
                    '.' => break,
                    '/' => {
                        found_legacy = true;
                        break;
                    }
                    _ => {}
                }
            }
            found_legacy
        };

        if !is_legacy {
            return id.clone();
        }

        // Parse using legacy rules: split on `/`, unescape `\/` and `\\`
        let mut parts = Vec::new();
        let mut current = String::new();
        let mut chars = s.chars().peekable();

        while let Some(ch) = chars.next() {
            match ch {
                '\\' => {
                    if let Some(&next_ch) = chars.peek() {
                        match next_ch {
                            '/' | '\\' => {
                                current.push(next_ch);
                                chars.next();
                            }
                            _ => current.push(ch),
                        }
                    } else {
                        current.push(ch);
                    }
                }
                '/' => {
                    if !current.is_empty() {
                        parts.push(std::mem::take(&mut current));
                    }
                }
                _ => current.push(ch),
            }
        }
        if !current.is_empty() {
            parts.push(current);
        }

        // Rebuild in current format: escape `.` and `\`, join with `.`
        let new_value = parts.iter().map(|p| SecretId::escape(p)).collect::<Vec<_>>().join(".");
        SecretId::from(new_value)
    }

    fn parse_path(id: &SecretId) -> Vec<String> {
        let s = id.as_str();
        let mut parts = Vec::new();
        let mut current = String::new();
        let mut chars = s.chars().peekable();

        while let Some(ch) = chars.next() {
            match ch {
                '\\' => {
                    if let Some(&next_ch) = chars.peek() {
                        match next_ch {
                            '.' => {
                                current.push('.');
                                chars.next();
                            }
                            '\\' => {
                                current.push('\\');
                                chars.next();
                            }
                            _ => {
                                current.push(ch);
                            }
                        }
                    } else {
                        current.push(ch);
                    }
                }
                '.' => {
                    if !current.is_empty() {
                        parts.push(current.clone());
                        current.clear();
                    }
                }
                _ => {
                    current.push(ch);
                }
            }
        }

        if !current.is_empty() {
            parts.push(current);
        }

        parts
    }

    async fn save_to_disk(&self, tree: &SecretNode) -> anyhow::Result<()> {
        let storage_file = StorageFile { version: Self::FORMAT_VERSION, tree: tree.clone() };
        let json = serde_json::to_string_pretty(&storage_file)?;

        Self::safe_save(&json, &self.file_path).await
    }

    async fn safe_save(data: &str, file_path: &Path) -> anyhow::Result<()> {
        let temp_path = file_path.with_extension("tmp");
        let mut file = tokio::fs::File::create(&temp_path).await?;
        file.write_all(data.as_bytes()).await?;
        file.sync_all().await?;
        drop(file);
        tokio::fs::rename(&temp_path, file_path).await?;

        Ok(())
    }

    async fn migrate_to(
        from_version: u32,
        to_version: u32,
        mut value_json: serde_json::Value,
        master_key: &KeyMaterial,
        crypto: Arc<dyn Crypto>,
    ) -> anyhow::Result<serde_json::Value> {
        match (from_version, to_version) {
            (1, 2) => {
                if let Some(tree) = value_json.get_mut("tree") {
                    migrate_tree_node_v1_to_v2(tree, master_key, crypto.as_ref())?;
                }
                value_json["version"] = serde_json::json!(2);
                Ok(value_json)
            }
            _ => {
                anyhow::bail!(
                    "No migration available from version {} to {}",
                    from_version,
                    to_version
                )
            }
        }
    }

    pub async fn migrate(
        file_path: &Path,
        master_key: &KeyMaterial,
        crypto: Arc<dyn Crypto>,
    ) -> anyhow::Result<()> {
        let json_str = tokio::fs::read_to_string(file_path).await?;
        let mut json_value: serde_json::Value = serde_json::from_str(&json_str)?;
        let format_version: u32 = json_value
            .get("version")
            .ok_or_else(|| anyhow::anyhow!("Missing 'version' field"))?
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("'version' must be a number"))?
            .try_into()
            .map_err(|_| anyhow::anyhow!("'version' does not fit into u32"))?;

        if format_version == Self::FORMAT_VERSION {
            return Ok(());
        }

        if format_version > Self::FORMAT_VERSION {
            anyhow::bail!(
                "Vault file version {} is newer than supported version {}",
                format_version,
                Self::FORMAT_VERSION
            );
        }

        // Backup
        {
            let timestamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
            let base_backup_name =
                format!("{}.backup_v{}_{}", file_path.to_string_lossy(), format_version, timestamp);

            let mut backup_name = base_backup_name.clone();
            let mut counter = 1;

            while tokio::fs::try_exists(&backup_name).await? {
                backup_name = format!("{}.{}", base_backup_name, counter);
                counter += 1;
            }

            tokio::fs::copy(file_path, &backup_name).await?;
        }

        let mut current_version = format_version;
        while current_version < Self::FORMAT_VERSION {
            let next_version = current_version + 1;
            json_value = Self::migrate_to(
                current_version,
                next_version,
                json_value,
                master_key,
                crypto.clone(),
            )
            .await?;
            current_version = next_version;
        }

        let migrated_str = serde_json::to_string_pretty(&json_value)?;
        Self::safe_save(&migrated_str, file_path).await?;

        Ok(())
    }
}

#[async_trait::async_trait]
impl Storage for FileJsonStorage {
    async fn flush(&self) -> anyhow::Result<()> {
        let tree = self.tree.write().await;
        self.save_to_disk(&tree).await
    }

    async fn generate_secret(
        &self,
        spec: &SecretSpec,
        secret_id: &SecretId,
    ) -> anyhow::Result<Secret> {
        let secret = generate_secret_in_memory(spec, secret_id, self.crypto.clone())?;
        self.store(&secret, StoreMode::NewOnly).await?;
        self.load(secret_id).await
    }

    async fn store(&self, secret: &Secret, mode: StoreMode) -> anyhow::Result<()> {
        let secret_id = match &secret.metadata().secret_id {
            Some(secret_id) => secret_id,
            None => {
                anyhow::bail!(VaultError::empty_secret_id("Failed to store"));
            }
        };

        let data = secret.serialize()?;
        let mut tree = self.tree.write().await;
        let path_parts = Self::parse_path(secret_id);
        let exists = tree.exists(&path_parts);
        let encrypted_data = prepare_to_store(
            &data,
            secret.metadata(),
            mode,
            exists,
            self.master_key.key_material(),
            self.crypto.as_ref(),
        )?;
        tree.insert(&path_parts, encrypted_data);

        self.save_to_disk(&tree).await
    }

    async fn load(&self, secret_id: &SecretId) -> anyhow::Result<Secret> {
        let path_parts = Self::parse_path(secret_id);
        let tree = self.tree.read().await;
        let stored = tree
            .get(&path_parts)
            .ok_or_else(|| VaultError::not_found(format!("Secret '{}' not found", secret_id)))?;
        let (data, metadata) =
            decrypt(self.master_key.key_material(), &stored.encrypted_data, self.crypto.as_ref())?;

        let secret = SecretInMemoryFactory::deserialize(data, metadata, self.crypto.clone())?;
        Ok(secret)
    }

    async fn load_metadata(&self, secret_id: &SecretId) -> anyhow::Result<Option<Metadata>> {
        match self.load(secret_id).await {
            Ok(secret) => Ok(Some(secret.metadata().clone())),
            Err(err) => {
                if let Some(vault_err) = err.downcast_ref::<VaultError>() {
                    match vault_err.code() {
                        VaultError::NOT_FOUND => Ok(None),
                        _ => Err(err),
                    }
                } else {
                    Err(err)
                }
            }
        }
    }

    async fn list_metadata(&self) -> anyhow::Result<Vec<Metadata>> {
        let tree = self.tree.read().await;
        let mut all_secrets = Vec::new();
        tree.collect_all(Vec::new(), &mut all_secrets);

        let mut result = Vec::with_capacity(all_secrets.len());

        for (_path_parts, stored) in all_secrets {
            match decrypt(
                self.master_key.key_material(),
                &stored.encrypted_data,
                self.crypto.as_ref(),
            ) {
                Ok((_, mut meta)) => {
                    if let Some(ref secret_id) = meta.secret_id {
                        meta.secret_id = Some(Self::migrate_legacy_secret_id(secret_id));
                    }
                    result.push(meta)
                }
                Err(e) => anyhow::bail!(e),
            }
        }

        Ok(result)
    }

    async fn delete(&self, secret_id: &SecretId) -> anyhow::Result<()> {
        let path_parts = Self::parse_path(secret_id);
        let mut tree = self.tree.write().await;

        if !tree.remove(&path_parts) {
            anyhow::bail!(VaultError::not_found(format!("Secret '{}' not found", secret_id)))
        }

        self.save_to_disk(&tree).await
    }

    fn format_version(&self) -> anyhow::Result<u32> {
        Ok(Self::FORMAT_VERSION)
    }

    #[cfg(test)]
    async fn clear(&self) -> anyhow::Result<()> {
        let metas = self.list_metadata().await?;

        for meta in &metas {
            let secret_id =
                meta.secret_id.as_ref().ok_or_else(|| VaultError::empty_secret_id(""))?;
            self.delete(secret_id).await?;
        }

        Ok(())
    }

    #[cfg(test)]
    async fn is_empty(&self) -> anyhow::Result<bool> {
        let tree = self.tree.read().await;
        Ok(tree.secret.is_none() && tree.children.is_empty())
    }
}
