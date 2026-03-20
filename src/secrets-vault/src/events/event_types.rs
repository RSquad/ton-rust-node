/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::types::{algorithm::Algorithm, secret_id::SecretId};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    // Lifecycle
    KeyGenerated, // A new key was generated
    KeyImported,  // A key was imported
    KeyDeleted,   // A key was deleted
    KeyLoaded,    // A private key was loaded into memory
    KeyUnloaded,  // A private key was cleared from memory

    // Crypto operations
    SignatureCreated,    // A signature was created
    SignatureVerified,   // A signature was verified
    EncryptionPerformed, // Data was encrypted
    DecryptionPerformed, // Data was decrypted

    // Access events
    PublicKeyExported,  // A public key was exported
    PrivateKeyExported, // A private key was exported

    // Failures
    OperationFailed, // An operation failed
    KeyNotFound,     // A key was not found
    AccessDenied,    // Access was denied
}

// Event record
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Event {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub event_id: uuid::Uuid,
    pub event_type: EventType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_id: Option<SecretId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub algorithm: Option<Algorithm>,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
}

impl Event {
    pub fn new(event_type: EventType) -> Self {
        Self {
            timestamp: chrono::Utc::now(),
            event_id: uuid::Uuid::new_v4(),
            event_type,
            secret_id: None,
            algorithm: None,
            success: true,
            error_code: None,
            error_message: None,
            metadata: HashMap::new(),
        }
    }

    pub fn with_secret_id(mut self, secret_id: &SecretId) -> Self {
        self.secret_id = Some(secret_id.clone());
        self
    }

    pub fn with_algorithm(mut self, algorithm: Algorithm) -> Self {
        self.algorithm = Some(algorithm);
        self
    }

    pub fn failed(mut self, code: u16, message: impl Into<String>) -> Self {
        self.success = false;
        self.error_code = Some(code);
        self.error_message = Some(message.into());
        self
    }

    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }
}
