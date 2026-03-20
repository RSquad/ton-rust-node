/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
pub mod jwt;
pub mod middleware;
pub mod user_store;

pub use common::app_config::Role;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Claims {
    /// Subject: authenticated username.
    pub sub: String,
    /// User role captured at token issuance time.
    pub role: Role,
    /// Issued-at timestamp (unix seconds).
    pub iat: u64,
    /// Expiration timestamp (unix seconds).
    pub exp: u64,
}
