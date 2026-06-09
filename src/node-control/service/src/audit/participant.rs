/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuditActor {
    Service {
        id: String,
    },
    User {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        role: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        ip: Option<String>,
    },
    System,
}

impl AuditActor {
    pub fn service(id: impl Into<String>) -> Self {
        Self::Service { id: id.into() }
    }

    pub fn user(id: impl Into<String>, role: Option<String>, ip: Option<String>) -> Self {
        Self::User { id: id.into(), role, ip }
    }

    pub fn system() -> Self {
        Self::System
    }
}

/// What the action was applied to. Internally tagged so per-variant required
/// fields are enforced by the type system; `#[non_exhaustive]` because new
/// target kinds are expected as more producers are wired.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AuditTarget {
    Node {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        election_id: Option<u64>,
    },
    Elections {
        election_id: u64,
    },
    Config {
        id: String,
    },
    Wallet {
        id: String,
    },
    VaultKey {
        id: String,
    },
    User {
        id: String,
    },
    RewardRound {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        election_id: Option<u64>,
    },
    Recipient {
        id: String,
    },
    System,
}
