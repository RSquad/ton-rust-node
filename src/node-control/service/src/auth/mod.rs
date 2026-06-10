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

/// Maps JWT verification failures to stable audit reason strings (no token material).
pub fn token_rejection_reason(err: &jsonwebtoken::errors::Error) -> &'static str {
    use jsonwebtoken::errors::ErrorKind;
    match err.kind() {
        ErrorKind::ExpiredSignature => "expired",
        ErrorKind::InvalidSignature => "signature_mismatch",
        ErrorKind::InvalidAlgorithm => "invalid_algorithm",
        ErrorKind::InvalidToken => "invalid_token",
        ErrorKind::InvalidIssuer => "invalid_issuer",
        ErrorKind::InvalidAudience => "invalid_audience",
        ErrorKind::ImmatureSignature => "immature_signature",
        ErrorKind::InvalidSubject => "invalid_subject",
        ErrorKind::MissingRequiredClaim(_) => "missing_required_claim",
        _ => "invalid_token",
    }
}

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
