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

    /// Election participation status for all controlled nodes.
    pub our_participants: Vec<OurElectionParticipant>,

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

    /// Minimum stake required by elections config/params (nanotons, decimal string).
    pub min_stake: String,

    /// Minimum stake among current participants (nanotons, decimal string).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub participant_min_stake: Option<String>,

    /// Maximum stake among current participants (nanotons, decimal string).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub participant_max_stake: Option<String>,

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
    /// Validator public key (base64).
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

/// Single stake submission record.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct StakeSubmission {
    /// Stake sent to elector (nanotons, decimal string).
    pub stake: String,

    /// Max factor used for this submission.
    pub max_factor: f32,

    /// Time when stake was submitted (unix seconds).
    pub submission_time: u64,

    /// Time when stake was submitted (UTC string).
    pub submission_time_utc: String,
}

/// Participation status enum for election flow.
/// Flow: Idle → Participating → Submitted → Accepted → Elected → Validating.
/// `ProcessingWithdrawRequests` is a TONCore-only intermediate state set whenever the runner's
/// last on-chain probe reports a non-empty `withdraw_requests` queue and stake has not yet been
/// submitted — the runner sends `process_withdraw_requests` (op = 2) on each such tick and waits
/// one tick for the pool to drain before submitting a new stake.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ParticipationStatus {
    /// Node is not participating in elections.
    #[default]
    Idle,
    /// Node has generated election key, preparing to submit stake.
    Participating,
    /// Stake has been submitted to the elector.
    Submitted,
    /// Stake was accepted by the elector.
    Accepted,
    /// Node is elected in next validator set (p36) but not yet validating.
    Elected,
    /// Node is actively validating (in current validator set p34).
    Validating,
    /// TONCore: the pool's `withdraw_requests` queue is still non-empty per the latest on-chain
    /// probe and stake has not been submitted yet; the runner sends `process_withdraw_requests`
    /// (op = 2) and re-probes each tick until the queue drains.
    #[serde(rename = "processing_withdraw_requests", alias = "processing_withdraws")]
    ProcessingWithdrawRequests,
}

impl std::fmt::Display for ParticipationStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParticipationStatus::Idle => write!(f, "idle"),
            ParticipationStatus::Participating => write!(f, "participating"),
            ParticipationStatus::Submitted => write!(f, "submitted"),
            ParticipationStatus::Accepted => write!(f, "accepted"),
            ParticipationStatus::Elected => write!(f, "elected"),
            ParticipationStatus::Validating => write!(f, "validating"),
            ParticipationStatus::ProcessingWithdrawRequests => {
                write!(f, "processing_withdraw_requests")
            }
        }
    }
}

/// Election participation status for a controlled node.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct OurElectionParticipant {
    /// Node id from config.
    pub node_id: String,

    /// Current participation status.
    pub status: ParticipationStatus,

    /// Validator public key (base64).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pubkey: Option<String>,

    /// Key id (base64).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_id: Option<String>,

    /// ADNL address (base64).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adnl: Option<String>,

    /// History of stake submissions for this election cycle.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stake_submissions: Vec<StakeSubmission>,

    /// Stake accepted by elector (nanotons, decimal string).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accepted_stake: Option<String>,

    /// Whether the stake was accepted by the elector.
    pub stake_accepted: bool,

    /// Whether the node was elected.
    pub elected: bool,

    /// Position in the ranked participant list (1-based, by stake descending).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<u32>,

    /// Wallet address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wallet_addr: Option<String>,

    /// Pool address (if any).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pool_addr: Option<String>,

    /// Last error (if any).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// Validators snapshot.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct ValidatorsSnapshot {
    /// Only nodectl-controlled nodes from config
    pub controlled_nodes: Vec<ValidatorNodeSnapshot>,
    pub default_stake_policy: StakePolicy,
    /// Current validation time range (from p34 validator set).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validation_range: Option<TimeRange>,
}

/// Per-node validator status.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct ValidatorNodeSnapshot {
    /// Node id from config.
    pub node_id: String,

    /// In current validator set (if known).
    pub is_validator: bool,

    /// Index in validator set (0-based).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validator_index: Option<u16>,

    /// Validator weight in the set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weight: Option<u64>,

    /// Wallet address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wallet_addr: Option<String>,

    /// Pool address (if any).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pool_addr: Option<String>,

    /// Public key (base64).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pubkey: Option<String>,

    /// ADNL address (base64).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adnl: Option<String>,

    /// Key id (base64).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_id: Option<String>,

    /// Election ID for which the key was created.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_election_id: Option<u64>,

    /// Key expiration timestamp (unix seconds).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_expires_at: Option<u64>,

    /// Key expiration timestamp in UTC.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_expires_at_utc: Option<String>,

    /// Whether the key is still active (not expired).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_key_active: Option<bool>,

    /// Stake submitted to elections (nanotons as decimal string).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stake: Option<String>,

    /// Stake accepted by the elector.
    pub stake_accepted: bool,

    /// Last error (if any).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,

    /// Binding lifecycle status.
    #[serde(default)]
    pub binding_status: BindingStatus,
}

/// View for `/v1/elections` endpoint.
pub struct ElectionsView {
    pub status: ElectionsStatus,
    pub elections: Option<ElectionsSnapshot>,
    pub next_elections: Option<TimeRange>,
    pub our_participants: Vec<OurElectionParticipant>,
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

    /// Read a view optimized for `/v1/elections`.
    /// When `include_participants` is false, participants are not cloned (empty list returned).
    pub fn get_elections_view(&self, include_participants: bool) -> ElectionsView {
        let guard = self.inner.read().expect("SnapshotStore poisoned (read)");
        let elections = guard.elections.as_ref().map(|src| {
            if include_participants {
                src.clone()
            } else {
                // Clone only metadata, skip participants to avoid large allocation
                ElectionsSnapshot {
                    election_id: src.election_id,
                    elect_close: src.elect_close,
                    elect_close_utc: src.elect_close_utc.clone(),
                    finished: src.finished,
                    failed: src.failed,
                    participants_count: src.participants_count,
                    participants: Vec::new(),
                    min_stake: src.min_stake.clone(),
                    participant_min_stake: src.participant_min_stake.clone(),
                    participant_max_stake: src.participant_max_stake.clone(),
                    total_stake: src.total_stake.clone(),
                    next_validation_range: src.next_validation_range.clone(),
                    elections_range: src.elections_range.clone(),
                }
            }
        });
        ElectionsView {
            status: guard.elections_status.clone(),
            elections,
            next_elections: guard.next_elections_range.clone(),
            our_participants: guard.our_participants.clone(),
        }
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
