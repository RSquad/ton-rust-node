/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::audit::{
    enums::{AuditEventPayload, AuditOutcome, AuditSeverity, AuditSource},
    participant::{AuditActor, AuditSubject},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditEvent {
    pub schema_version: u16,
    pub id: Uuid,
    pub ts: DateTime<Utc>,
    pub source: AuditSource,
    pub severity: AuditSeverity,
    pub outcome: AuditOutcome,
    pub actor: AuditActor,
    pub subject: AuditSubject,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(flatten)]
    pub payload: AuditEventPayload,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{
        AuditLogConfig,
        enums::{AuditActorKind, AuditEventPayload, AuditSubjectKind, StakeSkipReason},
    };
    use serde_json::{Value, json};
    use std::{collections::BTreeMap, path::PathBuf};

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

    #[test]
    fn serializes_stake_submitted_to_expected_json() {
        let event = AuditEvent {
            schema_version: 1,
            id: fixture_id(),
            ts: fixture_ts(),
            source: AuditSource::Elections,
            severity: AuditSeverity::Info,
            outcome: AuditOutcome::Success,
            actor: AuditActor {
                kind: AuditActorKind::Service,
                id: Some("elections-task".into()),
                role: None,
                ip: None,
            },
            subject: AuditSubject {
                kind: AuditSubjectKind::Node,
                id: Some("node1".into()),
                election_id: Some(1_779_265_552),
                labels: BTreeMap::new(),
            },
            message: None,
            payload: AuditEventPayload::ElectionsStakeSubmitted {
                election_id: 1_779_265_552,
                stake_nanotons: "50000000000000".into(),
                max_factor: 196_608,
                policy: "adaptive_split50".into(),
                submission_time: 1_779_265_400,
            },
        };

        assert_json_eq(
            &event,
            json!({
                "schema_version": 1,
                "id": FIXTURE_ID,
                "ts": FIXTURE_TS,
                "source": "elections",
                "severity": "info",
                "outcome": "success",
                "actor": {
                    "kind": "service",
                    "id": "elections-task"
                },
                "subject": {
                    "kind": "node",
                    "id": "node1",
                    "election_id": 1779265552
                },
                "event_type": "elections.stake_submitted",
                "data": {
                    "election_id": 1779265552,
                    "stake_nanotons": "50000000000000",
                    "max_factor": 196608,
                    "policy": "adaptive_split50",
                    "submission_time": 1779265400
                }
            }),
        );
    }

    #[test]
    fn serializes_stake_skipped_to_expected_json() {
        let event = AuditEvent {
            schema_version: 1,
            id: fixture_id(),
            ts: fixture_ts(),
            source: AuditSource::Elections,
            severity: AuditSeverity::Warn,
            outcome: AuditOutcome::Skipped,
            actor: AuditActor {
                kind: AuditActorKind::Service,
                id: Some("elections-task".into()),
                role: None,
                ip: None,
            },
            subject: AuditSubject {
                kind: AuditSubjectKind::Node,
                id: Some("node6".into()),
                election_id: Some(1_779_265_552),
                labels: BTreeMap::new(),
            },
            message: None,
            payload: AuditEventPayload::ElectionsStakeSkipped {
                election_id: 1_779_265_552,
                reason: StakeSkipReason::LowWalletBalance,
                required_nanotons: Some("1200000000".into()),
                available_nanotons: Some("900000000".into()),
            },
        };

        assert_json_eq(
            &event,
            json!({
                "schema_version": 1,
                "id": FIXTURE_ID,
                "ts": FIXTURE_TS,
                "source": "elections",
                "severity": "warn",
                "outcome": "skipped",
                "actor": {
                    "kind": "service",
                    "id": "elections-task"
                },
                "subject": {
                    "kind": "node",
                    "id": "node6",
                    "election_id": 1779265552
                },
                "event_type": "elections.stake_skipped",
                "data": {
                    "election_id": 1779265552,
                    "reason": "low_wallet_balance",
                    "required_nanotons": "1200000000",
                    "available_nanotons": "900000000"
                }
            }),
        );
    }

    #[test]
    fn serializes_config_updated_to_expected_json() {
        let event = AuditEvent {
            schema_version: 1,
            id: fixture_id(),
            ts: fixture_ts(),
            source: AuditSource::RestApi,
            severity: AuditSeverity::Info,
            outcome: AuditOutcome::Success,
            actor: AuditActor {
                kind: AuditActorKind::User,
                id: Some("admin".into()),
                role: Some("operator".into()),
                ip: None,
            },
            subject: AuditSubject {
                kind: AuditSubjectKind::Config,
                id: Some("elections".into()),
                election_id: None,
                labels: BTreeMap::new(),
            },
            message: None,
            payload: AuditEventPayload::RestApiConfigUpdated {
                operation: "elections.wait_updated".into(),
                changes: json!({
                    "sleep_period_pct": { "old": 0.2, "new": 0.9 },
                    "waiting_period_pct": { "old": 0.4, "new": 0.95 }
                }),
            },
        };

        assert_json_eq(
            &event,
            json!({
                "schema_version": 1,
                "id": FIXTURE_ID,
                "ts": FIXTURE_TS,
                "source": "rest_api",
                "severity": "info",
                "outcome": "success",
                "actor": {
                    "kind": "user",
                    "id": "admin",
                    "role": "operator"
                },
                "subject": {
                    "kind": "config",
                    "id": "elections"
                },
                "event_type": "rest_api.config_updated",
                "data": {
                    "operation": "elections.wait_updated",
                    "changes": {
                        "sleep_period_pct": { "old": 0.2, "new": 0.9 },
                        "waiting_period_pct": { "old": 0.4, "new": 0.95 }
                    }
                }
            }),
        );
    }

    fn sample_event(payload: AuditEventPayload) -> AuditEvent {
        AuditEvent {
            schema_version: 1,
            id: fixture_id(),
            ts: fixture_ts(),
            source: AuditSource::System,
            severity: AuditSeverity::Info,
            outcome: AuditOutcome::Success,
            actor: AuditActor { kind: AuditActorKind::System, id: None, role: None, ip: None },
            subject: AuditSubject {
                kind: AuditSubjectKind::Node,
                id: Some("node1".into()),
                election_id: None,
                labels: BTreeMap::new(),
            },
            message: None,
            payload,
        }
    }

    fn all_payload_variants() -> Vec<AuditEventPayload> {
        const ELECTION_ID: u64 = 1_779_265_552;
        vec![
            AuditEventPayload::ElectionsKeyGenerated {
                election_id: ELECTION_ID,
                pubkey: Some("aabb".into()),
            },
            AuditEventPayload::ElectionsStakeSubmitted {
                election_id: ELECTION_ID,
                stake_nanotons: "1".into(),
                max_factor: 1,
                policy: "all".into(),
                submission_time: 1,
            },
            AuditEventPayload::ElectionsStakeAccepted {
                election_id: ELECTION_ID,
                stake_nanotons: "50000000000000".into(),
            },
            AuditEventPayload::ElectionsStakeSkipped {
                election_id: ELECTION_ID,
                reason: StakeSkipReason::WithdrawRequestsPending,
                required_nanotons: None,
                available_nanotons: None,
            },
            AuditEventPayload::ElectionsWithdrawProcessed {
                election_id: ELECTION_ID,
                tx_hash: "abc".into(),
            },
            AuditEventPayload::ElectionsStakeRecovered {
                election_id: ELECTION_ID,
                amount_nanotons: "50000000000000".into(),
                tx_hash: Some("def".into()),
            },
            AuditEventPayload::RestApiConfigUpdated {
                operation: "patch".into(),
                changes: json!({ "path": "/v1/elections/settings" }),
            },
            AuditEventPayload::RestApiAuthLoginSuccess { username: "admin".into() },
            AuditEventPayload::RestApiAuthLoginRejected {
                username: "admin".into(),
                reason: "bad password".into(),
            },
            AuditEventPayload::RestApiTokenRejected { reason: "expired".into() },
            AuditEventPayload::SystemServiceStarted { version: "0.5.0".into() },
            AuditEventPayload::SystemServiceStopped,
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
    fn subject_labels_omitted_when_empty() {
        let event = AuditEvent {
            schema_version: 1,
            id: fixture_id(),
            ts: fixture_ts(),
            source: AuditSource::Elections,
            severity: AuditSeverity::Info,
            outcome: AuditOutcome::Success,
            actor: AuditActor { kind: AuditActorKind::Service, id: None, role: None, ip: None },
            subject: AuditSubject {
                kind: AuditSubjectKind::Node,
                id: Some("node1".into()),
                election_id: None,
                labels: BTreeMap::new(),
            },
            message: None,
            payload: AuditEventPayload::ElectionsWithdrawProcessed {
                election_id: 1_779_265_552,
                tx_hash: "abc".into(),
            },
        };

        let value = serde_json::to_value(&event).expect("serialize");
        let subject = value.get("subject").expect("subject").as_object().expect("object");
        assert!(!subject.contains_key("labels"), "empty labels must not appear in JSON: {value}");
    }

    #[test]
    fn default_config_matches_spec_defaults() {
        let cfg = AuditLogConfig::default();
        assert_eq!(cfg.path, PathBuf::from("./logs/audit.jsonl"));
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
        assert_eq!(cfg.ring_buffer_capacity, 10_000);
    }
}
