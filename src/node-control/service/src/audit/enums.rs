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
#[serde(tag = "event_type", content = "data", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AuditEventPayload {
    #[serde(rename = "elections.key_generated")]
    ElectionsKeyGenerated {
        election_id: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        pubkey: Option<String>,
    },

    #[serde(rename = "elections.stake_submitted")]
    ElectionsStakeSubmitted {
        election_id: u64,
        stake_nanotons: String,
        max_factor: u32,
        policy: String,
        submission_time: u64,
    },

    #[serde(rename = "elections.stake_accepted")]
    ElectionsStakeAccepted { election_id: u64, stake_nanotons: String },

    #[serde(rename = "elections.stake_skipped")]
    ElectionsStakeSkipped {
        election_id: u64,
        reason: StakeSkipReason,
        #[serde(skip_serializing_if = "Option::is_none")]
        required_nanotons: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        available_nanotons: Option<String>,
    },

    #[serde(rename = "elections.withdraw_processed")]
    ElectionsWithdrawProcessed { election_id: u64, tx_hash: String },

    #[serde(rename = "elections.stake_recovered")]
    ElectionsStakeRecovered {
        election_id: u64,
        amount_nanotons: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        tx_hash: Option<String>,
    },

    #[serde(rename = "rest_api.config_updated")]
    RestApiConfigUpdated {
        operation: String,
        changes: serde_json::Value, // diff stays free-form
    },

    #[serde(rename = "rest_api.auth_login_success")]
    RestApiAuthLoginSuccess { username: String },

    #[serde(rename = "rest_api.auth_login_rejected")]
    RestApiAuthLoginRejected { username: String, reason: String },

    #[serde(rename = "rest_api.token_rejected")]
    RestApiTokenRejected { reason: String },

    #[serde(rename = "system.service_started")]
    SystemServiceStarted { version: String },

    #[serde(rename = "system.service_stopped")]
    SystemServiceStopped,

    #[serde(rename = "system.audit_events_dropped")]
    SystemAuditEventsDropped { dropped_events: u64, reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum StakeSkipReason {
    LowWalletBalance,
    WithdrawRequestsPending,
    PoolNotReady,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditSource {
    Elections,
    Rewards,
    RestApi,
    Vault,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditSeverity {
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    Success,
    Failure,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditActorKind {
    Service,
    User,
    Scheduler,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditSubjectKind {
    Node,
    Elections,
    Config,
    Wallet,
    VaultKey,
    User,
    RewardRound,
    Recipient,
    System,
}
