/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
/// Hashes a plaintext password with Argon2id and a random salt.
pub fn hash_password(password: &str) -> anyhow::Result<String> {
    use argon2::PasswordHasher;
    let salt =
        argon2::password_hash::SaltString::generate(&mut argon2::password_hash::rand_core::OsRng);
    let hash = argon2::Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("password hashing failed: {e}"))?;
    Ok(hash.to_string())
}

/// Verifies a plaintext password against a stored Argon2id hash.
/// Returns `true` if the password is correct, `false` otherwise.
pub fn verify_password(password: &str, hash: &str) -> anyhow::Result<bool> {
    use argon2::PasswordVerifier;
    let hash = argon2::PasswordHash::new(hash)
        .map_err(|e| anyhow::anyhow!("invalid password hash: {e}"))?;
    Ok(argon2::Argon2::default().verify_password(password.as_bytes(), &hash).is_ok())
}
