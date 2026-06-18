/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    crypto::{
        aes_gcm::{decrypt, encrypt, NONCE_SIZE, TAG_SIZE},
        factory::{CryptoFactory, DefaultCryptoFactory},
        key_material::KeyMaterial,
        prng::Prng,
        prng_chacha20::PrngChacha20,
    },
    errors::error::VaultError,
    memory::protected_memory::{ProtectedMemory, ProtectedMemoryInner},
    types::algorithm::Algorithm,
};

async fn generate_key(size: usize) -> anyhow::Result<KeyMaterial> {
    let crypto = DefaultCryptoFactory {}.new_crypto()?;
    KeyMaterial::generate_new(Algorithm::Aes256Gcm, Some(size), crypto.as_ref())
}

#[tokio::test]
async fn test_generate_key() -> anyhow::Result<()> {
    let key: KeyMaterial = generate_key(32).await.unwrap();
    assert_eq!(key.secret_key.as_ref().ok_or_else(|| VaultError::empty_secret_key(""))?.len(), 32);

    Ok(())
}

#[tokio::test]
async fn test_generate_multiple_keys_are_different() -> anyhow::Result<()> {
    let key1 = generate_key(32).await.unwrap();
    let key2 = generate_key(32).await.unwrap();

    let key1_read = key1
        .secret_key
        .as_ref()
        .ok_or_else(|| VaultError::empty_secret_key("Secret key is not set"))?
        .lock()
        .unwrap();
    let key2_read = key2
        .secret_key
        .as_ref()
        .ok_or_else(|| VaultError::empty_secret_key("Secret key is not set"))?
        .lock()
        .unwrap();

    assert_ne!(&*key1_read, &*key2_read);

    Ok(())
}

#[tokio::test]
async fn test_generate_key_zero_size_fails() {
    let result = generate_key(0).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_encrypt_decrypt_short_message() {
    let key = generate_key(32).await.unwrap();
    let plaintext = b"Hello";

    let prng = PrngChacha20 {};
    let ciphertext = encrypt(&key, plaintext, &prng).unwrap();
    let decrypted = decrypt(&key, &ciphertext).unwrap();
    let decrypted_read = decrypted.lock().unwrap();

    assert_eq!(&*decrypted_read, plaintext);
}

#[tokio::test]
async fn test_encrypt_decrypt_medium_message() {
    let key = generate_key(32).await.unwrap();
    let plaintext = b"Some very long secret message must be here: -^=^-, -0=0-, -6=6-, -9^=9-";

    let prng = PrngChacha20 {};
    let ciphertext = encrypt(&key, plaintext, &prng).unwrap();
    let decrypted = decrypt(&key, &ciphertext).unwrap();
    let decrypted_read = decrypted.lock().unwrap();

    assert_eq!(&*decrypted_read, plaintext);
}

#[tokio::test]
async fn test_encrypt_decrypt_empty_message() {
    let key = generate_key(32).await.unwrap();
    let plaintext = b"";

    let prng = PrngChacha20 {};
    let ciphertext = encrypt(&key, plaintext, &prng).unwrap();

    assert_eq!(ciphertext.len(), NONCE_SIZE + TAG_SIZE);

    let decrypted = decrypt(&key, &ciphertext).unwrap();
    let decrypted_read = decrypted.lock().unwrap();

    assert_eq!(&*decrypted_read, plaintext);
    assert_eq!(decrypted_read.len(), 0);
}

#[tokio::test]
async fn test_encrypt_decrypt_single_byte() {
    let key = generate_key(32).await.unwrap();
    let plaintext = b"X";

    let prng = PrngChacha20 {};
    let ciphertext = encrypt(&key, plaintext, &prng).unwrap();
    let decrypted = decrypt(&key, &ciphertext).unwrap();
    let decrypted_read = decrypted.lock().unwrap();

    assert_eq!(&*decrypted_read, plaintext);
}

#[tokio::test]
async fn test_encrypt_decrypt_large_message() {
    let key = generate_key(32).await.unwrap();
    let mut plaintext = vec![0u8; 10_000];
    let prng = PrngChacha20 {};
    prng.fill_random(&mut plaintext).unwrap();

    let ciphertext = encrypt(&key, &plaintext, &prng).unwrap();
    let decrypted = decrypt(&key, &ciphertext).unwrap();
    let decrypted_read = decrypted.lock().unwrap();

    assert_eq!(&*decrypted_read, &plaintext[..]);
}

#[tokio::test]
async fn test_encrypt_decrypt_very_large_message() {
    let key = generate_key(32).await.unwrap();
    let plaintext = vec![123u8; 1_000_000];

    let prng = PrngChacha20 {};
    let ciphertext = encrypt(&key, &plaintext, &prng).unwrap();
    let decrypted = decrypt(&key, &ciphertext).unwrap();
    let decrypted_read = decrypted.lock().unwrap();

    assert_eq!(&*decrypted_read, &plaintext[..]);
}

#[tokio::test]
async fn test_ciphertext_format() {
    let key = generate_key(32).await.unwrap();
    let plaintext = b"Test message";

    let prng = PrngChacha20 {};
    let ciphertext = encrypt(&key, plaintext, &prng).unwrap();
    let expected_len = NONCE_SIZE + plaintext.len() + TAG_SIZE;
    assert_eq!(ciphertext.len(), expected_len);
}

#[tokio::test]
async fn test_ciphertext_is_different_deserialize() {
    let key = generate_key(32).await.unwrap();
    let plaintext = b"Secret message";

    let prng = PrngChacha20 {};
    let ciphertext = encrypt(&key, plaintext, &prng).unwrap();
    assert!(!ciphertext.windows(plaintext.len()).any(|w| w == plaintext));
}

#[tokio::test]
async fn test_unique_nonces_for_same_plaintext() {
    let key = generate_key(32).await.unwrap();
    let plaintext = b"Same plaintext";

    let prng = PrngChacha20 {};
    let ciphertext1 = encrypt(&key, plaintext, &prng).unwrap();
    let ciphertext2 = encrypt(&key, plaintext, &prng).unwrap();

    assert_ne!(&ciphertext1[..NONCE_SIZE], &ciphertext2[..NONCE_SIZE]);
    assert_ne!(ciphertext1, ciphertext2);

    let decrypted1 = decrypt(&key, &ciphertext1).unwrap();
    let decrypted2 = decrypt(&key, &ciphertext2).unwrap();

    let read1 = decrypted1.lock().unwrap();
    let read2 = decrypted2.lock().unwrap();

    assert_eq!(&*read1, plaintext);
    assert_eq!(&*read2, plaintext);
}

#[tokio::test]
async fn test_multiple_encryptions_produce_different_ciphertexts() {
    let key = generate_key(32).await.unwrap();
    let plaintext = b"Test";

    let prng = PrngChacha20 {};
    let mut ciphertexts = Vec::new();
    for _ in 0..10 {
        ciphertexts.push(encrypt(&key, plaintext, &prng).unwrap());
    }

    for i in 0..ciphertexts.len() {
        for j in i + 1..ciphertexts.len() {
            assert_ne!(ciphertexts[i], ciphertexts[j]);
        }
    }

    for ct in &ciphertexts {
        let decrypted = decrypt(&key, ct).unwrap();
        let read = decrypted.lock().unwrap();
        assert_eq!(&*read, plaintext);
    }
}

#[tokio::test]
async fn test_decrypt_ciphertext_too_short() {
    let key = generate_key(32).await.unwrap();
    let invalid_ciphertext = vec![0u8; 10];

    let result = decrypt(&key, &invalid_ciphertext);
    assert!(result.is_err());
}

#[tokio::test]
async fn test_decrypt_exactly_nonce_size() {
    let key = generate_key(32).await.unwrap();
    let invalid_ciphertext = vec![0u8; NONCE_SIZE];

    let result = decrypt(&key, &invalid_ciphertext);
    assert!(result.is_err());
}

#[tokio::test]
async fn test_decrypt_nonce_plus_partial_tag() {
    let key = generate_key(32).await.unwrap();
    let invalid_ciphertext = vec![0u8; NONCE_SIZE + TAG_SIZE - 1];

    let result = decrypt(&key, &invalid_ciphertext);
    assert!(result.is_err());
}

#[tokio::test]
async fn test_decrypt_corrupted_nonce() {
    let key = generate_key(32).await.unwrap();
    let plaintext = b"Test message";

    let prng = PrngChacha20 {};
    let mut ciphertext = encrypt(&key, plaintext, &prng).unwrap();

    ciphertext[0] ^= 1;

    let result = decrypt(&key, &ciphertext);
    assert!(result.is_err());
}

#[tokio::test]
async fn test_decrypt_corrupted_ciphertext() {
    let key = generate_key(32).await.unwrap();
    let plaintext = b"Test message";

    let prng = PrngChacha20 {};
    let mut ciphertext = encrypt(&key, plaintext, &prng).unwrap();
    ciphertext[NONCE_SIZE + 5] ^= 1;

    let result = decrypt(&key, &ciphertext);
    assert!(result.is_err());
}

#[tokio::test]
async fn test_decrypt_corrupted_tag() {
    let key = generate_key(32).await.unwrap();
    let plaintext = b"Test message";

    let prng = PrngChacha20 {};
    let mut ciphertext = encrypt(&key, plaintext, &prng).unwrap();
    let last_idx = ciphertext.len() - 1;
    ciphertext[last_idx] ^= 1;

    let result = decrypt(&key, &ciphertext);
    assert!(result.is_err());
}

#[tokio::test]
async fn test_decrypt_with_wrong_key() {
    let key1 = generate_key(32).await.unwrap();
    let key2 = generate_key(32).await.unwrap();
    let plaintext = b"Secret message";

    let prng = PrngChacha20 {};
    let ciphertext = encrypt(&key1, plaintext, &prng).unwrap();
    let result = decrypt(&key2, &ciphertext);
    assert!(result.is_err());
}

#[tokio::test]
async fn test_encrypt_with_invalid_key_size() {
    let invalid_key: ProtectedMemory = ProtectedMemoryInner::new(16).unwrap().into();
    let key_material = KeyMaterial::new(Some(invalid_key), None).unwrap();
    let plaintext = b"test";

    let prng = PrngChacha20 {};
    let result = encrypt(&key_material, plaintext, &prng);
    assert!(result.is_err());
}

#[tokio::test]
async fn test_decrypt_with_invalid_key_size() {
    let valid_key = generate_key(32).await.unwrap();
    let plaintext = b"test";

    let prng = PrngChacha20 {};
    let ciphertext = encrypt(&valid_key, plaintext, &prng).unwrap();

    let invalid_key: ProtectedMemory = ProtectedMemoryInner::new(16).unwrap().into();
    let invalid_key_material = KeyMaterial::new(Some(invalid_key), None).unwrap();

    let result = decrypt(&invalid_key_material, &ciphertext);
    assert!(result.is_err());
}

#[tokio::test]
async fn test_decrypt_completely_random_data() {
    let key = generate_key(32).await.unwrap();

    let mut random_data = vec![0u8; NONCE_SIZE + TAG_SIZE + 10];
    let prng = PrngChacha20 {};
    prng.fill_random(&mut random_data).unwrap();

    let result = decrypt(&key, &random_data);
    assert!(result.is_err());
}

#[tokio::test]
async fn test_encrypt_decrypt_all_zeros() {
    let key = generate_key(32).await.unwrap();
    let plaintext = vec![0u8; 100];

    let prng = PrngChacha20 {};
    let ciphertext = encrypt(&key, &plaintext, &prng).unwrap();
    let decrypted = decrypt(&key, &ciphertext).unwrap();
    let decrypted_read = decrypted.lock().unwrap();

    assert_eq!(&*decrypted_read, &plaintext[..]);
}

#[tokio::test]
async fn test_encrypt_decrypt_all_ones() {
    let key = generate_key(32).await.unwrap();
    let plaintext = vec![0xFFu8; 100];

    let prng = PrngChacha20 {};
    let ciphertext = encrypt(&key, &plaintext, &prng).unwrap();
    let decrypted = decrypt(&key, &ciphertext).unwrap();
    let decrypted_read = decrypted.lock().unwrap();

    assert_eq!(&*decrypted_read, &plaintext[..]);
}

#[tokio::test]
async fn test_encrypt_decrypt_page_boundary_sizes() {
    let key = generate_key(32).await.unwrap();
    let sizes = vec![4095, 4096, 4097, 8191, 8192, 8193];
    let prng = PrngChacha20 {};

    for size in sizes {
        let plaintext = vec![42u8; size];
        let ciphertext = encrypt(&key, &plaintext, &prng).unwrap();
        let decrypted = decrypt(&key, &ciphertext).unwrap();
        let decrypted_read = decrypted.lock().unwrap();

        assert_eq!(&*decrypted_read, &plaintext[..], "Failed for size {}", size);
    }
}

#[tokio::test]
async fn test_reuse_key_multiple_times() {
    let key = generate_key(32).await.unwrap();
    let prng = PrngChacha20 {};

    for i in 0..100 {
        let plaintext = format!("Message number {}", i);
        let ciphertext = encrypt(&key, plaintext.as_bytes(), &prng).unwrap();
        let decrypted = decrypt(&key, &ciphertext).unwrap();
        let decrypted_read = decrypted.lock().unwrap();

        assert_eq!(&*decrypted_read, plaintext.as_bytes());
    }
}
