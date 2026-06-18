/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! # Candidate-phase controller
//!
//! Owns the candidate domain that used to live inline on `SessionProcessor`:
//!
//! - **Ingress** ([`CandidateController::receive_candidate`]): precheck →
//!   parse/verify → dedup → book insert + TL-bytes cache + DB payload/info
//!   persist. Returns an [`IngressOutcome`] describing what the thin
//!   `SessionProcessor::on_candidate_received` shell must fan out across the
//!   other controllers (consensus / validation / callbacks / notar-cert / FSM
//!   pump). The controller itself never touches another controller — it stays a
//!   pure candidate-domain entity behind the narrow [`CandidateBackend`].
//! - **Outbound repair** ([`CandidateController::ensure_candidate_available`],
//!   [`CandidateController::request_candidate`]): the resolver-availability walk
//!   and the throttled `requestCandidate` scheduler, backed by the owned
//!   `requested_candidates` throttle map. Delayed retries re-enter the
//!   controller through its [`ControllerQueuePtr`] (no `SessionProcessor` name).
//! - **Serving** ([`CandidateController::serve_query_fallback`]): the SXRCV-side
//!   `RequestCandidate` fallback that reconstructs a body / notar-cert from the
//!   owned [`CandidateBook`] and the database.
//!
//! ## State ownership
//!
//! The controller owns the [`CandidateBook`] (received candidates, the TL-bytes
//! `candidate_data_cache`, and the per-slot broadcast-dedup map) and the
//! `requested_candidates` repair throttle. Other controllers that need to read
//! the book (collation / validation / consensus backends, startup recovery)
//! source it through [`CandidateController::book`] /
//! [`CandidateController::book_mut`] at the composition root.
//!
//! ## Backend seam
//!
//! [`CandidateBackend`] is deliberately narrow: only the cross-aspect
//! reads/effects ingress + repair + serving need (FSM state, the database
//! controller, the candidate-info persist effect, the validation dedup probe,
//! the collation half of the `BlockIdExt → RawCandidateId` resolve, and the two
//! receiver repair effects). Everything else — session description / leader /
//! keys / timing, telemetry counters and notes, and the book / throttle — is
//! held directly on the controller, so those need no backend hop.

use crate::{
    block::{
        CandidateId as BlockCandidateId, RawCandidate, RawCandidateId, SlotIndex, ValidatorIndex,
    },
    candidate_book::{CandidateBook, ReceivedCandidate, EMPTY_CHAIN_WARN_DEPTH, MAX_CHAIN_DEPTH},
    controller_queue::{Controlled, ControllerQueueExt, ControllerQueuePtr},
    database::CandidateInfoRecord,
    database_controller::DatabaseController,
    session_description::SessionDescription,
    session_telemetry::SessionTelemetry,
    simplex_state::SimplexState,
    trace_collector::TraceCollector,
    SessionId,
};
use consensus_common::{
    check_execution_time, CandidateObservedFlags, EnsureCandidateAvailabilityOptions,
};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime},
};
use ton_api::{
    serialize_boxed,
    ton::consensus::{
        candidatedata::Empty as CandidateDataEmpty, candidateid::CandidateId,
        simplex::candidateandcert::CandidateAndCert, CandidateData, CandidateHashData,
    },
    IntoBoxed,
};
use ton_block::{error, BlockIdExt, Result, UInt256};

/*
    Repair constants
*/

/// Delay before requesting a missing candidate from peers.
///
/// This allows time for the broadcast to arrive naturally before triggering a query.
const CANDIDATE_REQUEST_DELAY: Duration = Duration::from_secs(1);

/// Minimum interval between repeated `requestCandidate` attempts for the same (slot,hash).
///
/// Under network partitions, a single request may time out; we must retry, but not spam.
pub(crate) const CANDIDATE_REQUEST_RETRY_INTERVAL: Duration = Duration::from_secs(2);

/// Delay between deferred retries of `ensure_candidate_available` when the
/// `BlockIdExt → RawCandidateId` mapping is not yet known.
pub(crate) const RESOLVER_AVAILABILITY_RETRY_DELAY: Duration = Duration::from_millis(500);

/// Maximum number of deferred retries before giving up on resolving a
/// `BlockIdExt` to `RawCandidateId` for the resolver.
pub(crate) const RESOLVER_AVAILABILITY_MAX_RETRIES: u32 = 6;

/// Validator-layer observe payload surfaced by ingress for the shell to deliver
/// via `SessionCallbacks::notify_candidate_observed` (non-empty blocks only).
pub(crate) struct CandidateObserved {
    pub block_id: BlockIdExt,
    pub data: crate::BlockPayloadPtr,
    pub collated_data: crate::BlockPayloadPtr,
    pub flags: CandidateObservedFlags,
}

/// What [`CandidateController::receive_candidate`] resolved an inbound candidate
/// to, so the thin `SessionProcessor::on_candidate_received` shell can perform
/// the cross-controller fan-out synchronously (in today's exact order) without
/// the controller ever touching another controller.
pub(crate) enum IngressOutcome {
    /// A new candidate was stored (book + cache + DB). The shell performs the
    /// fan-out: before-split insert, observe callback, consensus retry, the
    /// optional notar-cert path, validation registration, and `check_all`.
    Stored {
        raw_candidate: RawCandidate,
        slot: SlotIndex,
        hash: UInt256,
        leader_idx: ValidatorIndex,
        receive_time: SystemTime,
        /// `Some((block_id, before_split))` for non-empty blocks whose
        /// before-split flag parsed; `None` for empties / parse failure.
        before_split: Option<(BlockIdExt, bool)>,
        /// Observe payload for non-empty blocks; `None` for empties.
        observed: Option<CandidateObserved>,
        /// Notar-cert bytes carried by a query response (always `None` for
        /// broadcasts).
        notar_cert: Option<Vec<u8>>,
    },
    /// The candidate body was already known; only the optional notar-cert path
    /// (+ pump) remains for the shell.
    AlreadyKnown { slot: SlotIndex, hash: UInt256, notar_cert: Option<Vec<u8>> },
    /// Dropped by a precheck / dedup / verify gate (telemetry already recorded).
    Rejected,
}

/// Narrow synchronous seam the candidate controller uses to reach the
/// cross-aspect reads/effects that do not live on the controller itself.
///
/// Built fresh from `&mut SessionProcessor` at every re-entry (see
/// `SessionProcessor::with_candidate_backend`) and handed to the controller by
/// `&mut`, so the split-borrow adapter can hold `&mut` references to disjoint
/// processor fields and the controller can drive effects without interior
/// mutability. Intentionally minimal: no `register_for_validation`, no
/// `insert_before_split`, no consensus retry, no `check_all`, and no notar-cert
/// effect — those cross-controller steps stay in the
/// `SessionProcessor::on_candidate_received` shell.
pub(crate) trait CandidateBackend {
    // --- reads ---

    /// Consensus FSM state: far-future / stale-slot prechecks, skip-cert
    /// lookups, and the notarize-certificate presence probe.
    fn simplex_state(&self) -> &SimplexState;

    /// Persistence controller: serving loads (candidate info / payload / notar
    /// bytes) and the in-line payload save during ingress.
    fn database(&self) -> &DatabaseController;

    /// Dedup probe: `true` when the candidate is already tracked by any of the
    /// validation queues (pending-validation / pending-approve / approved /
    /// rejected). The book half of the dedup is owned by the controller.
    fn candidate_known_in_validation(&self, candidate_id: &RawCandidateId) -> bool;

    /// Collation half of the resolver `BlockIdExt → RawCandidateId` lookup
    /// (generated-parent cache). The controller ORs this with its own book.
    fn collation_candidate_id_by_block_id(&self, block_id: &BlockIdExt) -> Option<RawCandidateId>;

    // --- effects ---

    /// Persist candidate metadata (fire-and-forget). The adapter supplies the
    /// session id and telemetry sink.
    fn persist_candidate_info(
        &mut self,
        slot: SlotIndex,
        candidate_hash: &UInt256,
        leader_idx: ValidatorIndex,
        candidate_hash_data_bytes: &[u8],
        signature: Vec<u8>,
    );

    /// Issue a `requestCandidate` to peers (receiver `&self` effect).
    fn request_candidate_from_peers(&self, slot: u32, hash: UInt256);

    /// Cancel in-flight `requestCandidate`s for a slot (receiver `&self` effect).
    fn cancel_candidate_requests_for_slot(&self, slot: u32);
}

impl Controlled for CandidateController {
    type Backend<'b> = dyn CandidateBackend + 'b;
}

/// SXMAIN candidate-phase controller. See the module docs for the full picture.
pub(crate) struct CandidateController {
    /// Shared session description (immutable config): leader / source keys /
    /// shard / self-idx / timing / session id. Same `Arc` the runtime holds.
    description: Arc<SessionDescription>,

    /// Shared per-session telemetry (interior-mutable): ingress counters,
    /// precheck-drop counters, generated-candidate validation notes, the
    /// missing-body log throttle, and the error counter.
    telemetry: Arc<SessionTelemetry>,

    /// In-memory candidate store: received candidates, the TL-bytes
    /// `candidate_data_cache`, and the per-slot broadcast-dedup map.
    book: CandidateBook,

    /// Candidate-request throttle map: `RawCandidateId(slot, hash)` → next
    /// allowed request time. Avoids duplicate `requestCandidate`s and implements
    /// the delayed-request logic (wait for the broadcast before querying peers).
    requested_candidates: HashMap<RawCandidateId, SystemTime>,

    /// Generic deferred-work handle: re-enters `&mut Self` with a fresh backend
    /// view, used by the repair delayed retries.
    queue: ControllerQueuePtr<Self>,

    /// Trace collector for the candidate-received lifecycle event on the
    /// network / query ingress path (`record_candidate_received`,
    /// `is_collator = false`). `None` when stats collection is disabled.
    /// Cheap-to-clone handle cloned from `SessionProcessor` at construction,
    /// mirroring the collation / validation / consensus controllers.
    trace_collector: Option<TraceCollector>,
}

// ======================================================================
// Construction & handles
// ======================================================================
// Build the controller and read its shared session handles and candidate book.
impl CandidateController {
    /// Create a new candidate controller with an empty book / throttle.
    pub(crate) fn new(
        description: Arc<SessionDescription>,
        telemetry: Arc<SessionTelemetry>,
        queue: ControllerQueuePtr<Self>,
        trace_collector: Option<TraceCollector>,
    ) -> Self {
        Self {
            description,
            telemetry,
            book: CandidateBook::new(),
            requested_candidates: HashMap::new(),
            queue,
            trace_collector,
        }
    }

    /* Book accessors (other controllers source the book through these) */

    /// Shared access to the candidate book.
    pub(crate) fn book(&self) -> &CandidateBook {
        &self.book
    }

    /// Mutable access to the candidate book (startup recovery / GC pruning).
    pub(crate) fn book_mut(&mut self) -> &mut CandidateBook {
        &mut self.book
    }

    /* Session identity */

    /// Get session identifier (convenience accessor).
    #[inline]
    fn session_id(&self) -> &SessionId {
        self.description.get_session_id()
    }
}

// ======================================================================
// Ingress (candidate reception)
// ======================================================================
// Inbound candidate reception: precheck -> parse/verify -> dedup -> book +
// cache + DB persist. Returns an `IngressOutcome` for the `SessionProcessor`
// shell to fan out; never touches another controller.
impl CandidateController {
    /// Candidate-domain core of inbound candidate reception.
    ///
    /// Runs the precheck → parse/verify → dedup → book/cache/DB-persist
    /// pipeline and returns an [`IngressOutcome`] describing the cross-controller
    /// fan-out the `SessionProcessor::on_candidate_received` shell must perform.
    /// This method never touches another controller.
    ///
    /// # Arguments
    /// * `source_idx` - Validator index of the sender
    /// * `candidate` - Deserialized candidate data
    /// * `notar_cert` - Serialized notarization certificate signature-set bytes (None for broadcasts)
    ///
    /// Reference: validator-session/src/session_processor.rs process_broadcast()
    /// Reference: C++ block-validator.cpp handle(ValidationRequest)
    pub(crate) fn receive_candidate(
        &mut self,
        source_idx: u32,
        candidate: CandidateData,
        notar_cert: Option<Vec<u8>>,
        b: &mut dyn CandidateBackend,
    ) -> IngressOutcome {
        check_execution_time!(20_000);

        // Extract slot and parent info from CandidateData variant.
        // TL uses i32 for slots; reject negative values at the boundary.
        let (slot, tl_parent_str) = match &candidate {
            CandidateData::Consensus_Block(block) => {
                if block.slot < 0 {
                    log::warn!(
                        "Session {} on_candidate_received: REJECTED - negative slot {} in Block",
                        self.session_id().to_hex_string(),
                        block.slot
                    );
                    return IngressOutcome::Rejected;
                }
                let parent_str = match block.parent.id() {
                    None => "genesis".to_string(),
                    Some(id) => {
                        if *id.slot() < 0 {
                            log::warn!(
                                "Session {} on_candidate_received: REJECTED - \
                                negative parent slot {} in Block",
                                self.session_id().to_hex_string(),
                                id.slot()
                            );
                            return IngressOutcome::Rejected;
                        }
                        format!("s{}:{}", id.slot(), &hex::encode(id.hash().as_slice())[..8])
                    }
                };
                (block.slot as u32, parent_str)
            }
            CandidateData::Consensus_Empty(empty) => {
                if empty.slot < 0 {
                    log::warn!(
                        "Session {} on_candidate_received: REJECTED - negative slot {} in Empty",
                        self.session_id().to_hex_string(),
                        empty.slot
                    );
                    return IngressOutcome::Rejected;
                }
                if *empty.parent.slot() < 0 {
                    log::warn!(
                        "Session {} on_candidate_received: REJECTED - \
                        negative parent slot {} in Empty",
                        self.session_id().to_hex_string(),
                        empty.parent.slot()
                    );
                    return IngressOutcome::Rejected;
                }
                let id_slot = *empty.parent.slot();
                let id_hash = empty.parent.hash();
                let parent_str = format!("s{}:{}", id_slot, &hex::encode(id_hash.as_slice())[..8]);
                (empty.slot as u32, parent_str)
            }
        };

        let sender_idx = ValidatorIndex::new(source_idx);
        let slot = SlotIndex::new(slot);
        let is_broadcast_candidate = notar_cert.is_none();
        let is_local_self_candidate =
            is_broadcast_candidate && sender_idx == self.description.get_self_idx();
        self.record_ingress(sender_idx, is_broadcast_candidate);

        // Reject far-future slots (DoS protection) — before any signature verification
        if b.simplex_state().is_slot_too_far_ahead(slot) {
            if is_broadcast_candidate {
                self.telemetry.candidate_precheck_future_slot_drop_counter.increment(1);
            }
            if is_local_self_candidate {
                self.note_generated_candidate_validation_missed_for_slot(
                    slot,
                    format!(
                        "candidate_precheck_too_far_ahead max_acceptable_slot={}",
                        b.simplex_state().max_acceptable_slot()
                    ),
                );
            }
            log::warn!(
                "Session {} on_candidate_received: REJECTED precheck_drop_reason=too_far_ahead \
                slot={} max={} origin={}",
                &self.session_id().to_hex_string()[..8],
                slot,
                b.simplex_state().max_acceptable_slot(),
                if is_broadcast_candidate { "broadcast" } else { "query" },
            );
            return IngressOutcome::Rejected;
        }

        // Candidate signatures are always created by the slot leader (not by the relay / query responder).
        // For requestCandidate responses, `sender_idx` is the responder, which can differ from the leader.
        let leader_idx = self.description.get_leader(slot);

        if log::log_enabled!(log::Level::Trace) {
            log::trace!(
                "Session {} on_candidate_received: \
                sender_idx={sender_idx}, leader_idx={leader_idx}, \
                slot={slot}, tl_parent={tl_parent_str}",
                &self.session_id().to_hex_string()[..8],
            );
        }

        // 1. Check sender_idx is valid
        if !sender_idx.is_valid(self.description.get_total_nodes()) {
            log::warn!(
                "Session {} on_candidate_received: unknown sender_idx={} (max={})",
                self.session_id().to_hex_string(),
                sender_idx,
                self.description.get_total_nodes()
            );
            return IngressOutcome::Rejected;
        }

        // NOTE: A broadcast candidate (no attached notar cert) is authenticated
        // by the slot leader's signature, which is verified against `leader_key` in
        // `RawCandidate::from_tl(...)` below. The delivering peer (`sender_idx`) may be a
        // relay / gossip hop rather than the leader itself. Dropping a relayed broadcast
        // here strands any node that missed the leader's direct delivery: it can never
        // notarize the slot, is forced to skip, and a single such node is enough to wedge
        // finalization on a notarized slot.
        //
        // C++ parity: overlay broadcasts carry the leader as their signed source (preserved
        // across relays), and `consensus.cpp handle(CandidateReceived)` applies no
        // sender==leader gate; the resolver also serves candidate bodies with no
        // leader-source restriction. We therefore accept relayed broadcasts and let the
        // leader-signature verification below authenticate the body.
        if is_broadcast_candidate && sender_idx != leader_idx {
            self.telemetry.candidate_relayed_broadcast_counter.increment(1);
            log::debug!(
                "Session {} on_candidate_received: received relayed broadcast candidate \
                slot={} leader={} sender={} (leader-signature verification pending below)",
                &self.session_id().to_hex_string()[..8],
                slot,
                leader_idx,
                sender_idx
            );
        }

        // Broadcast path must reject stale slots eagerly to avoid stale-body/db churn.
        let fsm_first_non_finalized_slot = b.simplex_state().get_first_non_finalized_slot();
        if slot < fsm_first_non_finalized_slot {
            if is_broadcast_candidate {
                self.telemetry.candidate_precheck_old_slot_drop_counter.increment(1);
                if is_local_self_candidate {
                    self.note_generated_candidate_validation_missed_for_slot(
                        slot,
                        format!(
                            "candidate_precheck_old_slot first_non_finalized_slot={fsm_first_non_finalized_slot}"
                        ),
                    );
                }
                log::warn!(
                    "Session {} on_candidate_received: REJECTED precheck_drop_reason=old_slot \
                    slot={} first_non_finalized={} origin=broadcast",
                    &self.session_id().to_hex_string()[..8],
                    slot,
                    fsm_first_non_finalized_slot
                );
                return IngressOutcome::Rejected;
            }

            log::trace!(
                "Session {} on_candidate_received: old slot received {} (current={}) origin=query",
                self.session_id().to_hex_string(),
                slot,
                fsm_first_non_finalized_slot,
            );
        }

        // Get leader public key for signature verification
        let leader_key = self.description.get_source_public_key(leader_idx).clone();

        // 2. Create RawCandidate directly from TL (no serialization needed)
        // Note: max_size check is done in receiver
        let max_size =
            self.description.opts().max_block_size + self.description.opts().max_collated_data_size;

        let raw_candidate = match RawCandidate::from_tl(
            &candidate,
            &self.session_id(),
            &leader_key,
            leader_idx,
            self.description.get_shard(),
            max_size,
            self.description.opts().proto_version,
        ) {
            Ok(c) => c,
            Err(e) => {
                if is_local_self_candidate {
                    self.note_generated_candidate_validation_missed_for_slot(
                        slot,
                        format!("candidate_deserialization_failed error={e}"),
                    );
                }
                log::warn!(
                    "Session {} on_candidate_received: failed to deserialize candidate from \
                    sender={}, leader={}, slot={}: {}",
                    self.session_id().to_hex_string(),
                    sender_idx,
                    leader_idx,
                    slot,
                    e
                );
                return IngressOutcome::Rejected;
            }
        };

        // Trace log incoming candidate details for debugging
        log::trace!(
            "Session {} on_candidate_received: parsed candidate {}:{} parent={} leader=v{:03}",
            &self.session_id().to_hex_string()[..8],
            slot,
            &raw_candidate.id.hash.to_hex_string()[..8],
            raw_candidate
                .parent_id
                .as_ref()
                .map(|p| format!("{}:{}", p.slot, &p.hash.to_hex_string()[..8]))
                .unwrap_or_else(|| "genesis".to_string()),
            leader_idx
        );

        // 5. Candidates can be received via relay or requestCandidate (query response),
        // so the sender can differ from the slot leader. The signature is verified against
        // the leader's key above, so a mismatch here is not an error.
        if sender_idx != leader_idx && !is_broadcast_candidate {
            log::trace!(
                "Session {} on_candidate_received: received leader candidate via relay/query: \
                slot={slot} leader={leader_idx} sender={sender_idx}",
                &self.session_id().to_hex_string()[..8],
            );
        }

        // 6. Check if we already have this candidate
        let candidate_id = raw_candidate.id.clone();
        let id_hash = candidate_id.hash.clone();
        debug_assert!(
            candidate_id.slot == slot,
            "RawCandidateId slot mismatch: tl_slot={} raw_candidate.id.slot={}",
            slot,
            candidate_id.slot
        );

        if is_broadcast_candidate {
            match self.book.seen_broadcast(slot).cloned() {
                Some(existing) if existing != candidate_id => {
                    self.telemetry.candidate_precheck_conflicting_slot_drop_counter.increment(1);
                    if is_local_self_candidate {
                        self.note_generated_candidate_validation_missed(
                            &candidate_id,
                            format!(
                                "candidate_precheck_conflicting_slot first_seen_slot_hash={}:{}",
                                existing.slot,
                                &existing.hash.to_hex_string()[..8]
                            ),
                        );
                    }
                    log::warn!(
                        "Session {} on_candidate_received: REJECTED \
                        precheck_drop_reason=conflicting_slot_candidate \
                        slot={} first_seen={:?} new_candidate={:?} origin=broadcast",
                        &self.session_id().to_hex_string()[..8],
                        slot,
                        existing,
                        candidate_id
                    );
                    return IngressOutcome::Rejected;
                }
                Some(_) => {}
                None => {
                    self.book.insert_seen_broadcast(slot, candidate_id.clone());
                }
            }
        }

        // Check if candidate already known.
        // A finalized-boundary stub (seeded by handle_block_finalized with empty data) is NOT
        // "already known" for this purpose -- we want the real body to overwrite it.
        let is_finalized_stub = self
            .book
            .received(&candidate_id)
            .map(|r| r.candidate_hash_data_bytes.is_empty())
            .unwrap_or(false);
        if !is_finalized_stub
            && (b.candidate_known_in_validation(&candidate_id)
                || self.book.contains_received(&candidate_id))
        {
            log::trace!(
                "Session {} on_candidate_received: candidate already known: {:?}",
                self.session_id().to_hex_string(),
                candidate_id,
            );

            // CandidateResolver parity: query responses can carry NotarCert bytes even when we
            // already have the candidate body (e.g., we missed the certificate broadcast).
            // Do NOT drop notar_cert in this case, otherwise the node can get permanently stuck
            // waiting for NotarCert while repeatedly receiving bodies. The shell runs the
            // notar-cert path (and the pump) when present.
            return IngressOutcome::AlreadyKnown { slot, hash: id_hash, notar_cert };
        }

        // 7. Store candidate in received_candidates for finalization (even if not validated)
        // This allows us to accept blocks that are finalized before validation completes
        // Reference: validator-session/src/session_processor.rs set_block_candidate
        let receive_time = self.description.get_time();
        let block_id = raw_candidate.block.block_id();

        // Trace: candidate received from network (also fires for self-loop after own block)
        if let Some(tc) = &self.trace_collector {
            let trace_id = BlockCandidateId {
                slot: raw_candidate.id.slot,
                hash: raw_candidate.id.hash.clone(),
                block: block_id.clone(),
            };
            let trace_parent = raw_candidate.parent_id.as_ref().map(|p| BlockCandidateId {
                slot: p.slot,
                hash: p.hash.clone(),
                block: ton_block::BlockIdExt::default(),
            });
            tc.record_candidate_received(
                self.session_id(),
                &trace_id,
                trace_parent.as_ref(),
                Some(block_id),
                false,
            );
        }

        let root_hash = block_id.root_hash.clone();
        let file_hash = block_id.file_hash.clone();

        // Determine if this is an empty block from the TL variant
        let is_empty = matches!(candidate, CandidateData::Consensus_Empty(_));

        // Cache serialized CandidateData for RequestCandidate query fallback (C++ parity).
        // This provides a secondary in-memory store that persists independently of
        // the receiver's resolver_cache, enabling peers to retrieve candidates even
        // after the resolver_cache is cleaned up.
        match serialize_boxed(&candidate) {
            Ok(bytes) => {
                self.book.insert_cached_data(candidate_id.clone(), bytes.clone());
                // Persist to DB for restart serving (C++ CandidateResolver::store_candidate parity)
                if let Err(e) =
                    b.database().db().save_candidate_payload_async(&candidate_id, &bytes)
                {
                    log::error!(
                        "Session {} on_candidate_received: failed to persist candidate payload: {}",
                        &self.session_id().to_hex_string()[..8],
                        e
                    );
                    self.telemetry.increment_error();
                }
            }
            Err(e) => {
                log::warn!(
                    "Session {} on_candidate_received: failed to serialize CandidateData for cache: {}",
                    &self.session_id().to_hex_string()[..8],
                    e
                );
            }
        }

        // Seqno validation for on_candidate_received
        // Validate seqno is consistent with parent (if parent is already received)
        let received_seqno = block_id.seq_no;
        if let Some(ref parent) = raw_candidate.parent_id {
            if let Some(parent_received) = self.book.received(parent) {
                let parent_seqno = parent_received.block_id.seq_no;
                let expected_seqno = if is_empty { parent_seqno } else { parent_seqno + 1 };

                if received_seqno != expected_seqno {
                    // NOTE: We no longer reject candidates for seqno mismatch at receive time.
                    // The seqno in a candidate is based on the collator's prev_blocks_ids (their chain view),
                    // while the parent slot is from the Simplex FSM. These can legitimately diverge when:
                    // 1. The FSM parent is an older notarized block
                    // 2. The collator's chain has more finalized blocks
                    // Seqno validation is deferred until finalized state is materialized.
                    log::debug!(
                        "Session {} on_candidate_received: seqno differs from parent-based \
                        expectation for slot={slot}, received seqno={received_seqno}, \
                        expected={expected_seqno} (parent_seqno={parent_seqno}, \
                        is_empty={is_empty}). Allowing through - finalized path will resolve it.",
                        &self.session_id().to_hex_string()[..8],
                    );
                }
            }
            // If parent not yet received, we can't validate seqno - allow it through
            // Validation will happen when finalized state is applied.
        } else {
            // No parent (first block in epoch) - seqno is based on the session's initial_block_seqno
            // which may be > 1 if this is not the first session (e.g., after zerostate, seqno=1, but
            // subsequent sessions continue from their start seqno).
            // We don't validate first block seqno at receive time - defer to finalized application.

            // INVARIANT: First block (no parent) cannot be empty
            // Empty blocks inherit parent's BlockIdExt, so they require a parent
            if is_empty {
                if is_local_self_candidate {
                    self.note_generated_candidate_validation_missed(
                        &candidate_id,
                        "first_block_cannot_be_empty",
                    );
                }
                log::warn!(
                    "Session {} on_candidate_received: INVARIANT VIOLATION - first block (slot={}) \
                    cannot be empty (empty blocks require parent). Rejecting.",
                    &self.session_id().to_hex_string()[..8],
                    slot
                );
                return IngressOutcome::Rejected;
            }

            // Genesis-parent candidates at slot > 0 are normal in Simplex: when early
            // slots are skipped, subsequent leaders produce blocks with parent_id=None.
            if slot.value() != 0 {
                log::trace!(
                    "Session {} on_candidate_received: genesis-parent block at slot={} \
                    (early slots were skipped)",
                    &self.session_id().to_hex_string()[..8],
                    slot
                );
            }

            log::debug!(
                "Session {} on_candidate_received: first block (slot={}) has seqno={}",
                &self.session_id().to_hex_string()[..8],
                slot,
                received_seqno
            );
        }

        // Extract actual block data from RawCandidate (not the TL wrapper)
        // This is what validation/finalization callbacks consume.
        let gen_utime_ms = raw_candidate
            .block
            .as_block()
            .and_then(|block| crate::utils::extract_consensus_gen_utime_ms(&block.collated_data));
        let (block_data, collated_data) = match raw_candidate.block.as_block() {
            Some(block) => (
                consensus_common::ConsensusCommonFactory::create_block_payload(block.data.clone()),
                consensus_common::ConsensusCommonFactory::create_block_payload(
                    block.collated_data.clone(),
                ),
            ),
            None => (
                // Empty block - no data
                consensus_common::ConsensusCommonFactory::create_empty_block_payload(),
                consensus_common::ConsensusCommonFactory::create_empty_block_payload(),
            ),
        };
        let observed_data = block_data.clone();
        let observed_collated_data = collated_data.clone();

        let parent_id = raw_candidate.parent_id.clone();

        // Build CandidateHashData TL bytes for signature verification
        // This is the data that was hashed to produce candidate_id_hash
        let candidate_hash_data_bytes = if is_empty {
            // Empty blocks use candidateHashDataEmpty with CandidateId parent
            let Some(parent) = parent_id.as_ref() else {
                if is_local_self_candidate {
                    self.note_generated_candidate_validation_missed(
                        &candidate_id,
                        "empty_candidate_missing_parent",
                    );
                }
                log::error!(
                    "Session {} on_candidate_received: empty block must have parent",
                    &self.session_id().to_hex_string()[..8]
                );
                return IngressOutcome::Rejected;
            };
            crate::utils::build_candidate_hash_data_bytes_empty(
                &block_id,
                (parent.slot, &parent.hash),
            )
        } else {
            // Non-empty blocks use candidateHashDataOrdinary
            let collated_file_hash = match raw_candidate.block.as_block() {
                Some(block) => block.collated_file_hash.clone(),
                None => UInt256::default(),
            };
            let parent_info = parent_id.as_ref().map(|p| (p.slot, &p.hash));
            crate::utils::build_candidate_hash_data_bytes(
                Some(&block_id),
                Some(&collated_file_hash),
                parent_info,
            )
        };

        let parent_metadata_present =
            parent_id.as_ref().is_none_or(|parent| self.book.contains_received(parent));
        log::trace!(
            "Session {} on_candidate_received: slot={} parent={:?} parent_metadata_present={}",
            self.session_id().to_hex_string(),
            slot,
            parent_id.as_ref().map(|p| p.slot),
            parent_metadata_present,
        );

        // Clone data needed for DB save before moving into ReceivedCandidate
        let candidate_hash_data_bytes_for_db = candidate_hash_data_bytes.clone();
        let signature_for_db = raw_candidate.signature.clone();

        self.book.insert_received(
            candidate_id.clone(),
            ReceivedCandidate {
                slot,
                source_idx: leader_idx,
                candidate_hash_data_bytes,
                block_id: block_id.clone(),
                root_hash,
                file_hash,
                data: block_data,
                collated_data,
                gen_utime_ms,
                receive_time,
                is_empty,
                parent_id: parent_id.clone(),
            },
        );

        // Save candidate info to DB (fire-and-forget, matching C++ `.start().detach()` pattern)
        b.persist_candidate_info(
            slot,
            &id_hash,
            leader_idx,
            &candidate_hash_data_bytes_for_db,
            signature_for_db,
        );

        // Remove from requested_candidates if we were waiting for this
        self.requested_candidates.remove(&candidate_id);

        // Non-empty fan-out inputs (before-split flag + observe payload) are
        // computed here and handed to the shell; empties carry neither.
        let mut before_split = None;
        let mut observed = None;
        if !is_empty {
            match crate::utils::extract_before_split_flag(observed_data.data()) {
                Ok(flag) => {
                    before_split = Some((block_id.clone(), flag));
                }
                Err(e) => {
                    log::trace!(
                        "Session {} on_candidate_received: failed to extract before_split flag \
                        for block_id={}: {}",
                        self.session_id().to_hex_string(),
                        block_id,
                        e
                    );
                }
            }

            let observed_flags = CandidateObservedFlags {
                body_present: true,
                parent_ready: b.simplex_state().get_notarize_certificate(slot, &id_hash).is_some(),
                local_collated: is_local_self_candidate,
            };
            observed = Some(CandidateObserved {
                block_id: block_id.clone(),
                data: observed_data,
                collated_data: observed_collated_data,
                flags: observed_flags,
            });
        }

        // DEBUG: Short pattern for quick grep (RECV = candidate received)
        log::debug!(
            "Session {} RECV candidate: slot={slot}, hash={}, seqno={received_seqno}, \
            from=v{:03}, empty={is_empty}, parent_metadata_present={parent_metadata_present}",
            &self.session_id().to_hex_string()[..8],
            &id_hash.to_hex_string()[..8],
            leader_idx,
        );
        // TRACE: Method name pattern for detailed tracking
        log::trace!(
            "Session {} on_candidate_received: slot={slot}, hash={}, seqno={received_seqno}, \
            source={leader_idx}, empty={is_empty}, parent={:?}, parent_metadata_present={parent_metadata_present}",
            self.session_id().to_hex_string(),
            id_hash.to_hex_string(),
            parent_id.as_ref().map(|p| format!("{}:{}", p.slot, p.hash.to_hex_string())),
        );

        // 9. Admit the candidate immediately; check_validation() owns the remaining
        // WaitForParent gate and, for empties, waits until the expected normal tip can be
        // reconstructed from locally known parent metadata.
        if !parent_metadata_present {
            log::debug!(
                "Session {} on_candidate_received: slot={} hash={} is missing parent metadata, \
                but ingress no longer parks candidates behind a simplex-local resolution queue",
                &self.session_id().to_hex_string()[..8],
                slot,
                &id_hash.to_hex_string()[..8],
            );
        }

        IngressOutcome::Stored {
            raw_candidate,
            slot,
            hash: id_hash,
            leader_idx,
            receive_time,
            before_split,
            observed,
            notar_cert,
        }
    }

    /// Record receipt of a candidate. Delegates to the telemetry aspect.
    ///
    /// Keeps ingress counters focused on peer-delivered traffic: locally
    /// generated blocks loop back through ingress but are not network ingress.
    /// The self-index filter lives inside
    /// [`SessionTelemetry::record_candidate_ingress`].
    #[inline]
    fn record_ingress(&self, sender_idx: ValidatorIndex, is_broadcast_candidate: bool) {
        self.telemetry.record_candidate_ingress(
            sender_idx,
            self.description.get_self_idx(),
            is_broadcast_candidate,
        );
    }
}

// ======================================================================
// Outbound repair (request scheduling / resolver availability)
// ======================================================================
// Demand-driven repair: resolve `BlockIdExt -> RawCandidateId`, then schedule
// throttled `requestCandidate`s (delayed retries re-enter through the
// controller's `ControllerQueue`). Owns the `requested_candidates` throttle.
impl CandidateController {
    /* Resolver availability */

    /// Handle a reverse-bridge request from the validator layer to ensure a
    /// candidate body (and optionally its parent chain) is available.
    ///
    /// The validator calls this when collation/validation needs a parent
    /// state that hasn't been applied by the engine yet. Simplex resolves
    /// `BlockIdExt` to the internal `RawCandidateId` and triggers repair.
    ///
    /// Important: a slot may be skipped but still have a notarized candidate.
    /// The repair must handle that case — the candidate body might not have
    /// been received via the normal broadcast path.
    ///
    /// C++ equivalent: demand-driven path in `BlockProducerImpl::produce()`
    /// that triggers `StateResolverImpl::resolve()`.
    pub(crate) fn ensure_candidate_available(
        &mut self,
        block_id: BlockIdExt,
        opts: EnsureCandidateAvailabilityOptions,
        b: &mut dyn CandidateBackend,
    ) {
        self.ensure_candidate_available_impl(block_id, opts, 0, b);
    }

    fn ensure_candidate_available_impl(
        &mut self,
        block_id: BlockIdExt,
        opts: EnsureCandidateAvailabilityOptions,
        attempt: u32,
        b: &mut dyn CandidateBackend,
    ) {
        log::info!(
            target: "simplex_resolver",
            "CandidateController::ensure_candidate_available session_id={} block_id={} \
            purpose={:?} include_parent_chain={} attempt={}/{}",
            self.session_id().to_hex_string(),
            block_id,
            opts.purpose,
            opts.include_parent_chain,
            attempt,
            RESOLVER_AVAILABILITY_MAX_RETRIES,
        );

        let Some(candidate_id) = b
            .collation_candidate_id_by_block_id(&block_id)
            .or_else(|| self.book.find_received_by_block_id(&block_id))
        else {
            if attempt < RESOLVER_AVAILABILITY_MAX_RETRIES {
                let next_attempt = attempt + 1;
                let expiration_time =
                    self.description.get_time() + RESOLVER_AVAILABILITY_RETRY_DELAY;
                log::info!(
                    target: "simplex_resolver",
                    "CandidateController::ensure_candidate_available: unresolved block_id={} \
                    purpose={:?}; scheduling deferred retry {}/{} in {}ms",
                    block_id,
                    opts.purpose,
                    next_attempt,
                    RESOLVER_AVAILABILITY_MAX_RETRIES,
                    RESOLVER_AVAILABILITY_RETRY_DELAY.as_millis(),
                );
                self.queue.post_delayed(expiration_time, move |c, b| {
                    c.ensure_candidate_available_impl(block_id, opts, next_attempt, b);
                });
            } else {
                log::warn!(
                    target: "simplex_resolver",
                    "CandidateController::ensure_candidate_available: unresolved block_id={} \
                    purpose={:?}; exhausted {RESOLVER_AVAILABILITY_MAX_RETRIES} retries, giving up",
                    block_id,
                    opts.purpose,
                );
            }
            return;
        };

        self.request_candidate_body_for_resolver(candidate_id.clone(), b);

        if !opts.include_parent_chain {
            return;
        }

        let mut current = candidate_id;
        let mut depth = 0u32;
        let mut depth_warned = false;
        loop {
            depth += 1;
            if depth > EMPTY_CHAIN_WARN_DEPTH && !depth_warned {
                log::warn!(
                    target: "simplex_resolver",
                    "CandidateController::ensure_candidate_available: deep parent chain depth={} \
                    (warn_threshold={EMPTY_CHAIN_WARN_DEPTH}) for block_id={}; \
                    continuing until hard limit={MAX_CHAIN_DEPTH}",
                    depth,
                    block_id,
                );
                depth_warned = true;
            }
            if depth > MAX_CHAIN_DEPTH {
                log::error!(
                    target: "simplex_resolver",
                    "CandidateController::ensure_candidate_available: exceeded \
                    hard MAX_CHAIN_DEPTH={MAX_CHAIN_DEPTH} while resolving parents for block_id={}",
                    block_id,
                );
                self.telemetry.increment_error();
                break;
            }

            let parent_id = match self
                .book
                .received(&current)
                .and_then(|received| received.parent_id.clone())
            {
                Some(parent_id) => parent_id,
                None => break,
            };

            self.request_candidate_body_for_resolver(parent_id.clone(), b);

            if !self.book.contains_received(&parent_id) {
                log::trace!(
                    target: "simplex_resolver",
                    "CandidateController::ensure_candidate_available: parent metadata missing at \
                    slot={} hash={} while resolving block_id={}; stopping chain traversal",
                    parent_id.slot,
                    &parent_id.hash.to_hex_string()[..8],
                    block_id,
                );
                break;
            }

            current = parent_id;
        }
    }

    /* Request scheduling */

    /// Schedule a candidate request with delay if not already requested.
    ///
    /// Called when we need to repair missing candidate data after learning about a
    /// finalized or otherwise required block before all body/notar data is present.
    /// Adds the (slot, hash) to `requested_candidates` and schedules a delayed action.
    /// After the delay, if the candidate is still not in `received_candidates`, requests
    /// it from peers (with want_notar=true to get NotarCert).
    ///
    /// The delay allows time for the broadcast to arrive naturally before triggering
    /// a P2P query, reducing unnecessary network traffic.
    ///
    /// # Parameters
    /// - `initial_delay`: Optional delay before sending the request.
    ///   - `None`: Use default `CANDIDATE_REQUEST_DELAY` (allows broadcast to arrive first)
    ///   - `Some(Duration::ZERO)`: Request immediately (for repair-critical paths)
    ///   - `Some(dur)`: Custom delay
    pub(crate) fn request_candidate(
        &mut self,
        slot: SlotIndex,
        block_hash: UInt256,
        initial_delay: Option<Duration>,
        b: &mut dyn CandidateBackend,
    ) {
        let delay = initial_delay.unwrap_or(CANDIDATE_REQUEST_DELAY);

        let key = RawCandidateId { slot, hash: block_hash.clone() };

        if b.simplex_state().has_skip_certificate_for_slot(&self.description, slot) {
            log::trace!(
                "Session {} request_candidate: slot={} hash={} - skipped already, not requesting",
                &self.session_id().to_hex_string()[..8],
                slot,
                &block_hash.to_hex_string()[..8],
            );
            self.requested_candidates.remove(&key);
            return;
        }

        // Throttle repeated requests for the same (slot,hash) to survive transient partitions.
        let now = self.description.get_time();
        if let Some(next_allowed_at) = self.requested_candidates.get(&key) {
            if *next_allowed_at > now {
                log::trace!(
                    "Session {} request_candidate: slot={} hash={} - throttled until {:?}",
                    &self.session_id().to_hex_string()[..8],
                    slot,
                    &block_hash.to_hex_string()[..8],
                    next_allowed_at
                );
                return;
            }
        }

        // Check if we already have what we need (stubs don't count as real bodies)
        let have_body = self.book.has_real_body(&key);
        let have_notar = b.simplex_state().get_notarize_certificate(slot, &block_hash).is_some();

        if have_body && have_notar {
            return;
        }

        if delay.is_zero() {
            self.requested_candidates.insert(key.clone(), now + CANDIDATE_REQUEST_RETRY_INTERVAL);

            log::debug!(
                "Session {} request_candidate: requesting slot={slot} hash={} immediately \
                (body={}, notar={})",
                &self.session_id().to_hex_string()[..8],
                &block_hash.to_hex_string()[..8],
                !have_body,
                !have_notar,
            );

            b.request_candidate_from_peers(slot.value(), block_hash);
        } else {
            self.requested_candidates
                .insert(key.clone(), now + delay + CANDIDATE_REQUEST_RETRY_INTERVAL);

            log::trace!(
                "Session {} request_candidate: scheduling request for slot={} hash={} in {:?}",
                &self.session_id().to_hex_string()[..8],
                slot,
                &block_hash.to_hex_string()[..8],
                delay,
            );

            let session_id = self.session_id().clone();
            let expiration_time = now + delay;

            self.queue.post_delayed(expiration_time, move |c, b| {
                let candidate_id = RawCandidateId { slot, hash: block_hash.clone() };
                if !c.requested_candidates.contains_key(&candidate_id) {
                    log::trace!(
                        "Session {} delayed_request_candidate: slot={slot} hash={} \
                        - cancelled before send",
                        &session_id.to_hex_string()[..8],
                        &block_hash.to_hex_string()[..8],
                    );
                    return;
                }
                if b.simplex_state().has_skip_certificate_for_slot(&c.description, slot) {
                    log::trace!(
                        "Session {} delayed_request_candidate: slot={slot} hash={} \
                        - skipped before send",
                        &session_id.to_hex_string()[..8],
                        &block_hash.to_hex_string()[..8],
                    );
                    c.requested_candidates.remove(&candidate_id);
                    return;
                }
                let have_body = c.book.has_real_body(&candidate_id);
                let have_notar =
                    b.simplex_state().get_notarize_certificate(slot, &block_hash).is_some();

                if have_body && have_notar {
                    log::trace!(
                        "Session {} delayed_request_candidate: slot={slot} hash={} - already have \
                        what we need",
                        &session_id.to_hex_string()[..8],
                        &block_hash.to_hex_string()[..8],
                    );
                    return;
                }

                log::debug!(
                    "Session {} delayed_request_candidate: requesting slot={slot} hash={} from \
                    peers (body={}, notar={})",
                    &session_id.to_hex_string()[..8],
                    &block_hash.to_hex_string()[..8],
                    !have_body,
                    !have_notar,
                );

                b.request_candidate_from_peers(slot.value(), block_hash);
                c.requested_candidates.insert(
                    candidate_id,
                    c.description.get_time() + CANDIDATE_REQUEST_RETRY_INTERVAL,
                );
            });
        }
    }

    /// Resolver-driven candidate body request.
    ///
    /// Unlike `request_candidate`, this path is used by validator-side state resolution and
    /// must still request a candidate even when the slot already has a skip certificate.
    /// (A slot can be skipped and still have a notarized block body needed for parent state.)
    fn request_candidate_body_for_resolver(
        &mut self,
        candidate_id: RawCandidateId,
        b: &mut dyn CandidateBackend,
    ) {
        let now = self.description.get_time();

        if self.book.has_real_body(&candidate_id) {
            return;
        }

        if let Some(next_allowed_at) = self.requested_candidates.get(&candidate_id) {
            if *next_allowed_at > now {
                log::trace!(
                    target: "simplex_resolver",
                    "Session {} request_candidate_body_for_resolver: slot={} hash={} \
                    - throttled until {:?}",
                    &self.session_id().to_hex_string()[..8],
                    candidate_id.slot,
                    &candidate_id.hash.to_hex_string()[..8],
                    next_allowed_at,
                );
                return;
            }
        }

        let skipped =
            b.simplex_state().has_skip_certificate_for_slot(&self.description, candidate_id.slot);

        log::debug!(
            target: "simplex_resolver",
            "Session {} request_candidate_body_for_resolver: requesting slot={} hash={} \
            immediately (skipped_slot={})",
            &self.session_id().to_hex_string()[..8],
            candidate_id.slot,
            &candidate_id.hash.to_hex_string()[..8],
            skipped,
        );

        self.requested_candidates
            .insert(candidate_id.clone(), now + CANDIDATE_REQUEST_RETRY_INTERVAL);
        b.request_candidate_from_peers(candidate_id.slot.value(), candidate_id.hash.clone());
    }

    /* Cancellation & GC */

    /// Cancel outbound repair (throttle + receiver) for a slot after it
    /// finalizes / skips.
    pub(crate) fn cancel_repairs_for_slot(
        &mut self,
        slot: SlotIndex,
        b: &mut dyn CandidateBackend,
    ) {
        let before = self.requested_candidates.len();
        self.requested_candidates.retain(|candidate_id, _| candidate_id.slot != slot);
        let removed_requests = before.saturating_sub(self.requested_candidates.len());
        let removed_missing_body = self.telemetry.forget_missing_body_log(slot.value());

        b.cancel_candidate_requests_for_slot(slot.value());

        if removed_requests > 0 || removed_missing_body {
            log::trace!(
                "Session {} cancel_candidate_repairs_for_slot: slot={slot} \
                removed_requests={removed_requests} removed_missing_body={removed_missing_body}",
                &self.session_id().to_hex_string()[..8]
            );
        }
    }

    /// GC hook: drop repair throttle entries for slots below `up_to_slot`.
    pub(crate) fn prune_requested_below(&mut self, up_to_slot: SlotIndex) {
        self.requested_candidates.retain(|id, _| id.slot >= up_to_slot);
    }
}

// ======================================================================
// Serving inbound candidate queries (SXRCV fallback)
// ======================================================================
// SXRCV-side `RequestCandidate` fallback: reconstruct a candidate body /
// notar-cert from the owned book and the database when the receiver
// resolver-cache misses.
impl CandidateController {
    /// Handle RequestCandidate query fallback when receiver's resolver_cache misses.
    ///
    /// Called from SXRCV thread via ReceiverListener when a peer's RequestCandidate query
    /// cannot be fully answered from the in-memory resolver_cache. Attempts to reconstruct
    /// requested candidate body and/or notar parts from:
    ///   1. `candidate_data_cache` (in-memory, fast path)
    ///   2. SimplexDB `CandidateInfoRecord` (empty blocks only -- reconstructed from metadata)
    ///
    /// Non-empty blocks not in the in-memory cache return an empty response; the
    /// querying peer will retry with other validators. This matches C++ behavior
    /// where `CandidateResolver` only loads from its own consensus DB, never from
    /// the validator manager.
    ///
    /// Reference: C++ `CandidateResolver::try_load_candidate_data_from_db()`
    /// TODO: LK: move DB operations to background thread
    pub(crate) fn serve_query_fallback(
        &mut self,
        slot: SlotIndex,
        block_hash: UInt256,
        want_candidate: bool,
        want_notar: bool,
        response_callback: crate::QueryResponseCallback,
        b: &mut dyn CandidateBackend,
    ) {
        check_execution_time!(50_000);

        let candidate_id = RawCandidateId { slot, hash: block_hash.clone() };
        let session_hex = &self.session_id().to_hex_string()[..8];

        // Candidate and notar can be requested independently. Build each part
        // from the best available source and return partials when only one part exists.
        let mut candidate_bytes = Vec::new();

        if want_candidate {
            // 1. Fast path: in-memory candidate_data_cache
            if let Some(bytes) = self.book.cached_data(&candidate_id) {
                log::debug!(
                    "Session {session_hex} candidate_query_fallback: \
                    candidate cache HIT for slot={slot} hash={} ({}B)",
                    &block_hash.to_hex_string()[..8],
                    bytes.len()
                );
                candidate_bytes.clone_from(bytes);
            } else {
                // 2. DB path: candidate metadata
                let candidate_info = b
                    .database()
                    .load_candidate_info_from_db(&candidate_id, self.description.get_session_id());

                // 3. Persisted payload (works for both empty and non-empty blocks)
                const DB_TIMEOUT: Duration = Duration::from_secs(2);
                match b.database().db().load_candidate_payload_by_id(&candidate_id, DB_TIMEOUT) {
                    Ok(Some(payload_bytes)) => {
                        log::debug!(
                            "Session {session_hex} candidate_query_fallback: \
                            loaded payload from DB for slot={slot} ({}B)",
                            payload_bytes.len()
                        );
                        candidate_bytes = payload_bytes;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        log::warn!(
                            "Session {session_hex} candidate_query_fallback: \
                            DB payload load error for slot={slot}: {e}"
                        );
                    }
                }

                // 4. Metadata reconstruction for empty blocks when payload missing.
                if candidate_bytes.is_empty() {
                    if let Some(info) = candidate_info.as_ref() {
                        let is_empty = matches!(
                            info.candidate_hash_data,
                            CandidateHashData::Consensus_CandidateHashDataEmpty(_)
                        );
                        if is_empty {
                            match self
                                .reconstruct_empty_candidate_data_from_info(&candidate_id, info)
                            {
                                Ok(bytes) => {
                                    log::debug!(
                                        "Session {session_hex} candidate_query_fallback: \
                                        reconstructed empty block for slot={slot} ({}B)",
                                        bytes.len()
                                    );
                                    candidate_bytes = bytes;
                                }
                                Err(e) => {
                                    log::warn!(
                                        "Session {session_hex} candidate_query_fallback: \
                                        failed to reconstruct empty block for slot={slot}: {e}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

        let notar_bytes = if want_notar {
            b.database()
                .load_notar_cert_bytes_from_db(&candidate_id, self.description.get_session_id())
        } else {
            Vec::new()
        };

        if candidate_bytes.is_empty() && notar_bytes.is_empty() {
            log::debug!(
                "Session {} candidate_query_fallback: NOT FOUND for slot={} hash={} \
                (want_candidate={}, want_notar={})",
                session_hex,
                slot,
                &block_hash.to_hex_string()[..8],
                want_candidate,
                want_notar,
            );
        } else {
            log::debug!(
                "Session {} candidate_query_fallback: responding slot={} hash={} \
                candidate_bytes={} notar_bytes={}",
                session_hex,
                slot,
                &block_hash.to_hex_string()[..8],
                candidate_bytes.len(),
                notar_bytes.len()
            );
        }

        Self::send_candidate_and_cert_response(candidate_bytes, notar_bytes, response_callback);
    }

    /// Build and send CandidateAndCert response.
    fn send_candidate_and_cert_response(
        candidate_bytes: Vec<u8>,
        notar_bytes: Vec<u8>,
        response_callback: crate::QueryResponseCallback,
    ) {
        use consensus_common::ConsensusCommonFactory;

        let response =
            CandidateAndCert { candidate: candidate_bytes.into(), notar: notar_bytes.into() };

        let result = match serialize_boxed(&response.into_boxed()) {
            Ok(bytes) => Ok(ConsensusCommonFactory::create_block_payload(bytes)),
            Err(e) => Err(error!("Failed to serialize fallback response: {}", e)),
        };
        response_callback(result);
    }

    /// Reconstruct CandidateData::Consensus_Empty bytes from CandidateInfoRecord.
    fn reconstruct_empty_candidate_data_from_info(
        &self,
        candidate_id: &RawCandidateId,
        candidate_info: &CandidateInfoRecord,
    ) -> Result<Vec<u8>> {
        let parent_id = match &candidate_info.candidate_hash_data {
            CandidateHashData::Consensus_CandidateHashDataEmpty(empty) => {
                let slot = SlotIndex(empty.parent.slot as u32);
                let hash = empty.parent.hash.clone();
                (slot, hash)
            }
            _ => return Err(error!("Expected empty hash data")),
        };

        let block_id = if let Some(rc) = self.book.received(candidate_id) {
            rc.block_id.clone()
        } else {
            return Err(error!(
                "Cannot reconstruct empty block: no block_id available for slot={}",
                candidate_id.slot
            ));
        };

        let parent =
            CandidateId { slot: parent_id.0.value() as i32, hash: parent_id.1 }.into_boxed();

        let tl_empty = CandidateDataEmpty {
            slot: candidate_id.slot.value() as i32,
            parent,
            block: block_id,
            signature: candidate_info.signature.clone(),
        };

        let candidate_data = CandidateData::Consensus_Empty(tl_empty);
        serialize_boxed(&candidate_data)
            .map_err(|e| error!("Failed to serialize empty CandidateData: {}", e))
    }
}

// ======================================================================
// Diagnostics & telemetry
// ======================================================================
// Generated-candidate validation observability: notes for locally generated
// candidates that never reached the validation pipeline.
impl CandidateController {
    fn note_generated_candidate_validation_missed(
        &self,
        candidate_id: &RawCandidateId,
        reason: impl Into<String>,
    ) {
        self.telemetry.note_generated_candidate_validation_missed(
            candidate_id,
            reason,
            &self.description,
            self.description.get_time(),
        );
    }

    fn note_generated_candidate_validation_missed_for_slot(
        &self,
        slot: SlotIndex,
        reason: impl Into<String>,
    ) {
        self.telemetry.note_generated_candidate_validation_missed_for_slot(
            slot,
            reason,
            &self.description,
            self.description.get_time(),
        );
    }
}

// ======================================================================
// Tests
// ======================================================================
// Test-only accessors consolidated under one `#[cfg(test)]` impl so the
// production impls carry no test scaffolding.
#[cfg(test)]
impl CandidateController {
    /* Throttle probe (test) */

    /// Read access to the pending-request throttle map (test-only inspection).
    pub(crate) fn requested_candidates(&self) -> &HashMap<RawCandidateId, SystemTime> {
        &self.requested_candidates
    }
}

/*
    Tests live in a sibling file but are included directly via `#[path]` so
    they can reach the private accessor surface without widening visibility.
    Mirrors `collation_controller.rs` / `candidate_book.rs`.
*/

#[cfg(test)]
#[path = "tests/test_candidate_controller.rs"]
mod tests;
