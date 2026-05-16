/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::crypto::prng::Prng;
use rand_chacha::rand_core::{RngCore, SeedableRng};
use std::sync::LazyLock;

static SECURE_RNG: LazyLock<anyhow::Result<std::sync::Mutex<rand_chacha::ChaCha20Rng>>> =
    LazyLock::new(|| {
        rand_chacha::ChaCha20Rng::try_from_os_rng()
            .map(std::sync::Mutex::new)
            .map_err(|e| anyhow::anyhow!("Failed to initialize RNG: {}", e))
    });

pub struct PrngChacha20 {}

impl Prng for PrngChacha20 {
    fn fill_random(&self, dest: &mut [u8]) -> anyhow::Result<()> {
        let rng_mutex = SECURE_RNG.as_ref().or_else(|e| anyhow::bail!(e))?;

        {
            let mut rng =
                rng_mutex.lock().map_err(|e| anyhow::anyhow!("RNG lock poisoned: {e}"))?;
            rng.fill_bytes(dest);
        }

        Ok(())
    }
}
