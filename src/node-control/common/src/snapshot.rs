/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    app_config::{BindingStatus, StakePolicy},
    time_format,
};
use std::sync::RwLock;

/// Snapshot for HTTP API (no secrets).
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct Snapshot {
    /// Unix timestamp (seconds).
    pub generated_at: u64,

    /// Elections status
    pub elections_status: ElectionsStatus,
    /// Elections snapshot (optional).
    pub elections: Option<ElectionsSnapshot>,
    /// Next elections range.
    pub next_elections_range: Option<TimeRange>,

    /// Validators snapshot.
    pub validators: ValidatorsSnapshot,
}

impl Snapshot {
    pub fn empty() -> Self {
        Self { generated_at: time_format::now(), ..Default::default() }
    }
}

#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ElectionsStatus {
    #[default]
    Closed,
    Finished,
    Failed,
    Postponed,
    Active,
}

/// Elections snapshot.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct ElectionsSnapshot {
    /// Active election id.
    pub election_id: u64,

    /// Close timestamp from the elector contract.
    pub elect_close: u64,
    /// Close timestamp from the elector contract in UTC string.
    pub elect_close_utc: String,

    /// Elections finished flag.
    pub finished: bool,

    /// Elections failed flag.
    pub failed: bool,

    /// Participants count.
    pub participants_count: u32,

    /// Participants list
    pub participants: Vec<ElectionsParticipantSnapshot>,

    /// Minimum stake (nanotons, decimal string).
    pub min_stake: String,

    /// Total stake (nanotons, decimal string).
    pub total_stake: String,

    /// Validation time range.
    pub next_validation_range: TimeRange,

    /// Elections time range.
    pub elections_range: TimeRange,
}

#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct TimeRange {
    /// Range start (unix seconds)
    pub start: u64,
    /// Range start in UTC
    pub start_utc: String,
    /// Range end (unix seconds)
    pub end: u64,
    /// Range end in UTC
    pub end_utc: String,
}

#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct ElectionsParticipantSnapshot {
    /// Validator public key (hex).
    pub pubkey: String,

    /// ADNL address (base64).
    pub adnl: String,

    /// Sender address
    pub sender_addr: String,

    /// True if participant is one of nodectl controlled nodes.
    pub is_controlled: bool,

    /// Stake (nanotons, decimal string).
    pub stake: String,

    /// Max factor
    pub max_factor: f32,

    /// Election id.
    pub election_id: u64,
}

/// Validators snapshot.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct ValidatorsSnapshot {
    /// Only nodectl-controlled nodes from config
    pub controlled_nodes: Vec<ValidatorNodeSnapshot>,
    pub default_stake_policy: StakePolicy,
}

/// Per-node status.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct ValidatorNodeSnapshot {
    /// Node id from config.
    pub node_id: String,

    /// In current validator set (if known).
    pub is_validator: bool,

    /// Index in validator set (if known).
    pub validator_index: Option<u16>,

    /// Wallet address (if known).
    pub wallet_addr: Option<String>,

    /// Pool address (if any).
    pub pool_addr: Option<String>,

    /// Max factor.
    pub max_factor: Option<f32>,

    /// Public key (hex).
    pub pubkey: Option<String>,

    /// ADNL (base64).
    pub adnl: Option<String>,

    /// Stake sent to elector (nanotons, decimal string).
    pub stake: Option<String>,

    /// Key id (base64).
    pub key_id: Option<String>,

    /// Stake accepted.
    pub stake_accepted: bool,

    /// Last error (if any).
    pub last_error: Option<String>,

    /// Effective stake policy for this node (override or default).
    pub stake_policy: StakePolicy,

    /// Binding lifecycle status.
    #[serde(default)]
    pub binding_status: BindingStatus,
}

/// In-memory snapshot store.
pub struct SnapshotStore {
    inner: RwLock<Snapshot>,
}

impl SnapshotStore {
    pub fn new() -> Self {
        Self { inner: RwLock::new(Snapshot::empty()) }
    }

    pub fn get(&self) -> Snapshot {
        self.inner.read().expect("SnapshotStore poisoned (read)").clone()
    }

    /// Update snapshot in-place and auto-update `generated_at`.
    pub fn update_with<F>(&self, f: F)
    where
        F: FnOnce(&mut Snapshot),
    {
        let mut guard = self.inner.write().expect("SnapshotStore poisoned (write)");
        f(&mut guard);
        guard.generated_at = time_format::now();
    }
}
