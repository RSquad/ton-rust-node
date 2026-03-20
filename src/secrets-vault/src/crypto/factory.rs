/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */

#[cfg(feature = "crypto-block")]
use crate::crypto::block_impl::BlockEd25519;
#[cfg(all(feature = "crypto-default", not(feature = "crypto-block")))]
use crate::crypto::default_impl::DefaultEd25519;
use crate::crypto::{
    crypto_trait::{Crypto, CryptoImpl},
    prng_chacha20::PrngChacha20,
};

#[cfg(not(any(feature = "crypto-block", feature = "crypto-default")))]
compile_error!("Either \"crypto-block\" or \"crypto-default\" feature must be enabled");

pub trait CryptoFactory: Send + Sync {
    fn new_crypto(&self) -> anyhow::Result<Box<dyn Crypto>>;
}

pub struct AutoCryptoFactory {}

impl CryptoFactory for AutoCryptoFactory {
    fn new_crypto(&self) -> anyhow::Result<Box<dyn Crypto>> {
        let prng = Box::new(PrngChacha20 {});

        #[cfg(feature = "crypto-block")]
        return Ok(Box::new(CryptoImpl::<BlockEd25519>::new(prng)));

        #[cfg(all(feature = "crypto-default", not(feature = "crypto-block")))]
        return Ok(Box::new(CryptoImpl::<DefaultEd25519>::new(prng)));
    }
}

#[cfg(feature = "crypto-default")]
pub struct DefaultCryptoFactory {}

#[cfg(feature = "crypto-default")]
impl CryptoFactory for DefaultCryptoFactory {
    fn new_crypto(&self) -> anyhow::Result<Box<dyn Crypto>> {
        let prng = Box::new(PrngChacha20 {});
        Ok(Box::new(CryptoImpl::<crate::crypto::default_impl::DefaultEd25519>::new(prng)))
    }
}

#[cfg(feature = "crypto-block")]
pub struct BlockCryptoFactory {}

#[cfg(feature = "crypto-block")]
impl CryptoFactory for BlockCryptoFactory {
    fn new_crypto(&self) -> anyhow::Result<Box<dyn Crypto>> {
        let prng = Box::new(PrngChacha20 {});
        Ok(Box::new(CryptoImpl::<BlockEd25519>::new(prng)))
    }
}
