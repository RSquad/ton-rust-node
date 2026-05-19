/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */

use crate::{
    memory::protected_memory::ProtectedMemory,
    types::{metadata::Metadata, secret::Blob, secret_id::SecretId},
};

pub struct BlobInMemory {
    metadata: Metadata,
    data: ProtectedMemory,
}

impl BlobInMemory {
    pub fn new(metadata: &Metadata, data: ProtectedMemory) -> Self {
        Self { metadata: metadata.clone(), data }
    }
}

impl Blob for BlobInMemory {
    fn id(&self) -> Option<&SecretId> {
        self.metadata.secret_id.as_ref()
    }

    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    fn metadata_mut(&mut self) -> &mut Metadata {
        &mut self.metadata
    }

    fn data(&self) -> &ProtectedMemory {
        &self.data
    }
}
