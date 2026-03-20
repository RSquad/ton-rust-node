/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::types::algorithm::Algorithm;
use std::collections::HashMap;

#[derive(Clone)]
pub struct SecretSpec {
    pub algorithm: Algorithm,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub tags: HashMap<String, String>,
    pub extractable: bool,
    pub size: Option<usize>,
}

impl SecretSpec {
    pub fn new(algorithm: Algorithm) -> Self {
        Self { algorithm, expires_at: None, tags: HashMap::new(), extractable: false, size: None }
    }

    pub fn with_expiration(mut self, expires_at: chrono::DateTime<chrono::Utc>) -> Self {
        self.expires_at = Some(expires_at);
        self
    }

    pub fn with_tag(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.tags.insert(key.into(), value.into());
        self
    }

    pub fn with_tags(mut self, tags: HashMap<String, String>) -> Self {
        self.tags.extend(tags);
        self
    }

    pub fn extractable(mut self, extractable: bool) -> Self {
        self.extractable = extractable;
        self
    }

    pub fn size(mut self, size: usize) -> Self {
        self.size = Some(size);
        self
    }
}
