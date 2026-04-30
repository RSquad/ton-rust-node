/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
 
use crate::crypto::crypto_trait::Crypto;
use core::fmt;
use std::sync::Arc;

pub trait CryptoFactory: Send + Sync + fmt::Debug {
    fn new_crypto(&self) -> anyhow::Result<Arc<dyn Crypto>>;
}

#[cfg(feature = "crypto-default")]
#[derive(Debug)]
pub struct DefaultCryptoFactory {}

#[cfg(feature = "crypto-default")]
impl CryptoFactory for DefaultCryptoFactory {
    fn new_crypto(&self) -> anyhow::Result<Arc<dyn Crypto>> {
        use crate::crypto::{crypto_trait::CryptoImpl, prng_chacha20::PrngChacha20};

        let prng = Box::new(PrngChacha20 {});
        Ok(Arc::new(CryptoImpl::<crate::crypto::default_impl::DefaultEd25519>::new(prng)))
    }
}
