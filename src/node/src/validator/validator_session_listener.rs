/*
 * Copyright (C) 2019-2024 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::consensus::{
    get_elapsed_time, AsyncRequestPtr, BlockHash, BlockPayloadPtr, BlockSourceInfo,
    CollationParentHint, CommittedBlockProofCallback, ConsensusReplayListener, PublicKey,
    PublicKeyHash, SessionId, SessionListener, SessionStats, ValidatorBlockCandidateCallback,
    ValidatorBlockCandidateDecisionCallback,
};
use crate::validator::validator_group::{ValidatorGroup, ValidatorGroupStatus};
use std::{
    fmt,
    sync::{atomic::Ordering, Arc},
    time::{Duration, SystemTime, SystemTimeError},
};
use ton_block::{BlockIdExt, BlockSignaturesVariant, ShardIdent};

pub struct OnBlockCommitted {
    source_info: BlockSourceInfo,
    root_hash: BlockHash,
    file_hash: BlockHash,
    data: BlockPayloadPtr,
    signatures: BlockSignaturesVariant,
    approve_signatures: Vec<(PublicKeyHash, BlockPayloadPtr)>,
}

#[allow(clippy::enum_variant_names)]
pub enum ValidationAction {
    OnGenerateSlot {
        source_info: BlockSourceInfo,
        request: AsyncRequestPtr,
        parent: CollationParentHint,
        callback: ValidatorBlockCandidateCallback,
    },
    OnCandidate {
        source_info: BlockSourceInfo,
        root_hash: BlockHash,
        data: BlockPayloadPtr,
        collated_data: BlockPayloadPtr,
        callback: ValidatorBlockCandidateDecisionCallback,
    },
    OnBlockCommitted(OnBlockCommitted),
    OnBlockSkipped {
        round: u32,
    },
    OnGetApprovedCandidate {
        source: PublicKey,
        root_hash: BlockHash,
        file_hash: BlockHash,
        collated_data_hash: BlockHash,
        callback: ValidatorBlockCandidateCallback,
    },
    OnGetCommittedCandidate {
        block_id: BlockIdExt,
        callback: CommittedBlockProofCallback,
    },
}

impl fmt::Display for OnBlockCommitted {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "OnBlockCommitted round: {}", self.source_info.priority.round)
    }
}

impl fmt::Display for ValidationAction {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            ValidationAction::OnGenerateSlot { ref source_info, ref request, .. } => {
                write!(
                    f,
                    "OnGenerateSlot round: {}, request_id: {}",
                    source_info.priority.round,
                    request.get_request_id()
                )
            }

            ValidationAction::OnCandidate { ref source_info, .. } => {
                write!(f, "OnCandidate round: {}", source_info.priority.round)
            }

            ValidationAction::OnBlockCommitted(ref committed) => {
                write!(f, "OnBlockCommitted round: {}", committed.source_info.priority.round)
            }

            ValidationAction::OnBlockSkipped { round } => {
                write!(f, "OnBlockSkipped round: {}", round)
            }

            ValidationAction::OnGetApprovedCandidate { .. } => write!(f, "OnGetApprovedCandidate"),

            ValidationAction::OnGetCommittedCandidate { ref block_id, .. } => {
                write!(f, "OnGetCommittedCandidate block_id={}", block_id)
            }
        }
    }
}

pub struct ValidatorSessionListener {
    queue: tokio::sync::mpsc::UnboundedSender<ValidationAction>,
    session_id: SessionId,
    shard: ShardIdent,
}

impl ValidatorSessionListener {
    pub fn info_round(&self, round: Option<u32>) -> String {
        format!("ValidatorSessionListener; round = {:?}", round)
    }

    fn do_send_general(&self, round: Option<u32>, action: ValidationAction) {
        if let Err(error) = self.queue.send(action) {
            log::error!(target: "validator", "Cannot send validator action: `{}`, {}",
                error,
                self.info_round(round));
        }
    }

    pub fn create(
        session_id: SessionId,
        shard: ShardIdent,
    ) -> (Self, tokio::sync::mpsc::UnboundedReceiver<ValidationAction>) {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        (ValidatorSessionListener { queue: sender, session_id, shard }, receiver)
    }
}

impl SessionListener for ValidatorSessionListener {
    /// New block candidate appears -- validate it
    fn on_candidate(
        &self,
        source_info: BlockSourceInfo,
        root_hash: BlockHash,
        data: BlockPayloadPtr,
        collated_data: BlockPayloadPtr,
        callback: ValidatorBlockCandidateDecisionCallback,
    ) {
        let round = source_info.priority.round;
        log::info!(target: "validator", "SessionListener::on_candidate: new candidate from source {} with hash {} appeared, round={}, priority={}, first_block_round={} (session_id = {:x}, shard = {})",
            source_info.source.id(), root_hash.to_hex_string(), round, source_info.priority.priority, source_info.priority.first_block_round, self.session_id, self.shard);
        self.do_send_general(
            Some(round),
            ValidationAction::OnCandidate { source_info, root_hash, data, collated_data, callback },
        );
    }

    /// New block should be collated -- generate_block_candidate
    fn on_generate_slot(
        &self,
        source_info: BlockSourceInfo,
        request: AsyncRequestPtr,
        parent: CollationParentHint,
        callback: ValidatorBlockCandidateCallback,
    ) {
        let round = source_info.priority.round;
        let request_id = request.get_request_id();
        log::info!(target: "validator",
            "SessionListener::on_generate_slot: collator request, round={}, priority={}, first_block_round={} (session_id = {:x}, shard = {}, collation request ID = {})",
            round,
            source_info.priority.priority,
            source_info.priority.first_block_round,
            self.session_id,
            self.shard,
            request_id
        );
        self.do_send_general(
            Some(round),
            ValidationAction::OnGenerateSlot { source_info, request, parent, callback },
        );
    }

    /// New block is committed - apply it and write to the database
    fn on_block_committed(
        &self,
        source_info: BlockSourceInfo,
        root_hash: BlockHash,
        file_hash: BlockHash,
        data: BlockPayloadPtr,
        signatures: BlockSignaturesVariant,
        approve_signatures: Vec<(PublicKeyHash, BlockPayloadPtr)>,
        _stats: SessionStats,
    ) {
        let round = source_info.priority.round;
        log::info!(target: "validator", "SessionListener::on_block_committed: new block from source {} with hash {:?} has been committed, round={}, priority={} (session_id = {:x}, shard = {})",
            source_info.source.id(), root_hash, round, source_info.priority.priority, self.session_id, self.shard);
        self.do_send_general(
            Some(round),
            ValidationAction::OnBlockCommitted(OnBlockCommitted {
                source_info,
                root_hash,
                file_hash,
                data,
                signatures,
                approve_signatures,
            }),
        );
    }

    /// Block generation is skipped for the current round
    fn on_block_skipped(&self, round: u32) {
        log::info!(target: "validator", "SessionListener::on_block_skipped, {} (session_id = {:x}, shard = {})", self.info_round(Some(round)), self.session_id, self.shard);
        self.do_send_general(Some(round), ValidationAction::OnBlockSkipped { round });
    }

    /// Ask validator to read block candidate from the database
    fn get_approved_candidate(
        &self,
        source: PublicKey,
        root_hash: BlockHash,
        file_hash: BlockHash,
        collated_data_hash: BlockHash,
        callback: ValidatorBlockCandidateCallback,
    ) {
        log::info!(target: "validator", "SessionListener::on_get_approved_candidate, {} (session_id = {:x}, shard = {})", self.info_round(None), self.session_id, self.shard);
        self.do_send_general(
            None,
            ValidationAction::OnGetApprovedCandidate {
                source,
                root_hash,
                file_hash,
                collated_data_hash,
                callback,
            },
        );
    }

    /// Download committed block proof from full-node
    fn get_committed_candidate(&self, block_id: BlockIdExt, callback: CommittedBlockProofCallback) {
        log::info!(
            target: "validator",
            "SessionListener::get_committed_candidate block_id={} (session_id={:x}, shard={})",
            block_id, self.session_id, self.shard
        );
        self.do_send_general(
            None,
            ValidationAction::OnGetCommittedCandidate { block_id, callback },
        );
    }
}

impl ConsensusReplayListener for ValidatorSessionListener {
    fn replay_started(&self) {
        log::info!(target: "validator", "ConsensusReplayListener: started");
    }

    fn replay_finished(&self) {
        log::info!(target: "validator", "ConsensusReplayListener: finished");

        //self.data.lock().unwrap().replay_finished = true;
        unimplemented!("Replay not available");
    }
}

async fn process_validation_action(action: ValidationAction, g: Arc<ValidatorGroup>) {
    let action_str = action.to_string();
    let next_block_descr = g.get_next_block_descr(None).await;
    log::info!(
        target: "validator",
        "({}): Processing action: {}, {}", next_block_descr, action_str, g.info().await
    );
    match action {
        ValidationAction::OnGenerateSlot { source_info, request, parent, callback } => {
            let round = source_info.priority.round;
            let priority = source_info.priority.priority;
            let first_block_round = source_info.priority.first_block_round;
            let is_first_block = round == first_block_round;

            log::trace!(
                target: "validator",
                "({}): OnGenerateSlot: round={}, priority={}, first_block={}, request_id={}, parent={:?}",
                next_block_descr,
                round,
                priority,
                is_first_block,
                request.get_request_id(),
                parent
            );

            g.on_generate_slot(source_info, request, parent, callback).await
        }

        ValidationAction::OnCandidate { source_info, root_hash, data, collated_data, callback } => {
            let round = source_info.priority.round;
            let priority = source_info.priority.priority;
            let first_block_round = source_info.priority.first_block_round;
            let is_first_block = round == first_block_round;
            let is_high_priority = priority == 0;

            log::trace!(
                target: "validator",
                "({}): OnCandidate: round={}, source={}, priority={}, first_block={}, root_hash={:x}",
                next_block_descr,
                round,
                source_info.source.id(),
                priority,
                is_first_block,
                root_hash
            );

            // Priority-based validation scheduling: log high-priority validations
            if is_high_priority {
                log::trace!(
                    target: "validator",
                    "({}): High-priority validation (priority=0) for round {}",
                    next_block_descr,
                    round
                );
            }

            g.on_candidate(source_info, root_hash, data, collated_data, callback).await
        }

        ValidationAction::OnBlockCommitted(OnBlockCommitted {
            source_info,
            root_hash,
            file_hash,
            data,
            signatures,
            approve_signatures,
        }) =>
        //panic!("ValidatorAction::OnBlockCommitted must be processed in a separate thread!");
        {
            let round = source_info.priority.round;
            let source = source_info.source;
            let priority = source_info.priority.priority;
            let first_block_round = source_info.priority.first_block_round;

            log::trace!(
                target: "validator",
                "({}): OnBlockCommitted: round={}, source={}, priority={}, first_block={}, root_hash={:x}",
                next_block_descr,
                round,
                source.id(),
                priority,
                round == first_block_round,
                root_hash
            );

            g.on_block_committed(
                round,
                source,
                root_hash,
                file_hash,
                data,
                signatures,
                approve_signatures,
            )
            .await
        }

        ValidationAction::OnBlockSkipped { round } => g.on_block_skipped(round).await,

        ValidationAction::OnGetApprovedCandidate {
            source,
            root_hash,
            file_hash,
            collated_data_hash,
            callback,
        } => {
            g.on_get_approved_candidate(source, root_hash, file_hash, collated_data_hash, callback)
                .await
        }

        ValidationAction::OnGetCommittedCandidate { block_id, callback } => {
            g.on_get_committed_candidate(block_id, callback).await
        }
    }
}

const VALIDATION_ACTION_TOO_LONG: Duration = Duration::from_secs(3);
const VALIDATION_QUEUE_EMPTY_TOO_LONG: Duration = Duration::from_secs(10);
const VALIDATION_QUEUE_TIMEOUT: Duration = Duration::from_millis(50);

pub async fn process_validation_queue(
    mut queue: tokio::sync::mpsc::UnboundedReceiver<ValidationAction>,
    g: Arc<ValidatorGroup>,
    rt: tokio::runtime::Handle,
) {
    let mut last_action = SystemTime::now();

    'queue_loop: while g.clone().get_status().await < ValidatorGroupStatus::Stopping {
        let g_clone = g.clone();
        let g_info = g_clone.info().await;
        let next_block_descr = g_clone.get_next_block_descr(None).await;

        match tokio::time::timeout(VALIDATION_QUEUE_TIMEOUT, queue.recv()).await {
            Ok(None) => {
                // Channel closed
                log::warn!(
                    target: "validator",
                    "({}): Session {}: validation action queue disconnected, exiting",
                    next_block_descr,
                    g_info
                );
                break 'queue_loop;
            }
            Err(_elapsed) => match (last_action + VALIDATION_QUEUE_EMPTY_TOO_LONG).elapsed() {
                Ok(_) => {
                    g.stalled.store(true, Ordering::Relaxed);
                    log::info!(
                        target: "validator",
                        "({}): Session {}: validation action queue empty (stalled=true)",
                        next_block_descr,
                        g_info
                    );
                    last_action = SystemTime::now();
                }
                Err(SystemTimeError { .. }) => (),
            },
            Ok(Some(action)) => {
                last_action = SystemTime::now();
                g.stalled.store(false, Ordering::Relaxed);
                let action_str = action.to_string();

                log::info!(
                    target: "validator",
                    "({}): Validation action request received from queue: {}, {}",
                    next_block_descr, action_str, g_info
                );

                let start_time = SystemTime::now();
                let mut join_handle = rt.spawn(async move {
                    process_validation_action(action, g_clone).await;
                });

                loop {
                    match tokio::time::timeout(VALIDATION_ACTION_TOO_LONG, &mut join_handle).await {
                        Ok(res) => {
                            let res_txt = match res {
                                Ok(_) => "Ok".to_string(),
                                Err(r) => format!("Error: {}", r),
                            };
                            log::info!(
                                target: "validator",
                                "({}): Validation action {}, {} finished: `{}`",
                                next_block_descr, action_str, g_info, res_txt
                            );
                            break;
                        }
                        Err(tokio::time::error::Elapsed { .. }) => log::warn!(
                            target: "validator",
                            "({}): Validation action {}, {} takes {:#?} and not finished",
                            next_block_descr, action_str, g_info, get_elapsed_time(&start_time)
                        ),
                    }

                    if g.clone().get_status().await == ValidatorGroupStatus::Stopped {
                        log::error!(
                            target: "validator",
                            "({}): Session processing cancelled, \
                            but validation action took {:#?} and not finished {}, {}",
                            next_block_descr, get_elapsed_time(&start_time), action_str, g_info
                        );
                        break 'queue_loop;
                    }
                }
            }
        }
    }

    log::info!(target: "validator",
        "({}): Exiting from validation queue processing: {}",
        g.get_next_block_descr(None).await,
        g.info().await
    );
}
