/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "event_type", content = "data", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AuditEventPayload {
    #[serde(rename = "elections.key_generated")]
    ElectionsKeyGenerated {
        #[serde(skip_serializing_if = "Option::is_none")]
        pubkey: Option<String>,
    },

    #[serde(rename = "elections.stake_submitted")]
    ElectionsStakeSubmitted { stake: String, max_factor: u32, policy: String, submission_time: u64 },

    #[serde(rename = "elections.stake_accepted")]
    ElectionsStakeAccepted { stake: String },

    #[serde(rename = "elections.stake_skipped")]
    ElectionsStakeSkipped {
        reason: StakeSkipReason,
        #[serde(skip_serializing_if = "Option::is_none")]
        required: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        available: Option<String>,
    },

    #[serde(rename = "elections.stake_failed")]
    ElectionsStakeFailed { reason: String },

    #[serde(rename = "elections.stake_recovered")]
    ElectionsStakeRecovered {
        amount: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        msg_hash: Option<String>,
    },

    #[serde(rename = "elections.stake_recover_failed")]
    ElectionsStakeRecoverFailed { reason: String },

    #[serde(rename = "elections.withdraw_processed")]
    ElectionsWithdrawProcessed { msg_hash: String },

    #[serde(rename = "elections.withdraw_failed")]
    ElectionsWithdrawFailed { reason: String },

    // ── rewards (reserved; producers not wired yet) ─────────────────────────
    #[serde(rename = "rewards.distribution_started")]
    RewardsDistributionStarted { recipients_count: u32 },

    #[serde(rename = "rewards.distribution_completed")]
    RewardsDistributionCompleted { recipients_count: u32, total: String },

    #[serde(rename = "rewards.distribution_failed")]
    RewardsDistributionFailed { reason: String },

    #[serde(rename = "rewards.recipient_skipped")]
    RewardsRecipientSkipped { reason: String },

    // ── rest_api ────────────────────────────────────────────────────────────
    #[serde(rename = "rest_api.config_updated")]
    RestApiConfigUpdated { operation: String, changes: Vec<ConfigFieldChange> },

    #[serde(rename = "rest_api.auth_login_succeeded")]
    RestApiAuthLoginSucceeded {},

    #[serde(rename = "rest_api.auth_login_rejected")]
    RestApiAuthLoginRejected { reason: String },

    #[serde(rename = "rest_api.token_rejected")]
    RestApiTokenRejected { reason: String },

    // ── vault (reserved; producers not wired yet) ───────────────────────────
    #[serde(rename = "vault.key_created")]
    VaultKeyCreated {},

    #[serde(rename = "vault.key_removed")]
    VaultKeyRemoved {},

    // ── system ───────────────────────────────────────────────────────────────
    #[serde(rename = "system.service_started")]
    SystemServiceStarted { version: String },

    #[serde(rename = "system.service_stopped")]
    SystemServiceStopped {},

    #[serde(rename = "system.audit_events_dropped")]
    SystemAuditEventsDropped { dropped_events: u64, reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum StakeSkipReason {
    LowWalletBalance,
    WithdrawRequestsPending,
    PoolNotReady,
    AdaptiveSleepingPeriod,
    AdaptiveWaitingPeriod,
    ElectionsDisabled,
    RecoverPending,
    InsufficientStakeFunds,
}

/// A single typed field change for `rest_api.config_updated`. Replaces the
/// previous free-form `serde_json::Value` diff.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ConfigFieldChange {
    /// Dotted path, e.g. `elections.sleep_period_pct`.
    pub field: String,
    pub old: serde_json::Value,
    pub new: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    Success,
    Failure,
    Skipped,
}

/// Log-level-like severity, derived from the event type at the display layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditSeverity {
    Info,
    Warn,
    Error,
}

/// Originating subsystem, derived from the `event_type` prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditSource {
    Elections,
    Rewards,
    RestApi,
    Vault,
    System,
}

impl AuditEventPayload {
    pub fn severity(&self) -> AuditSeverity {
        use AuditEventPayload::*;
        use AuditSeverity::*;
        match self {
            ElectionsKeyGenerated { .. }
            | ElectionsStakeSubmitted { .. }
            | ElectionsStakeAccepted { .. }
            | ElectionsStakeRecovered { .. }
            | ElectionsWithdrawProcessed { .. }
            | RewardsDistributionStarted { .. }
            | RewardsDistributionCompleted { .. }
            | RestApiConfigUpdated { .. }
            | RestApiAuthLoginSucceeded {}
            | VaultKeyCreated {}
            | VaultKeyRemoved {}
            | SystemServiceStarted { .. }
            | SystemServiceStopped {} => Info,

            ElectionsStakeSkipped { .. }
            | RewardsRecipientSkipped { .. }
            | RestApiAuthLoginRejected { .. }
            | RestApiTokenRejected { .. }
            | SystemAuditEventsDropped { .. } => Warn,

            ElectionsStakeFailed { .. }
            | ElectionsStakeRecoverFailed { .. }
            | ElectionsWithdrawFailed { .. }
            | RewardsDistributionFailed { .. } => Error,
        }
    }

    pub fn source(&self) -> AuditSource {
        use AuditEventPayload::*;
        use AuditSource::*;
        match self {
            ElectionsKeyGenerated { .. }
            | ElectionsStakeSubmitted { .. }
            | ElectionsStakeAccepted { .. }
            | ElectionsStakeSkipped { .. }
            | ElectionsStakeFailed { .. }
            | ElectionsStakeRecovered { .. }
            | ElectionsStakeRecoverFailed { .. }
            | ElectionsWithdrawProcessed { .. }
            | ElectionsWithdrawFailed { .. } => Elections,

            RewardsDistributionStarted { .. }
            | RewardsDistributionCompleted { .. }
            | RewardsDistributionFailed { .. }
            | RewardsRecipientSkipped { .. } => Rewards,

            RestApiConfigUpdated { .. }
            | RestApiAuthLoginSucceeded {}
            | RestApiAuthLoginRejected { .. }
            | RestApiTokenRejected { .. } => RestApi,

            VaultKeyCreated {} | VaultKeyRemoved {} => Vault,

            SystemServiceStarted { .. }
            | SystemServiceStopped {}
            | SystemAuditEventsDropped { .. } => System,
        }
    }
}
