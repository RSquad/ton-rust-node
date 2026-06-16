/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::audit::{
    AuditEvent, AuditEventPayload, AuditOutcome, AuditSource, StakeSkipReason,
    participant::AuditTarget,
};
use chrono::{DateTime, Utc};
use common::{
    snapshot::{OurElectionParticipant, StakeSubmission},
    time_format,
    ton_utils::max_stake_factor_raw_to_multiplier,
};
use std::collections::BTreeMap;

/// Stake skip recorded from audit events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StakeSkip {
    pub ts: DateTime<Utc>,
    pub reason: StakeSkipReason,
}

/// Withdraw outcome recorded from audit events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Withdraw {
    pub ts: DateTime<Utc>,
    pub outcome: AuditOutcome,
    pub msg_hash: Option<String>,
    pub error: Option<String>,
}

/// Stake failure recorded from audit events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StakeFailure {
    pub ts: DateTime<Utc>,
    pub reason: String,
}

/// Per-node elections audit data keyed by election id.
#[derive(Debug, Clone, Default)]
pub struct NodeElectionProjection {
    pub stake_submissions: Vec<StakeSubmission>,
    pub stake_skips: Vec<StakeSkip>,
    pub withdraws: Vec<Withdraw>,
    pub stake_failures: Vec<StakeFailure>,
}

/// Aggregated elections projection from the in-memory audit ring buffer.
#[derive(Debug, Clone, Default)]
pub struct ElectionsProjection {
    /// `election_id` → `node_id` → per-node projection.
    pub nodes: BTreeMap<u64, BTreeMap<String, NodeElectionProjection>>,
}

/// Builds an [`ElectionsProjection`] from audit events, keeping only `election_ids`.
pub fn project_elections(events: &[AuditEvent], election_ids: &[u64]) -> ElectionsProjection {
    let mut projection = ElectionsProjection::default();

    for ev in events {
        if ev.payload.source() != AuditSource::Elections {
            continue;
        }
        let Some(election_id) = election_id_from_event(ev) else { continue };
        if !election_ids.contains(&election_id) {
            continue;
        }
        let node_id = node_id_from_event(ev).unwrap_or_default();
        let node =
            projection.nodes.entry(election_id).or_default().entry(node_id.clone()).or_default();

        match &ev.payload {
            AuditEventPayload::ElectionsStakeSubmitted {
                stake,
                max_factor,
                submission_time,
                ..
            } => {
                node.stake_submissions.push(StakeSubmission {
                    stake: stake.clone(),
                    max_factor: max_stake_factor_raw_to_multiplier(*max_factor),
                    submission_time: *submission_time,
                    submission_time_utc: time_format::format_ts(*submission_time),
                });
            }
            AuditEventPayload::ElectionsStakeSkipped { reason, .. } => {
                node.stake_skips.push(StakeSkip { ts: ev.ts, reason: *reason });
            }
            AuditEventPayload::ElectionsStakeFailed { reason } => {
                node.stake_failures.push(StakeFailure { ts: ev.ts, reason: reason.clone() });
            }
            AuditEventPayload::ElectionsWithdrawProcessed { msg_hash } => {
                node.withdraws.push(Withdraw {
                    ts: ev.ts,
                    outcome: AuditOutcome::Success,
                    msg_hash: Some(msg_hash.clone()),
                    error: None,
                });
            }
            AuditEventPayload::ElectionsWithdrawFailed { reason } => {
                node.withdraws.push(Withdraw {
                    ts: ev.ts,
                    outcome: AuditOutcome::Failure,
                    msg_hash: None,
                    error: Some(reason.clone()),
                });
            }
            _ => {}
        }
    }

    projection
}

/// Merges projected audit data into live snapshot participants for `election_id`.
pub fn merge_projection_into_participants(
    participants: &mut [OurElectionParticipant],
    projection: &ElectionsProjection,
    election_id: u64,
) {
    let Some(by_node) = projection.nodes.get(&election_id) else { return };

    for participant in participants.iter_mut() {
        let Some(node_proj) = by_node.get(&participant.node_id) else { continue };

        merge_stake_submissions(&mut participant.stake_submissions, &node_proj.stake_submissions);

        match resolve_last_error(node_proj, &participant.stake_submissions) {
            LastErrorUpdate::Set(msg) => participant.last_error = Some(msg),
            LastErrorUpdate::Clear => participant.last_error = None,
            LastErrorUpdate::Keep => {}
        }
    }
}

enum LastErrorUpdate {
    Set(String),
    Clear,
    Keep,
}

/// Derives `last_error` for the REST/CLI "current cycle is failing" readout.
///
/// When the latest stake submission post-dates the latest error-class audit event,
/// the error is suppressed — a later successful submit means the node recovered.
/// When the projection has no error events, the snapshot value is left unchanged.
fn resolve_last_error(
    node_proj: &NodeElectionProjection,
    submissions: &[StakeSubmission],
) -> LastErrorUpdate {
    let mut candidates: Vec<(DateTime<Utc>, String)> = Vec::new();

    for skip in &node_proj.stake_skips {
        candidates.push((skip.ts, format!("stake skipped: {}", format_skip_reason(skip.reason))));
    }
    for failure in &node_proj.stake_failures {
        candidates.push((failure.ts, format!("stake failed: {}", failure.reason)));
    }
    for withdraw in &node_proj.withdraws {
        if withdraw.outcome == AuditOutcome::Failure
            && let Some(error) = &withdraw.error
        {
            candidates.push((withdraw.ts, format!("withdraw failed: {error}")));
        }
    }

    if candidates.is_empty() {
        return LastErrorUpdate::Keep;
    }

    candidates.sort_by_key(|(ts, _)| *ts);
    let (latest_err_ts, msg) = candidates.last().expect("non-empty");

    let latest_submission_time = submissions.iter().map(|s| s.submission_time).max();
    if latest_submission_time.is_some_and(|t| t > latest_err_ts.timestamp() as u64) {
        return LastErrorUpdate::Clear;
    }

    LastErrorUpdate::Set(msg.clone())
}

fn merge_stake_submissions(existing: &mut Vec<StakeSubmission>, projected: &[StakeSubmission]) {
    for sub in projected {
        let duplicate = existing
            .iter()
            .any(|s| s.submission_time == sub.submission_time && s.stake == sub.stake);
        if !duplicate {
            existing.push(sub.clone());
        }
    }
    existing.sort_by_key(|s| s.submission_time);
}

fn format_skip_reason(reason: StakeSkipReason) -> &'static str {
    match reason {
        StakeSkipReason::LowWalletBalance => "low_wallet_balance",
        StakeSkipReason::WithdrawRequestsPending => "withdraw_requests_pending",
        StakeSkipReason::PoolNotReady => "pool_not_ready",
        StakeSkipReason::AdaptiveSleepingPeriod => "adaptive_sleeping_period",
        StakeSkipReason::AdaptiveWaitingPeriod => "adaptive_waiting_period",
        StakeSkipReason::ElectionsDisabled => "elections_disabled",
        StakeSkipReason::RecoverPending => "recover_pending",
        StakeSkipReason::InsufficientStakeFunds => "insufficient_stake_funds",
    }
}

pub(crate) fn election_id_from_event(ev: &AuditEvent) -> Option<u64> {
    match &ev.target {
        AuditTarget::Node { election_id: Some(id), .. } => Some(*id),
        AuditTarget::Elections { election_id } => Some(*election_id),
        _ => None,
    }
}

pub(crate) fn node_id_from_event(ev: &AuditEvent) -> Option<String> {
    match &ev.target {
        AuditTarget::Node { id, .. } => Some(id.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{AuditActor, AuditEvent};
    use chrono::TimeZone;

    const ELECTION_ID: u64 = 1_779_265_552;
    const NODE_ID: &str = "node-1";

    fn elections_actor() -> AuditActor {
        AuditActor::service("elections-task")
    }

    #[test]
    fn projection_groups_events_by_election_id() {
        const OTHER_ELECTION: u64 = ELECTION_ID + 100;
        const NODE_B: &str = "node-2";

        let events = vec![
            AuditEvent::elections_stake_submitted(
                elections_actor(),
                NODE_ID,
                ELECTION_ID,
                crate::audit::ElectionsStakeSubmittedParams {
                    stake: "100000000000".into(),
                    max_factor: 196_608,
                    policy: "split50".into(),
                    submission_time: 1_700_000_000,
                },
            ),
            AuditEvent::elections_stake_skipped(
                elections_actor(),
                NODE_B,
                OTHER_ELECTION,
                StakeSkipReason::ElectionsDisabled,
                None,
                None,
            ),
        ];

        let projection = project_elections(&events, &[ELECTION_ID, OTHER_ELECTION]);

        let current = projection.nodes.get(&ELECTION_ID).unwrap().get(NODE_ID).unwrap();
        assert_eq!(current.stake_submissions.len(), 1);
        assert!(current.stake_skips.is_empty());

        let other = projection.nodes.get(&OTHER_ELECTION).unwrap().get(NODE_B).unwrap();
        assert!(other.stake_submissions.is_empty());
        assert_eq!(other.stake_skips.len(), 1);
        assert_eq!(other.stake_skips[0].reason, StakeSkipReason::ElectionsDisabled);
    }

    #[test]
    fn projection_ignores_non_elections_source() {
        let events = vec![
            AuditEvent::rest_api_auth_login_success(
                AuditActor::user("alice", Some("admin".into()), None),
                "alice",
            ),
            AuditEvent::elections_stake_failed(elections_actor(), NODE_ID, ELECTION_ID, "boom"),
        ];

        let projection = project_elections(&events, &[ELECTION_ID]);

        assert_eq!(projection.nodes.len(), 1);
        let node = projection.nodes.get(&ELECTION_ID).unwrap().get(NODE_ID).unwrap();
        assert_eq!(node.stake_failures.len(), 1);
        assert_eq!(node.stake_failures[0].reason, "boom");
    }

    #[test]
    fn projection_only_includes_requested_election_ids() {
        let events = vec![
            AuditEvent::elections_stake_failed(elections_actor(), NODE_ID, ELECTION_ID, "current"),
            AuditEvent::elections_stake_failed(
                elections_actor(),
                NODE_ID,
                ELECTION_ID + 1,
                "excluded",
            ),
        ];

        let projection = project_elections(&events, &[ELECTION_ID]);

        assert_eq!(projection.nodes.len(), 1);
        let node = projection.nodes.get(&ELECTION_ID).unwrap().get(NODE_ID).unwrap();
        assert_eq!(node.stake_failures[0].reason, "current");
    }

    #[test]
    fn merge_projection_enriches_participants_without_duplicates() {
        let events = vec![
            AuditEvent::elections_stake_submitted(
                elections_actor(),
                NODE_ID,
                ELECTION_ID,
                crate::audit::ElectionsStakeSubmittedParams {
                    stake: "200000000000".into(),
                    max_factor: 196_608,
                    policy: "all".into(),
                    submission_time: 1_700_000_100,
                },
            ),
            AuditEvent::elections_stake_skipped(
                elections_actor(),
                NODE_ID,
                ELECTION_ID,
                StakeSkipReason::InsufficientStakeFunds,
                Some("100".into()),
                Some("50".into()),
            ),
        ];
        let projection = project_elections(&events, &[ELECTION_ID]);

        let mut participants = vec![OurElectionParticipant {
            node_id: NODE_ID.to_string(),
            stake_submissions: vec![StakeSubmission {
                stake: "200000000000".into(),
                max_factor: 3.0,
                submission_time: 1_700_000_100,
                submission_time_utc: time_format::format_ts(1_700_000_100),
            }],
            ..Default::default()
        }];

        merge_projection_into_participants(&mut participants, &projection, ELECTION_ID);

        assert_eq!(participants[0].stake_submissions.len(), 1);
        assert_eq!(
            participants[0].last_error.as_deref(),
            Some("stake skipped: insufficient_stake_funds")
        );
    }

    #[test]
    fn merge_projection_clears_last_error_when_submission_postdates_error() {
        let mut skip = AuditEvent::elections_stake_skipped(
            elections_actor(),
            NODE_ID,
            ELECTION_ID,
            StakeSkipReason::InsufficientStakeFunds,
            None,
            None,
        );
        skip.ts = Utc.timestamp_opt(1_700_000_000, 0).unwrap();

        let mut submit = AuditEvent::elections_stake_submitted(
            elections_actor(),
            NODE_ID,
            ELECTION_ID,
            crate::audit::ElectionsStakeSubmittedParams {
                stake: "200000000000".into(),
                max_factor: 196_608,
                policy: "all".into(),
                submission_time: 1_700_000_100,
            },
        );
        submit.ts = Utc.timestamp_opt(1_700_000_100, 0).unwrap();

        let projection = project_elections(&[skip, submit], &[ELECTION_ID]);

        let mut participants = vec![OurElectionParticipant {
            node_id: NODE_ID.to_string(),
            last_error: Some("stale snapshot error".into()),
            ..Default::default()
        }];

        merge_projection_into_participants(&mut participants, &projection, ELECTION_ID);

        assert_eq!(participants[0].stake_submissions.len(), 1);
        assert!(participants[0].last_error.is_none());
    }

    #[test]
    fn merge_projection_still_shows_error_when_latest_error_postdates_submission() {
        let mut submit = AuditEvent::elections_stake_submitted(
            elections_actor(),
            NODE_ID,
            ELECTION_ID,
            crate::audit::ElectionsStakeSubmittedParams {
                stake: "200000000000".into(),
                max_factor: 196_608,
                policy: "all".into(),
                submission_time: 1_700_000_000,
            },
        );
        submit.ts = Utc.timestamp_opt(1_700_000_000, 0).unwrap();

        let mut skip = AuditEvent::elections_stake_skipped(
            elections_actor(),
            NODE_ID,
            ELECTION_ID,
            StakeSkipReason::PoolNotReady,
            None,
            None,
        );
        skip.ts = Utc.timestamp_opt(1_700_000_100, 0).unwrap();

        let projection = project_elections(&[submit, skip], &[ELECTION_ID]);

        let mut participants =
            vec![OurElectionParticipant { node_id: NODE_ID.to_string(), ..Default::default() }];

        merge_projection_into_participants(&mut participants, &projection, ELECTION_ID);

        assert_eq!(participants[0].last_error.as_deref(), Some("stake skipped: pool_not_ready"));
    }

    #[test]
    fn merge_projection_is_noop_when_ring_projection_empty() {
        let mut participants = vec![OurElectionParticipant {
            node_id: NODE_ID.to_string(),
            last_error: Some("existing".into()),
            ..Default::default()
        }];

        merge_projection_into_participants(
            &mut participants,
            &ElectionsProjection::default(),
            ELECTION_ID,
        );

        assert_eq!(participants[0].last_error.as_deref(), Some("existing"));
        assert!(participants[0].stake_submissions.is_empty());
    }
}
