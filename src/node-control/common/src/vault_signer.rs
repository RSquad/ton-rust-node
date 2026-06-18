/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::signer::Signer;
use secrets_vault::{errors::error::VaultError, types::secret::Secret};
use std::sync::Arc;

pub struct VaultSigner {
    secret: Arc<Secret>,
}

impl VaultSigner {
    pub async fn new(secret: Secret) -> anyhow::Result<Self> {
        if !matches!(secret, Secret::KeyPair { .. }) {
            anyhow::bail!("Unsupported secret type");
        }

        Ok(Self { secret: Arc::new(secret) })
    }
}

#[async_trait::async_trait]
impl Signer for VaultSigner {
    async fn public_key(&self) -> anyhow::Result<Vec<u8>> {
        let Secret::KeyPair { keypair } = self.secret.as_ref() else {
            anyhow::bail!("Unsupported secret type");
        };

        let pub_key = keypair
            .public_key()
            .ok_or_else(|| anyhow::anyhow!(VaultError::empty_public_key("Empty public key")))?
            .to_vec();

        Ok(pub_key)
    }

    async fn sign(&self, message: &[u8]) -> anyhow::Result<Vec<u8>> {
        let Secret::KeyPair { keypair } = self.secret.as_ref() else {
            anyhow::bail!("Unsupported secret type");
        };

        keypair.sign(message).await
    }
}
