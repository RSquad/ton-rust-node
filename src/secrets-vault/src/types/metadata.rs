/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::types::{
    algorithm::Algorithm, payload::PayloadType, secret_id::SecretId, secret_spec::SecretSpec,
};
use std::collections::HashMap;

#[derive(Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct Metadata {
    pub secret_id: Option<SecretId>,
    pub algorithm: Algorithm,
    pub extractable: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub tags: HashMap<String, String>,
}

impl Metadata {
    pub fn from_spec(secret_id: Option<&SecretId>, spec: &SecretSpec) -> Self {
        let mut metadata = Self::new(secret_id, spec.algorithm, spec.extractable);
        if let Some(expires_at) = spec.expires_at {
            metadata = metadata.with_expiration(expires_at);
        }
        if !spec.tags.is_empty() {
            metadata = metadata.with_tags(spec.tags.clone());
        }

        metadata
    }

    pub fn new(secret_id: Option<&SecretId>, algorithm: Algorithm, extractable: bool) -> Self {
        Self {
            secret_id: secret_id.cloned(),
            algorithm,
            extractable,
            created_at: chrono::Utc::now(),
            expires_at: None,
            tags: HashMap::new(),
        }
    }

    pub fn with_expiration(mut self, expires_at: chrono::DateTime<chrono::Utc>) -> Self {
        self.expires_at = Some(expires_at);
        self
    }

    pub fn with_tag(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.tags.insert(key.into(), value.into());
        self
    }

    pub fn with_tags(mut self, tags: HashMap<String, String>) -> Self {
        self.tags.extend(tags);
        self
    }

    pub fn is_expired(&self) -> bool {
        self.expires_at.map(|exp| chrono::Utc::now() > exp).unwrap_or(false)
    }

    pub fn can_sign(&self) -> bool {
        self.algorithm.can_sign()
    }

    pub fn can_encrypt(&self) -> bool {
        self.algorithm.can_encrypt()
    }

    pub fn is_blob(&self) -> bool {
        self.algorithm.payload_type() == PayloadType::Blob
    }

    pub fn is_symmetric(&self) -> bool {
        self.algorithm.payload_type() == PayloadType::SymmetricKey
    }

    pub fn is_asymmetric(&self) -> bool {
        self.algorithm.payload_type() == PayloadType::KeyPair
    }

    pub fn get_tag(&self, key: &str) -> Option<&str> {
        self.tags.get(key).map(|s| s.as_str())
    }

    pub fn get_tag_str(&self, key: &str) -> anyhow::Result<String> {
        let str = self.tags.get(key).ok_or_else(|| anyhow::anyhow!("Tag `{}` not found", key))?;
        Ok(str.clone())
    }

    pub fn get_tag_i32(&self, key: &str) -> anyhow::Result<i32> {
        let str = self.tags.get(key).ok_or_else(|| anyhow::anyhow!("Tag `{}` not found", key))?;
        let val = str.parse::<i32>().or_else(|_| anyhow::bail!("not a valid i32"))?;
        Ok(val)
    }

    pub fn get_tag_blob_hex(&self, key: &str) -> anyhow::Result<Vec<u8>> {
        let str = self.tags.get(key).ok_or_else(|| anyhow::anyhow!("Tag `{}` not found", key))?;
        let data = hex::decode(str)?;
        Ok(data)
    }

    pub fn get_tag_blob_b64(&self, key: &str) -> anyhow::Result<Vec<u8>> {
        let str = self.tags.get(key).ok_or_else(|| anyhow::anyhow!("Tag `{}` not found", key))?;
        let data = base64::decode(str)?;
        Ok(data)
    }
}

impl PartialEq for Metadata {
    fn eq(&self, other: &Self) -> bool {
        self.secret_id == other.secret_id
            && self.algorithm == other.algorithm
            && self.extractable == other.extractable
            && self.tags == other.tags
    }
}
