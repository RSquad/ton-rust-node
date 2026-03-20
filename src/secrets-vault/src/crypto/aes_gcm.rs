/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    crypto::{key_material::KeyMaterial, prng::Prng},
    errors::error::VaultError,
    memory::protected_memory::{ProtectedMemory, WriteGuard},
    utils::process::interrupt,
};
use aes_gcm::{aead::Aead, AeadInPlace, KeyInit};

pub(crate) const NONCE_SIZE: usize = 12;
pub(crate) const TAG_SIZE: usize = 16;

pub async fn encrypt(
    key: &KeyMaterial,
    plaintext: &[u8],
    prng: &dyn Prng,
) -> anyhow::Result<Vec<u8>> {
    // Output format: [nonce (12 bytes)][ciphertext][tag (16 bytes)]
    let secret_key = key
        .secret_key
        .as_ref()
        .ok_or_else(|| VaultError::empty_secret_key("Secret key is not set"))?;

    let secret_key_len = secret_key.len().await;

    if secret_key_len != 32 {
        anyhow::bail!(VaultError::invalid_key_size(format!(
            "Invalid AES key length: {}",
            secret_key_len
        )))
    }

    let mut nonce_bytes = [0u8; NONCE_SIZE];
    prng.fill_random(&mut nonce_bytes).await?;
    let nonce = aes_gcm::Nonce::from_slice(&nonce_bytes);

    let key_read_lock = secret_key.lock().await?;
    let cipher = aes_gcm::Aes256Gcm::new_from_slice(&key_read_lock)
        .map_err(|e| VaultError::internal(format!("Invalid AES-256 key: {}", e)))?;
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| VaultError::encryption_failed(format!("AES-GCM encryption failed: {}", e)))?;

    let mut result = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
    result.extend_from_slice(&nonce_bytes);
    result.extend_from_slice(&ciphertext);

    Ok(result)
}

pub async fn decrypt(
    key_material: &KeyMaterial,
    ciphertext: &[u8],
) -> anyhow::Result<ProtectedMemory> {
    // Input format: [nonce (12 bytes)][ciphertext][tag (16 bytes)]
    if ciphertext.len() < NONCE_SIZE + TAG_SIZE {
        anyhow::bail!(VaultError::decryption_failed("Ciphertext too short"))
    }

    let secret_key = key_material
        .secret_key
        .as_ref()
        .ok_or_else(|| VaultError::empty_secret_key("Secret key is not set"))?;

    let secret_key_len = secret_key.len().await;

    if secret_key_len != 32 {
        anyhow::bail!(VaultError::invalid_key_size(format!(
            "Invalid AES key length: {}",
            secret_key_len
        )))
    }

    let nonce = aes_gcm::Nonce::from_slice(&ciphertext[..NONCE_SIZE]);
    let encrypted_data = &ciphertext[NONCE_SIZE..];

    let key_read_lock = secret_key.lock().await?;
    let cipher = aes_gcm::Aes256Gcm::new_from_slice(&key_read_lock)
        .map_err(|e| VaultError::internal(format!("Invalid AES-256 key: {}", e)))?;

    let mut decrypted = ProtectedMemory::new(0)?;

    {
        let mut decrypted_write_guard = decrypted.lock_mut().await?;
        decrypted_write_guard.extend_from_slice(encrypted_data)?;
        cipher.decrypt_in_place(nonce, b"", &mut decrypted_write_guard).map_err(|e| {
            let err = VaultError::decryption_failed(format!("AES-GCM decryption failed: {}", e));
            err
        })?
    }

    Ok(decrypted)
}

impl<'a> aes_gcm::aead::Buffer for WriteGuard<'a> {
    fn extend_from_slice(&mut self, other: &[u8]) -> aes_gcm::aead::Result<()> {
        WriteGuard::extend_from_slice(self, other).map_err(|_| aes_gcm::aead::Error)
    }

    fn truncate(&mut self, len: usize) {
        if let Err(e) = WriteGuard::truncate(self, len) {
            eprintln!("Error when truncating AES GCM buffer: {e}. Exiting...");
            interrupt();
        }
    }
}
