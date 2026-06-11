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

/// Projected stake submission from audit events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedStakeSubmission {
    pub ts: DateTime<Utc>,
    pub node_id: String,
    pub stake: String,
    pub max_factor: u32,
    pub policy: String,
    pub submission_time: u64,
}

/// Projected stake skip from audit events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedStakeSkip {
    pub ts: DateTime<Utc>,
    pub node_id: String,
    pub reason: StakeSkipReason,
}

/// Projected withdraw outcome from audit events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedWithdraw {
    pub ts: DateTime<Utc>,
    pub node_id: String,
    pub outcome: AuditOutcome,
    pub msg_hash: Option<String>,
    pub error: Option<String>,
}

/// Projected stake failure from audit events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedStakeFailure {
    pub ts: DateTime<Utc>,
    pub node_id: String,
    pub reason: String,
}

/// Per-node elections audit data keyed by election id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeElectionProjection {
    pub stake_submissions: Vec<ProjectedStakeSubmission>,
    pub stake_skips: Vec<ProjectedStakeSkip>,
    pub withdraws: Vec<ProjectedWithdraw>,
    pub stake_failures: Vec<ProjectedStakeFailure>,
}

/// Aggregated elections projection from the in-memory audit ring buffer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ElectionsProjection {
    /// `election_id` → `node_id` → per-node projection.
    pub nodes: BTreeMap<u64, BTreeMap<String, NodeElectionProjection>>,
}

/// Collects up to `max` recent election ids: the live cycle first, then others seen in `events`.
pub fn collect_recent_election_ids(
    current_election_id: Option<u64>,
    events: &[AuditEvent],
    max: usize,
) -> Vec<u64> {
    if max == 0 {
        return Vec::new();
    }

    let mut ids = Vec::new();
    let mut seen = std::collections::HashSet::new();

    if let Some(id) = current_election_id {
        seen.insert(id);
        ids.push(id);
        if ids.len() >= max {
            return ids;
        }
    }

    for ev in events.iter().rev() {
        let Some(election_id) = election_id_from_event(ev) else { continue };
        if seen.insert(election_id) {
            ids.push(election_id);
            if ids.len() >= max {
                break;
            }
        }
    }

    ids
}

/// Builds an [`ElectionsProjection`] from audit events, keeping only `recent_election_ids`.
pub fn project_elections(
    events: &[AuditEvent],
    recent_election_ids: &[u64],
) -> ElectionsProjection {
    let mut projection = ElectionsProjection::default();

    for ev in events {
        if ev.payload.source() != AuditSource::Elections {
            continue;
        }
        let Some(election_id) = election_id_from_event(ev) else { continue };
        if !recent_election_ids.contains(&election_id) {
            continue;
        }
        let node_id = node_id_from_event(ev).unwrap_or_default();
        let node =
            projection.nodes.entry(election_id).or_default().entry(node_id.clone()).or_default();

        match &ev.payload {
            AuditEventPayload::ElectionsStakeSubmitted {
                stake,
                max_factor,
                policy,
                submission_time,
            } => {
                node.stake_submissions.push(ProjectedStakeSubmission {
                    ts: ev.ts,
                    node_id,
                    stake: stake.clone(),
                    max_factor: *max_factor,
                    policy: policy.clone(),
                    submission_time: *submission_time,
                });
            }
            AuditEventPayload::ElectionsStakeSkipped { reason, .. } => {
                node.stake_skips.push(ProjectedStakeSkip { ts: ev.ts, node_id, reason: *reason });
            }
            AuditEventPayload::ElectionsStakeFailed { reason } => {
                node.stake_failures.push(ProjectedStakeFailure {
                    ts: ev.ts,
                    node_id,
                    reason: reason.clone(),
                });
            }
            AuditEventPayload::ElectionsWithdrawProcessed { msg_hash } => {
                node.withdraws.push(ProjectedWithdraw {
                    ts: ev.ts,
                    node_id,
                    outcome: AuditOutcome::Success,
                    msg_hash: Some(msg_hash.clone()),
                    error: None,
                });
            }
            AuditEventPayload::ElectionsWithdrawFailed { reason } => {
                node.withdraws.push(ProjectedWithdraw {
                    ts: ev.ts,
                    node_id,
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

        if let Some(err) = latest_error_message(node_proj) {
            participant.last_error = Some(err);
        }
    }
}

fn merge_stake_submissions(
    existing: &mut Vec<StakeSubmission>,
    projected: &[ProjectedStakeSubmission],
) {
    for sub in projected {
        let converted = projected_to_stake_submission(sub);
        let duplicate = existing
            .iter()
            .any(|s| s.submission_time == converted.submission_time && s.stake == converted.stake);
        if !duplicate {
            existing.push(converted);
        }
    }
    existing.sort_by_key(|s| s.submission_time);
}

fn projected_to_stake_submission(sub: &ProjectedStakeSubmission) -> StakeSubmission {
    StakeSubmission {
        stake: sub.stake.clone(),
        max_factor: max_stake_factor_raw_to_multiplier(sub.max_factor),
        submission_time: sub.submission_time,
        submission_time_utc: time_format::format_ts(sub.submission_time),
    }
}

fn latest_error_message(node_proj: &NodeElectionProjection) -> Option<String> {
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

    candidates.sort_by_key(|(ts, _)| *ts);
    candidates.last().map(|(_, msg)| msg.clone())
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

    const ELECTION_ID: u64 = 1_779_265_552;
    const NODE_ID: &str = "node-1";

    fn elections_actor() -> AuditActor {
        AuditActor::service("elections-task")
    }

    #[test]
    fn collect_recent_election_ids_returns_empty_when_max_is_zero() {
        let events = vec![AuditEvent::elections_stake_failed(
            elections_actor(),
            NODE_ID,
            ELECTION_ID,
            "err",
        )];
        assert!(collect_recent_election_ids(Some(ELECTION_ID), &events, 0).is_empty());
        assert!(collect_recent_election_ids(None, &events, 0).is_empty());
    }

    #[test]
    fn collect_recent_election_ids_respects_max_after_current_id() {
        let events = vec![
            AuditEvent::elections_stake_failed(elections_actor(), NODE_ID, ELECTION_ID + 1, "a"),
            AuditEvent::elections_stake_failed(elections_actor(), NODE_ID, ELECTION_ID + 2, "b"),
        ];
        let ids = collect_recent_election_ids(Some(ELECTION_ID), &events, 1);
        assert_eq!(ids, vec![ELECTION_ID]);
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
    fn projection_only_includes_recent_election_ids() {
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
