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
    storage::hashicorp_token_provider::{create_token_provider, AuthConfig, TokenProvider},
    types::{metadata::Metadata, secret::Secret, store_mode::StoreMode},
};
use rand::RngCore;
use rsa::pkcs8::DecodePublicKey;
use std::{collections::HashMap, sync::Arc};
use zeroize::Zeroize;

#[allow(dead_code)]
pub enum KeyMode {
    Encryption,
    Signing,
}

#[derive(serde::Serialize)]
pub struct CreateKeyRequest {
    #[serde(rename = "type")]
    key_type: String,
    exportable: bool,
}

#[derive(serde::Deserialize)]
pub struct ExportKeyData {
    keys: HashMap<String, String>,
}

impl zeroize::Zeroize for ExportKeyData {
    fn zeroize(&mut self) {
        for (_k, v) in self.keys.iter_mut() {
            v.zeroize();
        }
        self.keys.clear();
    }
}

impl Drop for ExportKeyResponse {
    fn drop(&mut self) {
        self.zeroize();
    }
}

#[derive(serde::Deserialize)]
pub struct ExportKeyResponse {
    data: ExportKeyData,
}

impl zeroize::Zeroize for ExportKeyResponse {
    fn zeroize(&mut self) {
        self.data.zeroize();
    }
}

#[derive(serde::Serialize)]
pub struct SignDataRequest {
    input: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    hash_algorithm: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    signature_algorithm: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prehashed: Option<bool>,
}

#[derive(serde::Deserialize)]
pub struct SignDataResponse {
    data: SignDataData,
}

#[derive(serde::Deserialize)]
pub struct SignDataData {
    signature: String,
}

#[derive(serde::Serialize)]
pub struct VerifySignatureRequest {
    input: String,
    signature: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    hash_algorithm: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    signature_algorithm: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prehashed: Option<bool>,
}

#[derive(serde::Deserialize)]
pub struct VerifySignatureResponse {
    data: VerifySignatureData,
}

#[derive(serde::Deserialize)]
pub struct VerifySignatureData {
    valid: bool,
}

#[derive(serde::Deserialize)]
pub struct WrappingKeyData {
    public_key: String,
}

#[derive(serde::Deserialize)]
pub struct WrappingKeyResponse {
    data: WrappingKeyData,
}

#[derive(serde::Serialize)]
pub struct ImportKeyRequest {
    ciphertext: String,
    #[serde(rename = "type")]
    key_type: String,
    exportable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    allow_replacement: Option<bool>,
}

#[allow(dead_code)]
#[derive(serde::Deserialize)]
pub struct KeyInfo {
    #[serde(default)]
    pub certificate_chain: String,
    pub creation_time: chrono::DateTime<chrono::Utc>,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub public_key: String,
    #[serde(default)]
    pub hybrid_public_key: String,
}

#[allow(dead_code)]
#[derive(serde::Deserialize)]
pub struct KeyInfoData {
    #[serde(rename = "type")]
    pub key_type: String,
    #[serde(default)]
    pub exportable: bool,
    #[serde(default)]
    pub keys: HashMap<String, KeyInfo>,
    #[serde(default)]
    pub latest_version: i32,
}

#[derive(serde::Deserialize)]
pub struct KeyInfoResponse {
    pub data: KeyInfoData,
}

#[derive(serde::Deserialize, Default)]
pub struct ListKeysData {
    pub keys: Vec<String>,
}

#[derive(serde::Deserialize, Default)]
pub struct ListKeysResponse {
    pub data: ListKeysData,
}

#[derive(serde::Serialize)]
pub struct UpdateKeyConfigRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    deletion_allowed: Option<bool>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct BlobData {
    pub data: String,
    pub created_at: String,
}

#[derive(serde::Deserialize)]
pub struct KVDataWrapper<T> {
    pub data: T,
}

#[derive(serde::Deserialize)]
pub struct KVReadResponse<T> {
    pub data: KVDataWrapper<T>,
}

pub const DEFAULT_TRANSIT_MOUNT: &str = "transit";
pub const DEFAULT_KV_MOUNT: &str = "secret";

pub struct VaultConfig {
    pub namespace: Option<String>,
    pub transit_mount: String,
    pub transit_prefix: Option<String>,
    pub kv_mount: String,
    pub kv_prefix: Option<String>,
}

impl Default for VaultConfig {
    fn default() -> Self {
        Self {
            namespace: None,
            transit_mount: DEFAULT_TRANSIT_MOUNT.to_string(),
            transit_prefix: None,
            kv_mount: DEFAULT_KV_MOUNT.to_string(),
            kv_prefix: None,
        }
    }
}

pub struct Client {
    client: reqwest::Client,
    addr: String,
    token_provider: Arc<dyn TokenProvider>,
    transit_mount: String,
    transit_prefix: Option<String>,
    kv_mount: String,
    kv_prefix: Option<String>,
}

impl Client {
    pub fn new(addr: &str, auth: AuthConfig, config: VaultConfig) -> anyhow::Result<Self> {
        let addr = if addr.starts_with("http://") || addr.starts_with("https://") {
            addr.to_string()
        } else {
            format!("https://{}", addr)
        };

        let client = reqwest::Client::new();
        let vault_addr = format!("{addr}/v1");
        let token_provider = create_token_provider(auth, client.clone(), &vault_addr);

        let mut this = Self {
            client,
            addr: vault_addr,
            token_provider,
            transit_mount: config.transit_mount,
            transit_prefix: config.transit_prefix,
            kv_mount: config.kv_mount,
            kv_prefix: config.kv_prefix,
        };

        if let Some(namespace) = config.namespace {
            this = this.with_namespace(&namespace);
        }

        Ok(this)
    }

    pub fn with_namespace(self, namespace: &str) -> Self {
        Self { addr: format!("{}/{namespace}", self.addr), ..self }
    }

    pub fn addr(&self) -> &str {
        &self.addr
    }

    pub fn transit_mount_path(&self, action: &str, rest: Option<&str>) -> String {
        match &self.transit_prefix {
            Some(prefix) => match rest {
                Some(rest) => {
                    format!("{}/{}/{}/{}.{}", self.addr, self.transit_mount, action, prefix, rest)
                }
                None => format!("{}/{}/{}/{}", self.addr, self.transit_mount, action, prefix),
            },
            None => match rest {
                Some(rest) => {
                    format!("{}/{}/{}/{}", self.addr, self.transit_mount, action, rest)
                }
                None => format!("{}/{}/{}", self.addr, self.transit_mount, action),
            },
        }
    }

    pub fn kv_data_path(&self, rest: &str) -> String {
        match &self.kv_prefix {
            Some(prefix) => format!("{}/{}/data/{}/{}", self.addr, self.kv_mount, prefix, rest),
            None => format!("{}/{}/data/{}", self.addr, self.kv_mount, rest),
        }
    }

    pub fn kv_meta_path(&self, rest: &str) -> String {
        match &self.kv_prefix {
            Some(prefix) => {
                format!("{}/{}/metadata/{}/{}", self.addr, self.kv_mount, prefix, rest)
            }
            None => format!("{}/{}/metadata/{}", self.addr, self.kv_mount, rest),
        }
    }

    pub async fn create_key(
        &self,
        secret_name: &str,
        key_type: &str,
        exportable: bool,
    ) -> anyhow::Result<()> {
        let payload = CreateKeyRequest { key_type: key_type.to_string(), exportable };
        let url = self.transit_mount_path("keys", Some(Self::escape(secret_name).as_str()));
        let response: reqwest::Response =
            self.do_request_raw_rs(reqwest::Method::POST, &url, Some(&payload)).await?;

        let res_status = response.status();
        let response_payload = response.text().await?;

        if res_status.is_success() {
            Ok(())
        } else if res_status.as_u16() == 400 {
            if response_payload.contains("already exists") {
                anyhow::bail!("Key already exists");
            } else {
                anyhow::bail!("Failed to create key: {response_payload}");
            }
        } else {
            anyhow::bail!("Failed to create key: {response_payload}");
        }
    }

    pub async fn export_key(
        &self,
        secret_name: &str,
        key_mode: KeyMode,
    ) -> anyhow::Result<ProtectedMemory> {
        let url = match key_mode {
            KeyMode::Encryption => self.transit_mount_path(
                "export/encryption-key",
                Some(Self::escape(secret_name).as_str()),
            ),
            KeyMode::Signing => self
                .transit_mount_path("export/signing-key", Some(Self::escape(secret_name).as_str())),
        };

        let result: ExportKeyResponse = self
            .do_request_no_body(reqwest::Method::GET, &url)
            .await?
            .ok_or_else(|| VaultError::not_found(format!("URL '{}' not found (404)", url)))?;

        if result.data.keys.is_empty() {
            anyhow::bail!("No keys were found");
        }

        let latest_version = result
            .data
            .keys
            .keys()
            .filter_map(|v| v.parse::<u64>().ok())
            .max()
            .ok_or_else(|| anyhow::anyhow!("No keys were found"))?
            .to_string();

        let key_b64 = result.data.keys.get(&latest_version).ok_or_else(|| {
            anyhow::anyhow!("Version {} not found in exported keys", latest_version)
        })?;

        // TODO: make b64::decode implementation without allocation
        let key_bytes = base64::decode(key_b64)?;

        if key_bytes.len() != 64 {
            anyhow::bail!(VaultError::invalid_private_key(format!(
                "Private key data expected to be 64 bytes, but actual size is {} bytes",
                key_bytes.len()
            )));
        }

        let key_pd: ProtectedMemory =
            ProtectedMemoryInner::from_slice(key_bytes[..32].try_into().unwrap())?.into();

        Ok(key_pd)
    }

    pub async fn get_key_info(&self, secret_name: &str) -> anyhow::Result<(KeyInfo, bool)> {
        let url = self.transit_mount_path("keys", Some(Self::escape(secret_name).as_str()));
        let mut response: KeyInfoResponse = self
            .do_request_no_body(reqwest::Method::GET, &url)
            .await?
            .ok_or_else(|| VaultError::not_found(format!("URL '{}' not found (404)", url)))?;
        let key_id = response.data.latest_version.to_string();
        let key_info =
            response.data.keys.remove(&key_id).ok_or(anyhow::anyhow!(VaultError::NOT_FOUND))?;

        Ok((key_info, response.data.exportable))
    }

    pub async fn import_secret_ed25519(
        &self,
        secret: &Secret,
        mode: StoreMode,
    ) -> anyhow::Result<()> {
        // Check if not extractable
        if !secret.metadata().extractable {
            anyhow::bail!(VaultError::not_extractable(secret.metadata().secret_id.as_ref()));
        }

        // Get wrapping key from Vault
        let url = format!("{}/{}/wrapping_key", &self.addr, &self.transit_mount);
        let result: WrappingKeyResponse = self
            .do_request_no_body(reqwest::Method::GET, &url)
            .await?
            .ok_or_else(|| VaultError::not_found(format!("URL '{}' not found (404)", url)))?;
        let wrapping_key = rsa::RsaPublicKey::from_public_key_pem(&result.data.public_key)?;

        // Generate ephemeral AES-256 key
        let mut aes_key = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut aes_key);

        // Get the private key data
        let keypair = match &secret {
            Secret::SymmetricKey { .. } | Secret::Blob { .. } => {
                anyhow::bail!(VaultError::unsupported_algorithm(secret.metadata().algorithm));
            }
            Secret::KeyPair { keypair } => keypair,
        };

        // Convert raw Ed25519 seed to PKCS#8 DER format
        let signing_key = {
            let private_key = keypair.private_key()?;
            let private_key_lock = private_key.lock()?;
            let private_key_data: &[u8] = &private_key_lock;

            ed25519_dalek::SigningKey::from_bytes(private_key_data.try_into().map_err(|_| {
                VaultError::encryption_failed(
                    "Invalid Ed25519 key length, expected 32 bytes".to_string(),
                )
            })?)
        };

        use ed25519_dalek::pkcs8::EncodePrivateKey;
        let pkcs8_der = signing_key.to_pkcs8_der().map_err(|e| {
            VaultError::encryption_failed(format!("Failed to encode PKCS#8: {}", e))
        })?;
        let pkcs8_bytes = pkcs8_der.as_bytes();

        // Wrap target key with AES-KWP (Key Wrap with Padding)
        use aes_kw::Kek;
        let kek = Kek::<aes::Aes256>::from(aes_key);

        // AES-KWP output size: input padded to 8-byte boundary + 8 bytes for integrity check
        let padded_len = pkcs8_bytes.len().next_multiple_of(8);
        let wrapped_key_len = padded_len + 8;
        let mut wrapped_key = vec![0u8; wrapped_key_len];

        kek.wrap_with_padding(pkcs8_bytes, &mut wrapped_key)
            .map_err(|e| VaultError::encryption_failed(format!("AES-KWP wrap failed: {:?}", e)))?;

        // Wrap AES key with RSA-OAEP
        let wrapped_aes = wrapping_key.encrypt(
            &mut rand::thread_rng(),
            rsa::Oaep::new::<sha2::Sha256>(),
            &aes_key,
        )?;

        aes_key.zeroize();

        // Combine: wrapped_aes_key || wrapped_target_key
        let mut combined = Vec::with_capacity(wrapped_aes.len() + wrapped_key.len());
        combined.extend_from_slice(&wrapped_aes);
        combined.extend_from_slice(&wrapped_key);

        // Import request
        let allow_replacement = match mode {
            StoreMode::NewOnly => false,
            StoreMode::ReplaceExists => true,
            StoreMode::CreateOrReplace => true,
        };

        let request = ImportKeyRequest {
            ciphertext: base64::encode(&combined),
            key_type: "ed25519".to_string(),
            exportable: secret.metadata().extractable,
            allow_replacement: Some(allow_replacement),
        };

        let secret_id = match &secret.metadata().secret_id {
            Some(secret_id) => secret_id,
            None => {
                anyhow::bail!(VaultError::empty_secret_id("Failed to import"));
            }
        };

        let key_exists = self.get_key_info(secret_id.as_str()).await.is_ok();
        let url = if key_exists {
            self.transit_mount_path("keys", Some(Self::escape(secret_id.as_str()).as_str()))
                + "/import_version"
        } else {
            self.transit_mount_path("keys", Some(Self::escape(secret_id.as_str()).as_str()))
                + "/import"
        };

        let response = self.do_request_raw_rs(reqwest::Method::POST, &url, Some(&request)).await?;
        let status = response.status();
        let response_text = response.text().await?;

        if status.is_success() {
            Ok(())
        } else if status.as_u16() == 400 && response_text.contains("already exists") {
            anyhow::bail!("Key '{}' already exists", &secret_id);
        } else {
            anyhow::bail!("Failed to import key: {}", response_text)
        }
    }

    pub async fn sign(&self, secret_name: &str, message: &[u8]) -> anyhow::Result<Vec<u8>> {
        let message_b64 = base64::encode(message);

        let rq = SignDataRequest {
            input: message_b64.clone(),
            hash_algorithm: None,
            signature_algorithm: None,
            prehashed: None,
        };

        let url = self.transit_mount_path("sign", Some(Self::escape(secret_name).as_str()));
        let result: SignDataResponse = self
            .do_request(reqwest::Method::POST, &url, Some(&rq))
            .await?
            .ok_or_else(|| VaultError::not_found(format!("URL '{}' not found (404)", url)))?;

        let parts: Vec<&str> = result.data.signature.split(':').collect();
        let signature = base64::decode(
            parts.get(2).ok_or_else(|| anyhow::anyhow!("Malformed signature format from Vault"))?,
        )?;
        Ok(signature)
    }

    pub async fn verify_sign(
        &self,
        secret_name: &str,
        message: &[u8],
        signature: &[u8],
    ) -> anyhow::Result<()> {
        let message_b64 = base64::encode(message);
        let vault_signature = format!("vault:v1:{}", base64::encode(signature));

        let rq = VerifySignatureRequest {
            input: message_b64.clone(),
            signature: vault_signature,
            hash_algorithm: None,
            signature_algorithm: None,
            prehashed: None,
        };

        let url = self.transit_mount_path("verify", Some(Self::escape(secret_name).as_str()));
        let result: VerifySignatureResponse = self
            .do_request(reqwest::Method::POST, &url, Some(&rq))
            .await?
            .ok_or_else(|| VaultError::not_found(format!("URL '{}' not found (404)", url)))?;

        if !result.data.valid {
            anyhow::bail!("Signature invalid");
        }

        Ok(())
    }

    pub async fn list_keys(&self) -> anyhow::Result<Vec<String>> {
        let url = self.transit_mount_path("keys", None);
        let result: Option<ListKeysResponse> =
            self.do_request_no_body(reqwest::Method::from_bytes("LIST".as_bytes())?, &url).await?;
        Ok(result.unwrap_or_default().data.keys)
    }

    pub async fn delete_key(&self, secret_name: &str) -> anyhow::Result<()> {
        let config_url =
            self.transit_mount_path("keys", Some(Self::escape(secret_name).as_str())) + "/config";
        let config_request = UpdateKeyConfigRequest { deletion_allowed: Some(true) };
        let config_response = self
            .do_request_raw_rs(reqwest::Method::POST, &config_url, Some(&config_request))
            .await?;

        let config_status = config_response.status();
        if !config_status.is_success() {
            let error_text = config_response.text().await?;
            if config_status.as_u16() == 404 {
                anyhow::bail!(VaultError::not_found(format!("Key '{}' not found", secret_name)));
            }
            anyhow::bail!("Failed to enable deletion for key '{}': {}", secret_name, error_text);
        }

        let delete_url = self.transit_mount_path("keys", Some(Self::escape(secret_name).as_str()));
        let delete_response =
            self.do_request_raw_rs::<()>(reqwest::Method::DELETE, &delete_url, None).await?;

        let delete_status = delete_response.status();
        if delete_status.is_success() {
            Ok(())
        } else {
            let error_text = delete_response.text().await?;
            if delete_status.as_u16() == 404 {
                anyhow::bail!(VaultError::not_found(format!("Key '{}' not found", secret_name)));
            }
            anyhow::bail!("Failed to delete key '{}': {}", secret_name, error_text)
        }
    }

    pub async fn set_metadata(
        &self,
        secret_name: &str,
        metadata: &Metadata,
        cas: Option<u64>,
    ) -> anyhow::Result<()> {
        let url = self.kv_data_path(&format!("transit-metadata/{}", Self::escape(secret_name)));
        let mut payload = serde_json::json!({
                "data": metadata
        });
        if let Some(cas_value) = cas {
            payload["options"] = serde_json::json!({ "cas": cas_value });
        }
        let response = self.do_request_raw_rs(reqwest::Method::POST, &url, Some(&payload)).await?;

        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            let error_text = response.text().await?;
            if status.as_u16() == 400 && error_text.contains("check-and-set") {
                anyhow::bail!(VaultError::already_exists(format!(
                    "Metadata for '{}' already exists",
                    secret_name
                )));
            }
            anyhow::bail!("Failed to set metadata: {}", error_text)
        }
    }

    pub async fn get_metadata(&self, secret_name: &str) -> anyhow::Result<Option<Metadata>> {
        let url = self.kv_data_path(&format!("transit-metadata/{}", Self::escape(secret_name)));
        let response = self.do_request_raw_rs::<()>(reqwest::Method::GET, &url, None).await?;

        let status = response.status();
        if status.as_u16() == 404 {
            return Ok(None);
        }

        if !status.is_success() {
            let error_text = response.text().await?;
            anyhow::bail!("Failed to get metadata: {}", error_text)
        }

        let response_text = response.text().await?;
        let result: KVReadResponse<Metadata> = serde_json::from_str(&response_text)?;
        Ok(Some(result.data.data))
    }

    pub async fn delete_metadata(&self, secret_name: &str) -> anyhow::Result<()> {
        let url = self.kv_meta_path(&format!("transit-metadata/{}", Self::escape(secret_name)));

        let response = self.do_request_raw_rs::<()>(reqwest::Method::DELETE, &url, None).await?;

        let status = response.status();
        if status.is_success() {
            Ok(())
        } else if status.as_u16() == 404 {
            anyhow::bail!(VaultError::not_found(format!("Metadata '{}' not found", secret_name)));
        } else {
            let error_text = response.text().await?;
            anyhow::bail!("Failed to delete metadata: {}", error_text)
        }
    }

    pub async fn write_blob(
        &self,
        secret_name: &str,
        data_b64: &str,
        mode: StoreMode,
        exists: Option<bool>,
    ) -> anyhow::Result<()> {
        // For ReplaceExists, we still need the existence check (non-critical race).
        if mode == StoreMode::ReplaceExists {
            let exists = match exists {
                Some(e) => e,
                None => self.get_metadata(secret_name).await?.is_some(),
            };
            if !exists {
                anyhow::bail!(VaultError::not_found(format!("Secret '{}' not found", secret_name)));
            }
        }

        let url = self.kv_data_path(&format!("blobs/{}", Self::escape(secret_name)));

        let blob_data =
            BlobData { data: data_b64.to_string(), created_at: chrono::Utc::now().to_rfc3339() };
        let mut payload = serde_json::json!({ "data": blob_data });

        if mode == StoreMode::NewOnly {
            payload["options"] = serde_json::json!({ "cas": 0 }); //  CAS=0 for NewOnly: Vault will reject the write if the key already exists.
        }

        let response = self.do_request_raw_rs(reqwest::Method::POST, &url, Some(&payload)).await?;

        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            let error_text = response.text().await?;
            if mode == StoreMode::NewOnly
                && status.as_u16() == 400
                && error_text.contains("check-and-set")
            {
                anyhow::bail!(VaultError::already_exists(format!(
                    "Secret with id '{}' already exists",
                    secret_name
                )));
            }
            anyhow::bail!("Failed to write blob: {}", error_text)
        }
    }

    pub async fn read_blob(&self, secret_name: &str) -> anyhow::Result<BlobData> {
        let url = self.kv_data_path(&format!("blobs/{}", Self::escape(secret_name)));

        let response = self.do_request_raw_rs::<()>(reqwest::Method::GET, &url, None).await?;

        let status = response.status();
        if status.as_u16() == 404 {
            anyhow::bail!(VaultError::not_found(format!("Blob '{}' not found", secret_name)));
        }

        if !status.is_success() {
            let error_text = response.text().await?;
            anyhow::bail!("Failed to read blob: {}", error_text);
        }

        let response_text = response.text().await?;
        let result: KVReadResponse<BlobData> = serde_json::from_str(&response_text)?;
        Ok(result.data.data)
    }

    pub async fn delete_blob(&self, secret_name: &str) -> anyhow::Result<()> {
        let url = self.kv_meta_path(&format!("blobs/{}", Self::escape(secret_name)));

        let response = self.do_request_raw_rs::<()>(reqwest::Method::DELETE, &url, None).await?;

        let status = response.status();
        if status.is_success() {
            Ok(())
        } else if status.as_u16() == 404 {
            anyhow::bail!(VaultError::not_found(format!("Blob '{}' not found", secret_name)));
        } else {
            let error_text = response.text().await?;
            anyhow::bail!("Failed to delete blob: {}", error_text)
        }
    }

    pub async fn list_blobs(&self) -> anyhow::Result<Vec<String>> {
        let url = self.kv_meta_path("blobs");

        let result: Option<ListKeysResponse> =
            self.do_request_no_body(reqwest::Method::from_bytes("LIST".as_bytes())?, &url).await?;

        Ok(result.unwrap_or_default().data.keys)
    }

    async fn do_request_no_body<Rs>(
        &self,
        method: reqwest::Method,
        url: &str,
    ) -> anyhow::Result<Option<Rs>>
    where
        for<'a> Rs: serde::Deserialize<'a>,
    {
        self.do_request::<(), Rs>(method, url, None).await
    }

    async fn do_request<Rq, Rs>(
        &self,
        method: reqwest::Method,
        url: &str,
        rq: Option<&Rq>,
    ) -> anyhow::Result<Option<Rs>>
    where
        Rq: serde::Serialize + ?Sized,
        for<'a> Rs: serde::Deserialize<'a>,
    {
        let rs = self.do_request_raw_rs(method, url, rq).await?;

        if rs.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        let is_success = rs.status().is_success();
        let rs_payload = rs.text().await?;

        if !is_success {
            anyhow::bail!("Request failed: {rs_payload}");
        }

        let result: Rs = serde_json::from_str(&rs_payload)?;

        Ok(Some(result))
    }

    async fn do_request_raw_rs<Rq>(
        &self,
        method: reqwest::Method,
        url: &str,
        rq: Option<&Rq>,
    ) -> anyhow::Result<reqwest::Response>
    where
        Rq: serde::Serialize + ?Sized,
    {
        let token = self.token_provider.token().await?;
        self.request_with_token(&token, method, url, rq).await
    }

    async fn request_with_token<Rq>(
        &self,
        token: &ProtectedMemory,
        method: reqwest::Method,
        url: &str,
        rq: Option<&Rq>,
    ) -> anyhow::Result<reqwest::Response>
    where
        Rq: serde::Serialize + ?Sized,
    {
        let mut rq_builder =
            self.client.request(method, url).header("X-Vault-Token", token.lock()?.as_ref());

        if let Some(rq_payload) = rq {
            rq_builder = rq_builder.json(rq_payload);
        }

        Ok(rq_builder.send().await?)
    }

    pub fn escape(s: &str) -> String {
        s.replace('+', "-").replace('/', "_").trim_end_matches('=').to_string()
    }
}
