/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::audit::{
    enums::{AuditEventPayload, AuditOutcome, StakeSkipReason},
    participant::{AuditActor, AuditTarget},
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

/// Renders timestamps as RFC3339 with millisecond precision and a trailing `Z`
/// (e.g. `2026-05-22T12:10:30.123Z`), used for `ts` and `started_at`.
mod ts_millis_rfc3339 {
    use super::*;

    pub fn serialize<S: Serializer>(ts: &DateTime<Utc>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&ts.to_rfc3339_opts(SecondsFormat::Millis, true))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<DateTime<Utc>, D::Error> {
        let raw = String::deserialize(d)?;
        DateTime::parse_from_rfc3339(&raw)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(serde::de::Error::custom)
    }
}

/// First JSONL line of every (rotated) audit file. Readers distinguish it from
/// events by the absence of an `event_type` field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditFileHeader {
    pub schema_version: u16,
    /// Logical service name, e.g. `"nodectl"`.
    pub service: String,
    /// Service semver.
    pub service_version: String,
    pub host: String,
    #[serde(with = "ts_millis_rfc3339")]
    pub started_at: DateTime<Utc>,
}

/// A single audit record.
///
/// Wire shape: `id`, `ts`, `outcome`, the flattened payload
/// (`event_type` + `data`), `actor`, `target`. `severity`/`source` are derived
/// from the payload at the display layer and `schema_version` lives in
/// [`AuditFileHeader`], so none of them are stored per event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditEvent {
    /// UUID v7 — sortable by creation time.
    pub id: Uuid,
    #[serde(with = "ts_millis_rfc3339")]
    pub ts: DateTime<Utc>,
    pub outcome: AuditOutcome,
    #[serde(flatten)]
    pub payload: AuditEventPayload,
    pub actor: AuditActor,
    pub target: AuditTarget,
}

/// Named payload fields for [`AuditEvent::elections_stake_submitted`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElectionsStakeSubmittedParams {
    pub stake: String,
    pub max_factor: u32,
    pub policy: String,
    pub submission_time: u64,
}

impl AuditEvent {
    /// Returns a stable deduplication key for events that should appear at most once
    /// per election in the audit log (e.g. `elections.stake_skipped` with a persistent
    /// reason like `ElectionsDisabled`).
    ///
    /// Returns `None` for events that are always recorded without deduplication.
    /// The key encodes `(event_category, node_id, election_id, reason)` so that
    /// the same skip reason for the same node in the same election is written only once.
    pub fn dedup_key(&self) -> Option<String> {
        if let AuditEventPayload::ElectionsStakeSkipped { reason, .. } = &self.payload
            && let AuditTarget::Node { id: node_id, election_id: Some(election_id) } = &self.target
        {
            return Some(format!("stake_skipped:{node_id}:{election_id}:{reason:?}"));
        }
        None
    }

    /// Internal constructor that stamps `id`/`ts`. Crate-private so call sites
    /// must go through the typed constructors below, which bake the canonical
    /// outcome per event type.
    pub(crate) fn new(
        actor: AuditActor,
        target: AuditTarget,
        outcome: AuditOutcome,
        payload: AuditEventPayload,
    ) -> Self {
        Self { id: Uuid::now_v7(), ts: Utc::now(), outcome, payload, actor, target }
    }

    /// `target` for a per-node election event: always `Node { election_id }`.
    fn node_target(node_id: impl Into<String>, election_id: u64) -> AuditTarget {
        AuditTarget::Node { id: node_id.into(), election_id: Some(election_id) }
    }

    pub fn elections_key_generated(
        actor: AuditActor,
        node_id: impl Into<String>,
        election_id: u64,
        pubkey: Option<String>,
    ) -> Self {
        Self::new(
            actor,
            Self::node_target(node_id, election_id),
            AuditOutcome::Success,
            AuditEventPayload::ElectionsKeyGenerated { pubkey },
        )
    }

    pub fn elections_stake_submitted(
        actor: AuditActor,
        node_id: impl Into<String>,
        election_id: u64,
        params: ElectionsStakeSubmittedParams,
    ) -> Self {
        Self::new(
            actor,
            Self::node_target(node_id, election_id),
            AuditOutcome::Success,
            AuditEventPayload::ElectionsStakeSubmitted {
                stake: params.stake,
                max_factor: params.max_factor,
                policy: params.policy,
                submission_time: params.submission_time,
            },
        )
    }

    pub fn elections_stake_skipped(
        actor: AuditActor,
        node_id: impl Into<String>,
        election_id: u64,
        reason: StakeSkipReason,
        required: Option<String>,
        available: Option<String>,
    ) -> Self {
        Self::new(
            actor,
            Self::node_target(node_id, election_id),
            AuditOutcome::Skipped,
            AuditEventPayload::ElectionsStakeSkipped { reason, required, available },
        )
    }

    pub fn elections_stake_failed(
        actor: AuditActor,
        node_id: impl Into<String>,
        election_id: u64,
        reason: impl Into<String>,
    ) -> Self {
        Self::new(
            actor,
            Self::node_target(node_id, election_id),
            AuditOutcome::Failure,
            AuditEventPayload::ElectionsStakeFailed { reason: reason.into() },
        )
    }

    pub fn elections_stake_recovered(
        actor: AuditActor,
        node_id: impl Into<String>,
        election_id: u64,
        amount: impl Into<String>,
        msg_hash: Option<String>,
    ) -> Self {
        Self::new(
            actor,
            Self::node_target(node_id, election_id),
            AuditOutcome::Success,
            AuditEventPayload::ElectionsStakeRecovered { amount: amount.into(), msg_hash },
        )
    }

    pub fn elections_stake_recover_failed(
        actor: AuditActor,
        node_id: impl Into<String>,
        election_id: u64,
        reason: impl Into<String>,
    ) -> Self {
        Self::new(
            actor,
            Self::node_target(node_id, election_id),
            AuditOutcome::Failure,
            AuditEventPayload::ElectionsStakeRecoverFailed { reason: reason.into() },
        )
    }

    pub fn elections_withdraw_processed(
        actor: AuditActor,
        node_id: impl Into<String>,
        election_id: u64,
        msg_hash: impl Into<String>,
    ) -> Self {
        Self::new(
            actor,
            Self::node_target(node_id, election_id),
            AuditOutcome::Success,
            AuditEventPayload::ElectionsWithdrawProcessed { msg_hash: msg_hash.into() },
        )
    }

    pub fn elections_withdraw_failed(
        actor: AuditActor,
        node_id: impl Into<String>,
        election_id: u64,
        reason: impl Into<String>,
    ) -> Self {
        Self::new(
            actor,
            Self::node_target(node_id, election_id),
            AuditOutcome::Failure,
            AuditEventPayload::ElectionsWithdrawFailed { reason: reason.into() },
        )
    }

    pub fn system_service_started(version: impl Into<String>) -> Self {
        Self::new(
            AuditActor::System,
            AuditTarget::System,
            AuditOutcome::Success,
            AuditEventPayload::SystemServiceStarted { version: version.into() },
        )
    }

    pub fn system_audit_events_dropped(dropped: u64) -> Self {
        Self::new(
            AuditActor::System,
            AuditTarget::System,
            AuditOutcome::Failure,
            AuditEventPayload::SystemAuditEventsDropped {
                dropped_events: dropped,
                reason: "queue_full_after_timeout".into(),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{AuditLogConfig, enums::ConfigFieldChange};
    use serde_json::{Value, json};
    use std::path::PathBuf;

    const FIXTURE_ID: &str = "9b6c2b5a-9f9d-4a9f-bc31-9a89b0e9d111";
    const FIXTURE_TS: &str = "2026-05-22T12:10:30.123Z";

    fn fixture_id() -> Uuid {
        FIXTURE_ID.parse().unwrap()
    }

    fn fixture_ts() -> DateTime<Utc> {
        FIXTURE_TS.parse().unwrap()
    }

    fn assert_json_eq(actual: &AuditEvent, expected: Value) {
        let actual_value = serde_json::to_value(actual).expect("serialize event");
        assert_eq!(actual_value, expected);
    }

    fn fixed(
        outcome: AuditOutcome,
        actor: AuditActor,
        target: AuditTarget,
        payload: AuditEventPayload,
    ) -> AuditEvent {
        AuditEvent { id: fixture_id(), ts: fixture_ts(), outcome, payload, actor, target }
    }

    #[test]
    fn serializes_stake_submitted_to_expected_json() {
        let event = fixed(
            AuditOutcome::Success,
            AuditActor::service("elections-task"),
            AuditTarget::Node { id: "node1".into(), election_id: Some(1_779_265_552) },
            AuditEventPayload::ElectionsStakeSubmitted {
                stake: "50000000000000".into(),
                max_factor: 196_608,
                policy: "adaptive_split50".into(),
                submission_time: 1_779_265_400,
            },
        );

        assert_json_eq(
            &event,
            json!({
                "id": FIXTURE_ID,
                "ts": FIXTURE_TS,
                "outcome": "success",
                "event_type": "elections.stake_submitted",
                "data": {
                    "stake": "50000000000000",
                    "max_factor": 196608,
                    "policy": "adaptive_split50",
                    "submission_time": 1779265400
                },
                "actor": { "kind": "service", "id": "elections-task" },
                "target": { "kind": "node", "id": "node1", "election_id": 1779265552 }
            }),
        );
    }

    #[test]
    fn serializes_stake_skipped_to_expected_json() {
        let event = fixed(
            AuditOutcome::Skipped,
            AuditActor::service("elections-task"),
            AuditTarget::Node { id: "node6".into(), election_id: Some(1_779_265_552) },
            AuditEventPayload::ElectionsStakeSkipped {
                reason: StakeSkipReason::LowWalletBalance,
                required: Some("1200000000".into()),
                available: Some("900000000".into()),
            },
        );

        assert_json_eq(
            &event,
            json!({
                "id": FIXTURE_ID,
                "ts": FIXTURE_TS,
                "outcome": "skipped",
                "event_type": "elections.stake_skipped",
                "data": {
                    "reason": "low_wallet_balance",
                    "required": "1200000000",
                    "available": "900000000"
                },
                "actor": { "kind": "service", "id": "elections-task" },
                "target": { "kind": "node", "id": "node6", "election_id": 1779265552 }
            }),
        );
    }

    #[test]
    fn file_header_serializes_with_millis_ts() {
        let header = AuditFileHeader {
            schema_version: 1,
            service: "nodectl".into(),
            service_version: "0.5.1".into(),
            host: "node-host".into(),
            started_at: fixture_ts(),
        };
        let value = serde_json::to_value(&header).expect("serialize header");
        assert_eq!(
            value,
            json!({
                "schema_version": 1,
                "service": "nodectl",
                "service_version": "0.5.1",
                "host": "node-host",
                "started_at": FIXTURE_TS
            })
        );
        // Header has no event_type — that is how readers tell it apart from events.
        assert!(value.get("event_type").is_none());
    }

    fn sample_event(payload: AuditEventPayload) -> AuditEvent {
        fixed(
            AuditOutcome::Success,
            AuditActor::System,
            AuditTarget::Node { id: "node1".into(), election_id: Some(1_779_265_552) },
            payload,
        )
    }

    fn all_payload_variants() -> Vec<AuditEventPayload> {
        vec![
            AuditEventPayload::ElectionsKeyGenerated { pubkey: Some("aabb".into()) },
            AuditEventPayload::ElectionsStakeSubmitted {
                stake: "1".into(),
                max_factor: 1,
                policy: "all".into(),
                submission_time: 1,
            },
            AuditEventPayload::ElectionsStakeAccepted { stake: "50000000000000".into() },
            AuditEventPayload::ElectionsStakeSkipped {
                reason: StakeSkipReason::WithdrawRequestsPending,
                required: None,
                available: None,
            },
            AuditEventPayload::ElectionsStakeFailed { reason: "send failed".into() },
            AuditEventPayload::ElectionsWithdrawProcessed { msg_hash: "abc".into() },
            AuditEventPayload::ElectionsWithdrawFailed { reason: "send failed".into() },
            AuditEventPayload::ElectionsStakeRecovered {
                amount: "50000000000000".into(),
                msg_hash: Some("def".into()),
            },
            AuditEventPayload::ElectionsStakeRecoverFailed { reason: "send failed".into() },
            AuditEventPayload::RewardsDistributionStarted { recipients_count: 3 },
            AuditEventPayload::RewardsDistributionCompleted {
                recipients_count: 3,
                total: "9".into(),
            },
            AuditEventPayload::RewardsDistributionFailed { reason: "rpc".into() },
            AuditEventPayload::RewardsRecipientSkipped { reason: "below_min".into() },
            AuditEventPayload::RestApiConfigUpdated {
                operation: "patch".into(),
                changes: vec![ConfigFieldChange {
                    field: "elections.x".into(),
                    old: json!(1),
                    new: json!(2),
                }],
            },
            AuditEventPayload::RestApiAuthLoginSucceeded {},
            AuditEventPayload::RestApiAuthLoginRejected { reason: "bad password".into() },
            AuditEventPayload::RestApiTokenRejected { reason: "expired".into() },
            AuditEventPayload::VaultKeyCreated {},
            AuditEventPayload::VaultKeyRemoved {},
            AuditEventPayload::SystemServiceStarted { version: "0.5.0".into() },
            AuditEventPayload::SystemServiceStopped {},
            AuditEventPayload::SystemAuditEventsDropped {
                dropped_events: 3,
                reason: "queue_full_after_timeout".into(),
            },
        ]
    }

    #[test]
    fn round_trip_all_variants() {
        for payload in all_payload_variants() {
            let event = sample_event(payload);
            let json = serde_json::to_string(&event).expect("serialize");
            let restored: AuditEvent = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(event, restored, "json: {json}");
        }
    }

    #[test]
    fn canonical_outcome_is_baked_into_constructors() {
        let skipped = AuditEvent::elections_stake_skipped(
            AuditActor::service("elections-task"),
            "node1",
            1,
            StakeSkipReason::PoolNotReady,
            None,
            None,
        );
        assert_eq!(skipped.outcome, AuditOutcome::Skipped);

        let failed = AuditEvent::elections_stake_failed(
            AuditActor::service("elections-task"),
            "node1",
            1,
            "boom",
        );
        assert_eq!(failed.outcome, AuditOutcome::Failure);
    }

    #[test]
    fn default_config_matches_spec_defaults() {
        let cfg = AuditLogConfig::default();
        assert_eq!(cfg.path, PathBuf::from("./logs/audit.jsonl"));
        assert!(cfg.enabled);
        assert_eq!(cfg.max_size_bytes, 100 * 1024 * 1024);
        assert_eq!(cfg.max_files, 10);
        assert_eq!(cfg.batch_interval_ms, 1000);
        assert_eq!(cfg.batch_max_events, 100);
        assert_eq!(cfg.queue_capacity, 10_000);
        assert_eq!(cfg.queue_full_timeout_ms, 250);
        assert!(!cfg.fsync_on_batch);
        assert!(cfg.include_payload);
        assert!(!cfg.record_client_ip);
        assert!(!cfg.ip_anonymize);
        assert_eq!(cfg.ring_buffer_capacity, 100);
    }
}
