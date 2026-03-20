/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::types::{
    algorithm::Algorithm, operation::Operation, payload::PayloadType, secret_id::SecretId,
};

pub type ErrorSource = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    // 1xx: Secret errors
    #[error("[E{}: Not found] {message}", Self::NOT_FOUND)]
    NotFound {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Already exists] {message}", Self::ALREADY_EXISTS)]
    AlreadyExists {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Expired] {message}", Self::EXPIRED)]
    Expired {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Invalid payload type] {message}", Self::INVALID_PAYLOAD_TYPE)]
    InvalidPayloadType {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Not extractable] {message}", Self::NOT_EXTRACTABLE)]
    NotExtractable {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Operation not supported] {message}", Self::OPERATION_NOT_SUPPORTED)]
    OperationNotSupported {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Empty secret id] {message}", Self::EMPTY_SECRET_ID)]
    EmptySecretId {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Empty public key] {message}", Self::EMPTY_PUBLIC_KEY)]
    EmptyPublicKey {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Empty secret key] {message}", Self::EMPTY_SECRET_KEY)]
    EmptySecretKey {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Empty crypto implementation] {message}", Self::EMPTY_CRYPTO)]
    EmptyCrypto {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: The key is already set] {message}", Self::KEY_IS_ALREADY_SET)]
    KeyIsAlreadySet {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Wrong secret type] {message}", Self::WRONG_SECRET_TYPE)]
    WrongSecretType {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Invalid key size] {message}", Self::INVALID_KEY_SIZE)]
    InvalidKeySize {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Empty metadata] {message}", Self::EMPTY_METADATA)]
    EmptyMetadata {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    // 2xx: Cryptographic errors
    #[error("[E{}: Unsupported algorithm] {message}", Self::UNSUPPORTED_ALGORITHM)]
    UnsupportedAlgorithm {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Invalid signature] {message}", Self::INVALID_SIGNATURE)]
    InvalidSignature {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Decryption failed] {message}", Self::DECRYPTION_FAILED)]
    DecryptionFailed {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Encryption failed] {message}", Self::ENCRYPTION_FAILED)]
    EncryptionFailed {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Key generation failed] {message}", Self::KEY_GENERATION_FAILED)]
    KeyGenerationFailed {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Invalid key material] {message}", Self::INVALID_KEY_MATERIAL)]
    InvalidKeyMaterial {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Invalid master key] {message}", Self::INVALID_MASTER_KEY)]
    InvalidMasterKey {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Invalid private key] {message}", Self::INVALID_PRIVATE_KEY)]
    InvalidPrivateKey {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Invalid public key] {message}", Self::INVALID_PUBLIC_KEY)]
    InvalidPublicKey {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    // 3xx: Storage errors
    #[error("[E{}: Storage unavailable] {message}", Self::STORAGE_UNAVAILABLE)]
    StorageUnavailable {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Storage corrupted] {message}", Self::STORAGE_CORRUPTED)]
    StorageCorrupted {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Integrity check failed] {message}", Self::INTEGRITY_CHECK_FAILED)]
    IntegrityCheckFailed {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Storage read error] {message}", Self::STORAGE_READ_ERROR)]
    StorageReadError {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Storage write error] {message}", Self::STORAGE_WRITE_ERROR)]
    StorageWriteError {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Lock acquisition failed] {message}", Self::LOCK_ACQUISITION_FAILED)]
    LockAcquisitionFailed {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Lock timeout] {message}", Self::LOCK_TIMEOUT)]
    LockTimeout {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    // 4xx: Backend-specific errors
    #[error("[E{}: Backend connection failed] {message}", Self::BACKEND_CONNECTION_FAILED)]
    BackendConnectionFailed {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Backend auth failed] {message}", Self::BACKEND_AUTH_FAILED)]
    BackendAuthFailed {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Backend operation failed] {message}", Self::BACKEND_OPERATION_FAILED)]
    BackendOperationFailed {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Backend invalid response] {message}", Self::BACKEND_INVALID_RESPONSE)]
    BackendInvalidResponse {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    // 5xx: Configuration errors
    #[error("[E{}: Invalid configuration] {message}", Self::INVALID_CONFIG)]
    InvalidConfig {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Invalid configuration URL] {message}", Self::INVALID_CONFIG_URL)]
    InvalidConfigUrl {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Master key unavailable] {message}", Self::MASTER_KEY_UNAVAILABLE)]
    MasterKeyUnavailable {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Feature not enabled] {message}", Self::FEATURE_NOT_ENABLED)]
    FeatureNotEnabled {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Storage not set] {message}", Self::STORAGE_NOT_SET)]
    StorageNotSet {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    // 6xx: Other errors
    #[error("[E{}: Internal error] {message}", Self::INTERNAL_ERROR)]
    InternalError {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Serialization error] {message}", Self::SERIALIZATION_ERROR)]
    SerializationError {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("[E{}: Deserialization error] {message}", Self::DESERIALIZATION_ERROR)]
    DeserializationError {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
}

macro_rules! impl_with_source {
    ($self:expr, $source:expr, $($variant:ident),+) => {
        match $self {
            $(
                VaultError::$variant { message, .. } => {
                    VaultError::$variant { message, source: Some($source) }
                }
            )+
        }
    };
}

impl VaultError {
    // 1xx: Secret errors
    pub const NOT_FOUND: u16 = 100;
    pub const ALREADY_EXISTS: u16 = 101;
    pub const EXPIRED: u16 = 102;
    pub const INVALID_PAYLOAD_TYPE: u16 = 103;
    pub const NOT_EXTRACTABLE: u16 = 104;
    pub const OPERATION_NOT_SUPPORTED: u16 = 105;
    pub const EMPTY_SECRET_ID: u16 = 106;
    pub const EMPTY_PUBLIC_KEY: u16 = 107;
    pub const EMPTY_SECRET_KEY: u16 = 108;
    pub const EMPTY_CRYPTO: u16 = 109;
    pub const KEY_IS_ALREADY_SET: u16 = 110;
    pub const WRONG_SECRET_TYPE: u16 = 111;
    pub const INVALID_KEY_SIZE: u16 = 112;
    pub const EMPTY_METADATA: u16 = 113;

    // 2xx: Cryptographic errors
    pub const UNSUPPORTED_ALGORITHM: u16 = 200;
    pub const INVALID_SIGNATURE: u16 = 201;
    pub const DECRYPTION_FAILED: u16 = 202;
    pub const ENCRYPTION_FAILED: u16 = 203;
    pub const KEY_GENERATION_FAILED: u16 = 204;
    pub const INVALID_KEY_MATERIAL: u16 = 205;
    pub const INVALID_MASTER_KEY: u16 = 206;
    pub const INVALID_PRIVATE_KEY: u16 = 207;
    pub const INVALID_PUBLIC_KEY: u16 = 208;

    // 3xx: Storage errors
    pub const STORAGE_UNAVAILABLE: u16 = 300;
    pub const STORAGE_CORRUPTED: u16 = 301;
    pub const INTEGRITY_CHECK_FAILED: u16 = 302;
    pub const STORAGE_READ_ERROR: u16 = 303;
    pub const STORAGE_WRITE_ERROR: u16 = 304;
    pub const LOCK_ACQUISITION_FAILED: u16 = 305;
    pub const LOCK_TIMEOUT: u16 = 306;

    // 4xx: Backend-specific errors
    pub const BACKEND_CONNECTION_FAILED: u16 = 400;
    pub const BACKEND_AUTH_FAILED: u16 = 401;
    pub const BACKEND_OPERATION_FAILED: u16 = 402;
    pub const BACKEND_INVALID_RESPONSE: u16 = 403;

    // 5xx: Configuration errors
    pub const INVALID_CONFIG: u16 = 500;
    pub const INVALID_CONFIG_URL: u16 = 501;
    pub const MASTER_KEY_UNAVAILABLE: u16 = 502;
    pub const FEATURE_NOT_ENABLED: u16 = 503;
    pub const STORAGE_NOT_SET: u16 = 504;

    // 6xx: Other errors
    pub const INTERNAL_ERROR: u16 = 600;
    pub const SERIALIZATION_ERROR: u16 = 601;
    pub const DESERIALIZATION_ERROR: u16 = 602;

    pub fn code(&self) -> u16 {
        match self {
            VaultError::NotFound { .. } => Self::NOT_FOUND,
            VaultError::AlreadyExists { .. } => Self::ALREADY_EXISTS,
            VaultError::Expired { .. } => Self::EXPIRED,
            VaultError::InvalidPayloadType { .. } => Self::INVALID_PAYLOAD_TYPE,
            VaultError::NotExtractable { .. } => Self::NOT_EXTRACTABLE,
            VaultError::OperationNotSupported { .. } => Self::OPERATION_NOT_SUPPORTED,
            VaultError::EmptySecretId { .. } => Self::EMPTY_SECRET_ID,
            VaultError::EmptyPublicKey { .. } => Self::EMPTY_PUBLIC_KEY,
            VaultError::EmptySecretKey { .. } => Self::EMPTY_SECRET_KEY,
            VaultError::EmptyCrypto { .. } => Self::EMPTY_CRYPTO,
            VaultError::KeyIsAlreadySet { .. } => Self::KEY_IS_ALREADY_SET,
            VaultError::WrongSecretType { .. } => Self::WRONG_SECRET_TYPE,
            VaultError::InvalidKeySize { .. } => Self::INVALID_KEY_SIZE,
            VaultError::EmptyMetadata { .. } => Self::EMPTY_METADATA,
            VaultError::UnsupportedAlgorithm { .. } => Self::UNSUPPORTED_ALGORITHM,
            VaultError::InvalidSignature { .. } => Self::INVALID_SIGNATURE,
            VaultError::DecryptionFailed { .. } => Self::DECRYPTION_FAILED,
            VaultError::EncryptionFailed { .. } => Self::ENCRYPTION_FAILED,
            VaultError::KeyGenerationFailed { .. } => Self::KEY_GENERATION_FAILED,
            VaultError::InvalidKeyMaterial { .. } => Self::INVALID_KEY_MATERIAL,
            VaultError::InvalidMasterKey { .. } => Self::INVALID_MASTER_KEY,
            VaultError::InvalidPrivateKey { .. } => Self::INVALID_PRIVATE_KEY,
            VaultError::InvalidPublicKey { .. } => Self::INVALID_PUBLIC_KEY,
            VaultError::StorageUnavailable { .. } => Self::STORAGE_UNAVAILABLE,
            VaultError::StorageCorrupted { .. } => Self::STORAGE_CORRUPTED,
            VaultError::IntegrityCheckFailed { .. } => Self::INTEGRITY_CHECK_FAILED,
            VaultError::StorageReadError { .. } => Self::STORAGE_READ_ERROR,
            VaultError::StorageWriteError { .. } => Self::STORAGE_WRITE_ERROR,
            VaultError::LockAcquisitionFailed { .. } => Self::LOCK_ACQUISITION_FAILED,
            VaultError::LockTimeout { .. } => Self::LOCK_TIMEOUT,
            VaultError::BackendConnectionFailed { .. } => Self::BACKEND_CONNECTION_FAILED,
            VaultError::BackendAuthFailed { .. } => Self::BACKEND_AUTH_FAILED,
            VaultError::BackendOperationFailed { .. } => Self::BACKEND_OPERATION_FAILED,
            VaultError::BackendInvalidResponse { .. } => Self::BACKEND_INVALID_RESPONSE,
            VaultError::InvalidConfig { .. } => Self::INVALID_CONFIG,
            VaultError::InvalidConfigUrl { .. } => Self::INVALID_CONFIG_URL,
            VaultError::MasterKeyUnavailable { .. } => Self::MASTER_KEY_UNAVAILABLE,
            VaultError::FeatureNotEnabled { .. } => Self::FEATURE_NOT_ENABLED,
            VaultError::StorageNotSet { .. } => Self::STORAGE_NOT_SET,
            VaultError::InternalError { .. } => Self::INTERNAL_ERROR,
            VaultError::SerializationError { .. } => Self::SERIALIZATION_ERROR,
            VaultError::DeserializationError { .. } => Self::DESERIALIZATION_ERROR,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            VaultError::NotFound { message, .. }
            | VaultError::AlreadyExists { message, .. }
            | VaultError::Expired { message, .. }
            | VaultError::InvalidPayloadType { message, .. }
            | VaultError::NotExtractable { message, .. }
            | VaultError::OperationNotSupported { message, .. }
            | VaultError::EmptySecretId { message, .. }
            | VaultError::EmptyPublicKey { message, .. }
            | VaultError::EmptySecretKey { message, .. }
            | VaultError::EmptyCrypto { message, .. }
            | VaultError::KeyIsAlreadySet { message, .. }
            | VaultError::WrongSecretType { message, .. }
            | VaultError::InvalidKeySize { message, .. }
            | VaultError::EmptyMetadata { message, .. }
            | VaultError::UnsupportedAlgorithm { message, .. }
            | VaultError::InvalidSignature { message, .. }
            | VaultError::DecryptionFailed { message, .. }
            | VaultError::EncryptionFailed { message, .. }
            | VaultError::KeyGenerationFailed { message, .. }
            | VaultError::InvalidKeyMaterial { message, .. }
            | VaultError::InvalidMasterKey { message, .. }
            | VaultError::InvalidPrivateKey { message, .. }
            | VaultError::InvalidPublicKey { message, .. }
            | VaultError::StorageUnavailable { message, .. }
            | VaultError::StorageCorrupted { message, .. }
            | VaultError::IntegrityCheckFailed { message, .. }
            | VaultError::StorageReadError { message, .. }
            | VaultError::StorageWriteError { message, .. }
            | VaultError::LockAcquisitionFailed { message, .. }
            | VaultError::LockTimeout { message, .. }
            | VaultError::BackendConnectionFailed { message, .. }
            | VaultError::BackendAuthFailed { message, .. }
            | VaultError::BackendOperationFailed { message, .. }
            | VaultError::BackendInvalidResponse { message, .. }
            | VaultError::InvalidConfig { message, .. }
            | VaultError::InvalidConfigUrl { message, .. }
            | VaultError::MasterKeyUnavailable { message, .. }
            | VaultError::FeatureNotEnabled { message, .. }
            | VaultError::StorageNotSet { message, .. }
            | VaultError::InternalError { message, .. }
            | VaultError::SerializationError { message, .. }
            | VaultError::DeserializationError { message, .. } => message,
        }
    }

    pub fn with_source(self, source: impl std::error::Error + Send + Sync + 'static) -> Self {
        let boxed_source = Box::new(source);
        impl_with_source!(
            self,
            boxed_source,
            NotFound,
            AlreadyExists,
            Expired,
            InvalidPayloadType,
            NotExtractable,
            OperationNotSupported,
            EmptySecretId,
            EmptyPublicKey,
            EmptySecretKey,
            EmptyCrypto,
            KeyIsAlreadySet,
            WrongSecretType,
            InvalidKeySize,
            EmptyMetadata,
            UnsupportedAlgorithm,
            InvalidSignature,
            DecryptionFailed,
            EncryptionFailed,
            KeyGenerationFailed,
            InvalidKeyMaterial,
            InvalidMasterKey,
            InvalidPrivateKey,
            InvalidPublicKey,
            StorageUnavailable,
            StorageCorrupted,
            IntegrityCheckFailed,
            StorageReadError,
            StorageWriteError,
            LockAcquisitionFailed,
            LockTimeout,
            BackendConnectionFailed,
            BackendAuthFailed,
            BackendOperationFailed,
            BackendInvalidResponse,
            InvalidConfig,
            InvalidConfigUrl,
            MasterKeyUnavailable,
            FeatureNotEnabled,
            StorageNotSet,
            InternalError,
            SerializationError,
            DeserializationError
        )
    }

    pub fn invalid_payload_type(expected: &PayloadType, actual: &PayloadType) -> Self {
        VaultError::InvalidPayloadType {
            message: format!("Invalid payload type: expected {}, got {}", expected, actual),
            source: None,
        }
    }

    pub fn invalid_key_material(msg: impl Into<String>) -> Self {
        VaultError::InvalidKeyMaterial { message: msg.into(), source: None }
    }

    pub fn invalid_master_key(msg: impl Into<String>) -> Self {
        VaultError::InvalidMasterKey { message: msg.into(), source: None }
    }

    pub fn invalid_private_key(msg: impl Into<String>) -> Self {
        VaultError::InvalidPrivateKey { message: msg.into(), source: None }
    }

    pub fn invalid_public_key(msg: impl Into<String>) -> Self {
        VaultError::InvalidPublicKey { message: msg.into(), source: None }
    }

    pub fn operation_not_supported(
        secret_id: Option<&SecretId>,
        algo: Algorithm,
        operation: &Operation,
    ) -> Self {
        VaultError::OperationNotSupported {
            message: format!(
                "Secret '{}' with algorithm {} does not support operation: {}",
                secret_id.map(|id| id.to_string()).unwrap_or_else(|| "unknown".to_string()),
                algo.as_str(),
                operation
            ),
            source: None,
        }
    }

    pub fn empty_secret_id(msg: impl Into<String>) -> Self {
        VaultError::EmptySecretId { message: msg.into(), source: None }
    }

    pub fn empty_public_key(msg: impl Into<String>) -> Self {
        VaultError::EmptyPublicKey { message: msg.into(), source: None }
    }

    pub fn empty_secret_key(msg: impl Into<String>) -> Self {
        VaultError::EmptySecretKey { message: msg.into(), source: None }
    }

    pub fn empty_crypto(msg: impl Into<String>) -> Self {
        VaultError::EmptyCrypto { message: msg.into(), source: None }
    }

    pub fn key_is_already_set(msg: impl Into<String>) -> Self {
        VaultError::KeyIsAlreadySet { message: msg.into(), source: None }
    }

    pub fn wrong_secret_type(msg: impl Into<String>) -> Self {
        VaultError::WrongSecretType { message: msg.into(), source: None }
    }

    pub fn invalid_key_size(msg: impl Into<String>) -> Self {
        VaultError::InvalidKeySize { message: msg.into(), source: None }
    }

    pub fn empty_metadata(msg: impl Into<String>) -> Self {
        VaultError::EmptyMetadata { message: msg.into(), source: None }
    }

    pub fn backend_operation_failed(msg: impl Into<String>) -> Self {
        VaultError::BackendOperationFailed {
            message: format!("Remote operations not supported by this backend: {}", msg.into()),
            source: None,
        }
    }

    pub fn master_key_unavailable(msg: impl Into<String>) -> Self {
        VaultError::MasterKeyUnavailable { message: msg.into(), source: None }
    }

    pub fn already_exists(msg: impl Into<String>) -> Self {
        VaultError::AlreadyExists { message: msg.into(), source: None }
    }

    pub fn not_extractable(secret_id: Option<&SecretId>) -> Self {
        VaultError::NotExtractable {
            message: format!(
                "Secret with id '{}' is not extractable",
                secret_id.map(|id| id.to_string()).unwrap_or_else(|| "unknown".to_string())
            ),
            source: None,
        }
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        VaultError::NotFound { message: msg.into(), source: None }
    }

    pub fn unsupported_algorithm(algo: Algorithm) -> Self {
        VaultError::UnsupportedAlgorithm {
            message: format!("Unsupported algorithm: {}", algo.as_str()),
            source: None,
        }
    }

    pub fn invalid_signature(msg: impl Into<String>) -> Self {
        VaultError::InvalidSignature { message: msg.into(), source: None }
    }

    pub fn encryption_failed(msg: impl Into<String>) -> Self {
        VaultError::EncryptionFailed { message: msg.into(), source: None }
    }

    pub fn decryption_failed(msg: impl Into<String>) -> Self {
        VaultError::DecryptionFailed { message: msg.into(), source: None }
    }

    pub fn storage_corrupted(msg: impl Into<String>) -> Self {
        VaultError::StorageCorrupted { message: msg.into(), source: None }
    }

    pub fn invalid_config_url(msg: impl Into<String>) -> Self {
        VaultError::InvalidConfigUrl { message: msg.into(), source: None }
    }

    pub fn storage_not_set(msg: impl Into<String>) -> Self {
        VaultError::StorageNotSet { message: msg.into(), source: None }
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        VaultError::InternalError {
            message: format!("Internal error: {}", msg.into()),
            source: None,
        }
    }
}

pub type VaultResult<T> = Result<T, VaultError>;
