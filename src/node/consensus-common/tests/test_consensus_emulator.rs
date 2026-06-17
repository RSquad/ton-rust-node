/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Integration tests for `ConsensusEmulator`.
//!
//! Verifies the four headline scenarios in the design plan:
//! 1. Local-leader slots fully drive the SessionListener through
//!    `on_generate_slot → on_candidate_observed (×2) → on_block_finalized`
//!    and produce real Ed25519 signatures verifiable against pubkeys.
//! 2. Remote-leader slots drive `on_candidate → on_candidate_observed (×2)
//!    → on_block_finalized` (no `on_generate_slot`).
//! 3. `finalized_delay.skip_probability` actually drops a fraction of
//!    finalized callbacks.
//! 4. `set_params(...)` mutates cadence at runtime.

use consensus_common::{
    AsyncRequestPtr, BlockHash, BlockPayloadPtr, BlockSourceInfo, CandidateObservedFlags,
    CollationParentHint, ConsensusCommonFactory, EmulatorDelaySpec, EmulatorLeaderRotation,
    EmulatorOptions, EmulatorParams, EmulatorPtr, EmulatorSignerSubset, PublicKey, PublicKeyHash,
    SessionId, SessionListener, SessionListenerPtr, SessionNode, ValidatorBlockCandidate,
    ValidatorBlockCandidateCallback, ValidatorBlockCandidateDecisionCallback,
};
use std::{
    collections::HashSet,
    io::Cursor,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant, SystemTime},
};
use ton_api::{
    ton::consensus::{
        candidatehashdata::CandidateHashDataOrdinary, candidateid::CandidateId,
        candidateparent::CandidateParent, datatosign::DataToSign,
        simplex::unsignedvote::FinalizeVote, CandidateHashData as CandidateHashDataBoxed,
        CandidateParent as CandidateParentBoxed,
    },
    Deserializer, IntoBoxed,
};
use ton_block::{
    sha256_digest, BlockIdExt, BlockSignaturesVariant, Ed25519KeyOption, ShardIdent, UInt256,
};

// ============================================================================
// Recorder: a SessionListener that captures events for assertions
// ============================================================================

#[derive(Debug, Clone)]
#[allow(dead_code)] // some fields are used only for debug printout on assertion failure
enum RecordedEvent {
    GenerateSlot {
        source: PublicKeyHash,
    },
    Candidate {
        source: PublicKeyHash,
        root_hash: BlockHash,
    },
    Observed {
        block_id: BlockIdExt,
        flags: CandidateObservedFlags,
    },
    Finalized {
        block_id: BlockIdExt,
        slot_from_variant: u32,
        parent: Option<(u32, UInt256)>,
        candidate_hash: UInt256,
        sig_count: usize,
        all_sigs_valid: bool,
        source: PublicKeyHash,
    },
}

struct Recorder {
    events: Mutex<Vec<RecordedEvent>>,
    /// Counter used to fabricate distinct BlockIdExt for synthetic local
    /// candidates returned to `on_generate_slot`.
    next_seqno: Mutex<u32>,
    session_id: SessionId,
    shard: ShardIdent,
    validators: Mutex<Vec<SessionNode>>,
    generate_delay: Mutex<Duration>,
    /// If `Some(p)`, the recorder rejects each `on_candidate` with that
    /// probability (via a deterministic counter — every Nth call rejected).
    /// Used by the validation test to exercise the reject path.
    reject_every: Mutex<Option<u32>>,
    candidate_counter: Mutex<u32>,
}

impl Recorder {
    fn new(session_id: SessionId, shard: ShardIdent, validators: Vec<SessionNode>) -> Arc<Self> {
        Arc::new(Self {
            events: Mutex::new(Vec::new()),
            next_seqno: Mutex::new(1),
            session_id,
            shard,
            validators: Mutex::new(validators),
            generate_delay: Mutex::new(Duration::ZERO),
            reject_every: Mutex::new(None),
            candidate_counter: Mutex::new(0),
        })
    }

    fn snapshot(&self) -> Vec<RecordedEvent> {
        self.events.lock().unwrap().clone()
    }

    fn count_event<F: Fn(&RecordedEvent) -> bool>(&self, pred: F) -> usize {
        self.snapshot().iter().filter(|e| pred(e)).count()
    }

    fn set_reject_every(&self, n: Option<u32>) {
        *self.reject_every.lock().unwrap() = n;
    }

    fn set_generate_delay(&self, delay: Duration) {
        *self.generate_delay.lock().unwrap() = delay;
    }
}

impl SessionListener for Recorder {
    fn on_candidate(
        &self,
        source_info: BlockSourceInfo,
        root_hash: BlockHash,
        _data: BlockPayloadPtr,
        _collated_data: BlockPayloadPtr,
        callback: ValidatorBlockCandidateDecisionCallback,
    ) {
        self.events
            .lock()
            .unwrap()
            .push(RecordedEvent::Candidate { source: source_info.source.id().clone(), root_hash });
        let mut c = self.candidate_counter.lock().unwrap();
        *c = c.wrapping_add(1);
        let counter = *c;
        let reject =
            self.reject_every.lock().unwrap().is_some_and(|n| n != 0 && counter.is_multiple_of(n));
        if reject {
            callback(Err(ton_block::error!("recorder: synthetic reject")));
        } else {
            callback(Ok(SystemTime::now()));
        }
    }

    fn on_generate_slot(
        &self,
        source_info: BlockSourceInfo,
        _request: AsyncRequestPtr,
        _parent: CollationParentHint,
        callback: ValidatorBlockCandidateCallback,
    ) {
        self.events
            .lock()
            .unwrap()
            .push(RecordedEvent::GenerateSlot { source: source_info.source.id().clone() });
        let delay = *self.generate_delay.lock().unwrap();
        if !delay.is_zero() {
            thread::sleep(delay);
        }

        // Build a deterministic synthetic candidate. The emulator extracts
        // `id` and uses it for downstream callbacks; the file_hash and
        // collated_file_hash are zero so the test can recompute the
        // candidate hash for verification.
        let mut s = self.next_seqno.lock().unwrap();
        let seq_no = *s;
        *s = s.wrapping_add(1);
        drop(s);

        let mut buf = Vec::with_capacity(32 + 4);
        buf.extend_from_slice(self.session_id.as_slice());
        buf.extend_from_slice(&seq_no.to_be_bytes());
        let root = UInt256::from_slice(&sha256_digest(&buf));

        let block_id = BlockIdExt {
            shard_id: self.shard.clone(),
            seq_no,
            root_hash: root,
            file_hash: UInt256::default(),
        };

        let candidate = ValidatorBlockCandidate {
            public_key: source_info.source.clone(),
            id: block_id,
            collated_file_hash: UInt256::default(),
            data: ConsensusCommonFactory::create_empty_block_payload(),
            collated_data: ConsensusCommonFactory::create_empty_block_payload(),
        };
        callback(Ok(Arc::new(candidate)));
    }

    // ---- catchain-only callbacks (must compile but never fire) ----
    fn on_block_committed(
        &self,
        _source_info: BlockSourceInfo,
        _root_hash: BlockHash,
        _file_hash: BlockHash,
        _data: BlockPayloadPtr,
        _signatures: BlockSignaturesVariant,
        _approve_signatures: Vec<(PublicKeyHash, BlockPayloadPtr)>,
        _stats: consensus_common::SessionStats,
    ) {
        unreachable!("emulator should not fire on_block_committed (Simplex semantics)");
    }

    fn on_block_skipped(&self, _round: u32) {
        unreachable!("emulator should not fire on_block_skipped (Simplex semantics)");
    }

    fn get_approved_candidate(
        &self,
        _source: PublicKey,
        _root_hash: BlockHash,
        _file_hash: BlockHash,
        _collated_data_hash: BlockHash,
        _callback: ValidatorBlockCandidateCallback,
    ) {
        unreachable!("emulator should not fire get_approved_candidate (Simplex semantics)");
    }

    // ---- Simplex callbacks ----
    fn on_candidate_observed(
        &self,
        block_id: BlockIdExt,
        _data: BlockPayloadPtr,
        _collated_data: BlockPayloadPtr,
        flags: CandidateObservedFlags,
    ) {
        self.events.lock().unwrap().push(RecordedEvent::Observed { block_id, flags });
    }

    fn on_block_finalized(
        &self,
        block_id: BlockIdExt,
        source_info: BlockSourceInfo,
        _root_hash: BlockHash,
        _file_hash: BlockHash,
        _data: BlockPayloadPtr,
        signatures: BlockSignaturesVariant,
        approve_signatures: Vec<(PublicKeyHash, BlockPayloadPtr)>,
    ) {
        let validators = self.validators.lock().unwrap().clone();
        let (slot_from_variant, candidate_data_bytes) = match &signatures {
            BlockSignaturesVariant::Simplex(s) => {
                (s.slot, s.candidate_data_bytes().expect("candidate data bytes"))
            }
            _ => panic!("emulator must produce Simplex signatures"),
        };
        let candidate_hash = UInt256::from_slice(&sha256_digest(&candidate_data_bytes));
        let parent = extract_parent_from_candidate_data(&candidate_data_bytes);
        let all_sigs_valid = approve_signatures.iter().all(|(node_id, sig_payload)| {
            validators
                .iter()
                .find(|v| v.public_key.id() == node_id)
                .map(|v| {
                    verify_candidate_signature(
                        &self.session_id,
                        slot_from_variant,
                        &candidate_hash,
                        sig_payload.data(),
                        &v.public_key,
                    )
                })
                .unwrap_or(false)
        });
        self.events.lock().unwrap().push(RecordedEvent::Finalized {
            block_id,
            slot_from_variant,
            parent,
            candidate_hash,
            sig_count: approve_signatures.len(),
            all_sigs_valid,
            source: source_info.source.id().clone(),
        });
    }
}

// ============================================================================
// Signature verification helpers (independent of emulator internals)
// ============================================================================

fn compute_candidate_hash(
    block_id: &BlockIdExt,
    collated_file_hash: &UInt256,
    parent: Option<(u32, &UInt256)>,
) -> UInt256 {
    let parent_tl = match parent {
        Some((slot, hash)) => {
            let id = CandidateId { slot: slot as i32, hash: hash.clone() };
            CandidateParent { id: id.into_boxed() }.into_boxed()
        }
        None => CandidateParentBoxed::Consensus_CandidateWithoutParents,
    };
    let chd = CandidateHashDataOrdinary {
        block: block_id.clone(),
        collated_file_hash: collated_file_hash.clone(),
        parent: parent_tl,
    };
    let bytes = consensus_common::serialize_tl_boxed_object!(&chd.into_boxed());
    UInt256::from_slice(&sha256_digest(&bytes))
}

fn extract_parent_from_candidate_data(candidate_data_bytes: &[u8]) -> Option<(u32, UInt256)> {
    let candidate = Deserializer::new(&mut Cursor::new(candidate_data_bytes))
        .read_boxed::<CandidateHashDataBoxed>()
        .expect("deserialize candidate hash data");
    match candidate {
        CandidateHashDataBoxed::Consensus_CandidateHashDataOrdinary(ordinary) => {
            ordinary.parent.id().map(|id| (*id.slot() as u32, id.hash().clone()))
        }
        CandidateHashDataBoxed::Consensus_CandidateHashDataEmpty(empty) => {
            Some((empty.parent.slot as u32, empty.parent.hash))
        }
    }
}

fn verify_candidate_signature(
    session_id: &SessionId,
    slot: u32,
    candidate_hash: &UInt256,
    signature: &[u8],
    public_key: &PublicKey,
) -> bool {
    let cid = CandidateId { slot: slot as i32, hash: candidate_hash.clone() };
    let inner =
        consensus_common::serialize_tl_boxed_object!(
            &FinalizeVote { id: cid.into_boxed() }.into_boxed()
        );
    let dts = DataToSign { session_id: session_id.clone(), data: inner };
    let to_verify = consensus_common::serialize_tl_boxed_object!(&dts.into_boxed());
    public_key.verify(&to_verify, signature).is_ok()
}

// ============================================================================
// Setup helpers
// ============================================================================

const N_VALIDATORS: usize = 4;

fn build_validators(n: usize) -> Vec<SessionNode> {
    (0..n)
        .map(|_| {
            let key = Ed25519KeyOption::<ton_block::ZeroizingBytes>::generate()
                .expect("generate ed25519");
            SessionNode { adnl_id: key.id().clone(), public_key: key, weight: 1 }
        })
        .collect()
}

fn build_emulator_with(
    opts: EmulatorOptions,
    listener: &Arc<Recorder>,
    validators: Vec<SessionNode>,
    local_idx: usize,
) -> EmulatorPtr {
    let listener_ptr: SessionListenerPtr =
        Arc::downgrade(&(listener.clone() as Arc<dyn SessionListener + Send + Sync>));
    ConsensusCommonFactory::create_consensus_emulator(opts, validators, local_idx, listener_ptr)
        .expect("create_consensus_emulator")
}

fn make_session_id(tag: u8) -> SessionId {
    let mut bytes = [0u8; 32];
    bytes[0] = tag;
    UInt256::from_slice(&bytes)
}

/*
===================================================================================================
    Simplex-case emulator coverage plan

    This file should not port every `node/simplex` test. The simplex crate
    already owns low-level FSM, certificate, receiver, database, resolver, and
    real-overlay coverage. The emulator tests should instead prove that the
    in-process test double can be configured to exercise the same higher-level
    `SessionListener` behaviours that ValidatorGroup relies on.

    Source simplex tests reviewed:
      - `tests/test_collation.rs::test_single_node_collation`
      - `tests/test_validation.rs::test_two_node_validation`
      - `tests/test_consensus.rs::{
            test_simplex_consensus_basic,
            test_simplex_consensus_with_failures,
            test_simplex_consensus_finalcert_recovery,
            test_simplex_consensus_shard_with_mc_notifications,
            test_simplex_start_gate,
            test_simplex_consensus_candidate_chaining,
            test_simplex_consensus_candidate_chaining_with_lossy_overlay,
            test_simplex_consensus_ghost_parent_resolver_probe
        }`
      - `tests/test_restart.rs::test_single_session_restart_round_monotonicity_first_commit_after_finalized`

    Planned emulator-level scenarios and requirements:

    1. Basic liveness / callback pipeline
       Requirements:
         - Fixed-local run fires `on_generate_slot`, then two
           `on_candidate_observed` callbacks (`parent_ready=false`, then
           `parent_ready=true`), then `on_block_finalized`.
         - Fixed-remote run fires `on_candidate` instead of `on_generate_slot`,
           then the same observed/finalized path.
         - No legacy catchain callbacks (`on_block_committed`,
           `on_block_skipped`, `get_approved_candidate`) fire.
         - `BlockSourceInfo.priority.round == u32::MAX` for Simplex semantics.
         - Every finalized block carries a `BlockSignaturesVariant::Simplex`
           with Ed25519 signatures verifiable against the configured validators.
       Current coverage:
         - `test_emulator_local_leader_finalization`
         - `test_emulator_remote_leader_validation`

    2. Round-robin normal consensus
       Requirements:
         - Configure `EmulatorLeaderRotation::RoundRobin` with all validators
           signing and zero loss.
         - Run long enough for every validator to lead several finalized slots.
         - For each finalized event, verify `leader == validators[slot % n]`.
         - Verify local slots create collations and remote slots create
           validations; finalized count should grow steadily.
       Current coverage:
         - `test_emulator_round_robin_normal_load`

    3. High-load / lossy callback scheduling
       Requirements:
         - Configure long `on_candidate_delay` to keep multiple remote
           validations in flight.
         - Configure skip probability on observed-notar/finalized callbacks to
           simulate missing validation/final-certificate delivery.
         - Configure wide finalized-delay variance and assert finalized slots
           can arrive out of order while signatures remain valid.
         - Commit/finalized rate should stay above a low threshold when signer
           weight remains >= 2/3.
       Current coverage:
         - `test_emulator_skip_probability`
         - `test_emulator_high_load_with_skips_and_reordering`
       Follow-up (if needed):
         - Add a recorder knob that rejects every Nth `on_candidate` decision
           to mirror `candidate_rejection_probability` in
           `test_simplex_consensus_with_failures`.

    4. Runtime stalls and recovery
       Requirements:
         - Start healthy, then mutate `EmulatorParams` during work so
           finalization delivery stalls (`finalized_delay.skip_probability=1`).
         - Confirm slot generation/validation can continue while finalized
           callbacks plateau.
         - Restore healthy params and assert finalized callbacks resume without
           late-thread deadlocks or invalid signatures.
       Current coverage:
         - `test_emulator_runtime_param_change`
         - `test_emulator_stall_via_param_change`

    5. Start gate and synchronous shutdown
       Requirements:
         - Construct the emulator but do not call `start`; no listener callback
           may fire before the start gate is released.
         - After `start`, callbacks must begin promptly.
         - `stop()` must synchronously wait for the main and callback queues to
           drain; no listener events may fire after it returns.
         - `stop_async()` must only request shutdown and return promptly; a
           following `stop()` must provide the synchronous wait.
       Current coverage:
         - `test_emulator_stop_is_synchronous`
         - `test_emulator_stop_async_then_sync`
         - `test_emulator_start_gate_no_callbacks_before_start`

    6. Candidate chaining / multi-slot leader windows
       Requirements:
         - Mirror `test_simplex_consensus_candidate_chaining` at emulator level:
           support a leader holding several consecutive slots and verify seqnos
           advance through the window instead of repeating the first parent.
         - If the emulator is extended with `slots_per_leader_window`, assert
           local leader windows produce multiple collations and finalized
           events before rotation.
         - If `CollationParentHint::Explicit` is added to the emulator path,
           assert the hints reference the previous notarized/finalized block
           inside the window.
       Current coverage:
         - `test_emulator_parent_chain_across_leader_windows`
         - `test_emulator_parent_chain_skips_rejected_candidates`

    7. Resolver-observation behaviour
       Requirements:
         - Mirror only the listener-visible part of
           `test_simplex_consensus_ghost_parent_resolver_probe`: observed
           callbacks should include body-present and not-yet-parent-ready
           events before the parent-ready notification.
         - A test listener may count `!parent_ready` observations as synthetic
           "resolver demand"; full `ensure_candidate_available` remains owned
           by simplex integration tests.
       Current coverage:
         - Partially covered by parent_ready=true/false assertions in
           `test_emulator_local_leader_finalization`.
         - `test_emulator_resolver_observation_parent_ready_sequence`

    8. Restart / monotonicity
       Requirements:
         - The emulator has no DB, so do not port restart-gremlin tests.
         - Keep only session-contract checks: stop is synchronous, repeated
           stop is idempotent, and a fresh emulator created with
           `initial_seqno = last_finalized_seqno + 1` finalizes monotonically.
       Current coverage:
         - Stop contract is covered.
         - `test_emulator_fresh_session_initial_seqno_monotonic_after_stop`

    9. Out of scope for consensus-common emulator tests
       - ADNL overlay and net-gremlin networking (`test_simplex_consensus_adnl_*`).
       - Persistent DB restart/recovery and FinalCert repair internals.
       - Simplex FSM misbehavior, certificate thresholds, receiver slot bounds,
         and database retention; those remain in `node/simplex/src/tests`.
===================================================================================================
*/

// ============================================================================
// Test 1: local-leader finalization
// ============================================================================

#[test]
fn test_emulator_local_leader_finalization() {
    let session_id = make_session_id(0x01);
    let shard = ShardIdent::default();
    let validators = build_validators(N_VALIDATORS);
    let listener = Recorder::new(session_id.clone(), shard.clone(), validators.clone());

    let mut opts = EmulatorOptions::defaults_for_test(session_id.clone(), shard);
    opts.leader_rotation = EmulatorLeaderRotation::FixedLocal;
    opts.signer_subset = EmulatorSignerSubset::All;
    opts.deterministic_seed = Some(0xC0FFEE_u64);
    opts.initial_params = EmulatorParams {
        slot_interval: EmulatorDelaySpec::new(20, 30, 0.0),
        on_candidate_delay: EmulatorDelaySpec::ZERO,
        observed_body_delay: EmulatorDelaySpec::new(2, 5, 0.0),
        observed_notar_delay: EmulatorDelaySpec::new(8, 12, 0.0),
        finalized_delay: EmulatorDelaySpec::new(15, 25, 0.0),
    };

    let emu = build_emulator_with(opts, &listener, validators.clone(), 0);
    emu.start(vec![], BlockIdExt::default());
    thread::sleep(Duration::from_millis(800));
    emu.stop();

    let events = listener.snapshot();
    let generate_count =
        events.iter().filter(|e| matches!(e, RecordedEvent::GenerateSlot { .. })).count();
    let candidate_count =
        events.iter().filter(|e| matches!(e, RecordedEvent::Candidate { .. })).count();
    let observed_count =
        events.iter().filter(|e| matches!(e, RecordedEvent::Observed { .. })).count();
    let finalized_count =
        events.iter().filter(|e| matches!(e, RecordedEvent::Finalized { .. })).count();

    assert!(
        generate_count >= 5,
        "expected ≥5 generate_slot calls, got {generate_count}; events={events:?}"
    );
    assert_eq!(candidate_count, 0, "local-only run should not fire on_candidate");
    // Each generated slot produces 2 observed callbacks (parent_ready false + true).
    assert!(
        observed_count >= 2 * finalized_count,
        "expected ≥2x observed callbacks vs finalized; obs={observed_count} fin={finalized_count}"
    );
    assert!(finalized_count >= 4, "expected at least 4 finalized blocks, got {finalized_count}");

    // Verify per-finalized: exactly N validator signatures and all valid.
    for ev in &events {
        if let RecordedEvent::Finalized { sig_count, all_sigs_valid, .. } = ev {
            assert_eq!(*sig_count, validators.len(), "expected {} sigs", validators.len());
            assert!(*all_sigs_valid, "all per-validator signatures must verify");
        }
    }

    // parent_ready flag should appear in both true and false forms.
    let observed_true = events
        .iter()
        .filter(|e| matches!(e, RecordedEvent::Observed { flags, .. } if flags.parent_ready))
        .count();
    let observed_false = events
        .iter()
        .filter(|e| matches!(e, RecordedEvent::Observed { flags, .. } if !flags.parent_ready))
        .count();
    assert!(observed_true > 0 && observed_false > 0, "both parent_ready flags should occur");
}

// ============================================================================
// Test 2: remote-leader candidate validation
// ============================================================================

#[test]
fn test_emulator_remote_leader_validation() {
    let session_id = make_session_id(0x02);
    let shard = ShardIdent::default();
    let validators = build_validators(N_VALIDATORS);
    let listener = Recorder::new(session_id.clone(), shard.clone(), validators.clone());

    // Local at idx 0; leader fixed at idx 2 — local never leads.
    let mut opts = EmulatorOptions::defaults_for_test(session_id, shard);
    opts.leader_rotation = EmulatorLeaderRotation::FixedIndex(2);
    opts.signer_subset = EmulatorSignerSubset::All;
    opts.deterministic_seed = Some(0xBADC0DE_u64);
    opts.initial_params = EmulatorParams {
        slot_interval: EmulatorDelaySpec::new(20, 30, 0.0),
        on_candidate_delay: EmulatorDelaySpec::new(3, 8, 0.0),
        observed_body_delay: EmulatorDelaySpec::new(3, 6, 0.0),
        observed_notar_delay: EmulatorDelaySpec::new(8, 12, 0.0),
        finalized_delay: EmulatorDelaySpec::new(20, 30, 0.0),
    };

    let emu = build_emulator_with(opts, &listener, validators, 0);
    emu.start(vec![], BlockIdExt::default());
    thread::sleep(Duration::from_millis(800));
    emu.stop();

    let events = listener.snapshot();
    let generate_count =
        events.iter().filter(|e| matches!(e, RecordedEvent::GenerateSlot { .. })).count();
    let candidate_count =
        events.iter().filter(|e| matches!(e, RecordedEvent::Candidate { .. })).count();
    let finalized_count =
        events.iter().filter(|e| matches!(e, RecordedEvent::Finalized { .. })).count();

    assert_eq!(generate_count, 0, "local should never lead in this test");
    assert!(candidate_count >= 5, "expected ≥5 on_candidate calls, got {candidate_count}");
    assert!(finalized_count >= 4, "expected ≥4 finalized, got {finalized_count}");

    // For each finalized event, sigs must verify.
    for ev in &events {
        if let RecordedEvent::Finalized { all_sigs_valid, .. } = ev {
            assert!(*all_sigs_valid, "remote-leader sigs must verify");
        }
    }
}

// ============================================================================
// Test 3: finalized skip probability
// ============================================================================

#[test]
fn test_emulator_skip_probability() {
    let session_id = make_session_id(0x03);
    let shard = ShardIdent::default();
    let validators = build_validators(N_VALIDATORS);
    let listener = Recorder::new(session_id.clone(), shard.clone(), validators.clone());

    let mut opts = EmulatorOptions::defaults_for_test(session_id, shard);
    opts.leader_rotation = EmulatorLeaderRotation::FixedLocal;
    opts.signer_subset = EmulatorSignerSubset::All;
    opts.deterministic_seed = Some(0xDEADBEEF_u64);
    opts.initial_params = EmulatorParams {
        slot_interval: EmulatorDelaySpec::new(5, 7, 0.0),
        on_candidate_delay: EmulatorDelaySpec::ZERO,
        observed_body_delay: EmulatorDelaySpec::ZERO,
        observed_notar_delay: EmulatorDelaySpec::ZERO,
        // 50% finalized callbacks dropped.
        finalized_delay: EmulatorDelaySpec::new(0, 1, 0.5),
    };

    let emu = build_emulator_with(opts, &listener, validators, 0);
    emu.start(vec![], BlockIdExt::default());
    // Run long enough to accumulate hundreds of slots so the binomial CI is
    // tight even with deterministic-seed-driven micro-correlations.
    thread::sleep(Duration::from_millis(1500));
    emu.stop();
    // Count after stop so the queue-drain finishes; otherwise events posted
    // shortly before stop would be lost from the snapshot.

    let generate_count = listener.count_event(|e| matches!(e, RecordedEvent::GenerateSlot { .. }));
    let finalized_count = listener.count_event(|e| matches!(e, RecordedEvent::Finalized { .. }));

    assert!(generate_count >= 100, "need many slots to average; got {generate_count}");
    let ratio = finalized_count as f64 / generate_count as f64;
    assert!(
        (0.20..=0.80).contains(&ratio),
        "expected ~50% finalize rate, got {ratio:.2} ({finalized_count}/{generate_count})"
    );
}

// ============================================================================
// Test 4: runtime parameter change
// ============================================================================

#[test]
fn test_emulator_runtime_param_change() {
    let session_id = make_session_id(0x04);
    let shard = ShardIdent::default();
    let validators = build_validators(N_VALIDATORS);
    let listener = Recorder::new(session_id.clone(), shard.clone(), validators.clone());

    let mut opts = EmulatorOptions::defaults_for_test(session_id, shard);
    opts.leader_rotation = EmulatorLeaderRotation::FixedLocal;
    opts.signer_subset = EmulatorSignerSubset::All;
    opts.deterministic_seed = Some(0x12345_u64);
    opts.initial_params = EmulatorParams {
        slot_interval: EmulatorDelaySpec::new(80, 100, 0.0),
        on_candidate_delay: EmulatorDelaySpec::ZERO,
        observed_body_delay: EmulatorDelaySpec::ZERO,
        observed_notar_delay: EmulatorDelaySpec::ZERO,
        finalized_delay: EmulatorDelaySpec::ZERO,
    };

    let emu = build_emulator_with(opts, &listener, validators, 0);
    emu.start(vec![], BlockIdExt::default());

    // First half: slow cadence (~80-100 ms / slot).
    let phase = Duration::from_millis(800);
    thread::sleep(phase);
    // Snapshot the first-half count *after* a brief drain so the cb-thread
    // has caught up with main's posts.
    thread::sleep(Duration::from_millis(50));
    let first_half_slots =
        listener.count_event(|e| matches!(e, RecordedEvent::GenerateSlot { .. }));

    // Switch to a 4x faster cadence at runtime.
    let mut new_params = emu.get_params();
    new_params.slot_interval = EmulatorDelaySpec::new(15, 25, 0.0);
    emu.set_params(new_params).expect("set fast params");

    let t0 = Instant::now();
    thread::sleep(phase);
    let elapsed = t0.elapsed();
    emu.stop();
    // Count after stop so all events are flushed.
    let total_slots = listener.count_event(|e| matches!(e, RecordedEvent::GenerateSlot { .. }));
    let second_half_slots = total_slots - first_half_slots;

    assert!(elapsed >= phase, "second-phase sleep was too short: {elapsed:?}");
    assert!(first_half_slots >= 5, "first half too few slots: {first_half_slots}");
    // Expect roughly 4x more slots in the second half. Use a loose threshold
    // (≥2x) to absorb the in-flight slot scheduled before set_params.
    assert!(
        second_half_slots >= first_half_slots * 2,
        "second half should be much faster: first={first_half_slots} second={second_half_slots}"
    );
}

// ============================================================================
// Construction-time validation
// ============================================================================

#[test]
fn test_emulator_rejects_below_threshold_signer_subset() {
    let session_id = make_session_id(0xFE);
    let shard = ShardIdent::default();
    let validators = build_validators(4);
    let listener = Recorder::new(session_id.clone(), shard.clone(), validators.clone());

    let mut opts = EmulatorOptions::defaults_for_test(session_id, shard);
    // 2-of-4 = 50% weight, below threshold_66 (= 3 of 4).
    opts.signer_subset = EmulatorSignerSubset::First(2);

    let listener_ptr: SessionListenerPtr =
        Arc::downgrade(&(listener.clone() as Arc<dyn SessionListener + Send + Sync>));
    let result =
        ConsensusCommonFactory::create_consensus_emulator(opts, validators, 0, listener_ptr);
    assert!(result.is_err(), "expected create_consensus_emulator to reject below-threshold subset");
}

#[test]
fn test_emulator_rejects_invalid_local_idx() {
    let session_id = make_session_id(0xFD);
    let shard = ShardIdent::default();
    let validators = build_validators(3);
    let listener = Recorder::new(session_id.clone(), shard.clone(), validators.clone());
    let opts = EmulatorOptions::defaults_for_test(session_id, shard);
    let listener_ptr: SessionListenerPtr =
        Arc::downgrade(&(listener.clone() as Arc<dyn SessionListener + Send + Sync>));
    let result =
        ConsensusCommonFactory::create_consensus_emulator(opts, validators, 99, listener_ptr);
    assert!(result.is_err(), "expected create_consensus_emulator to reject local_idx out of range");
}

#[test]
fn test_emulator_unique_block_ids_local() {
    // Sanity check: local-leader candidates should produce unique block_ids.
    let session_id = make_session_id(0x05);
    let shard = ShardIdent::default();
    let validators = build_validators(N_VALIDATORS);
    let listener = Recorder::new(session_id.clone(), shard.clone(), validators.clone());

    let mut opts = EmulatorOptions::defaults_for_test(session_id, shard);
    opts.leader_rotation = EmulatorLeaderRotation::FixedLocal;
    opts.signer_subset = EmulatorSignerSubset::All;
    opts.deterministic_seed = Some(0xABCDEF_u64);
    opts.initial_params = EmulatorParams {
        slot_interval: EmulatorDelaySpec::new(10, 20, 0.0),
        on_candidate_delay: EmulatorDelaySpec::ZERO,
        observed_body_delay: EmulatorDelaySpec::ZERO,
        observed_notar_delay: EmulatorDelaySpec::ZERO,
        finalized_delay: EmulatorDelaySpec::ZERO,
    };

    let emu = build_emulator_with(opts, &listener, validators, 0);
    emu.start(vec![], BlockIdExt::default());
    thread::sleep(Duration::from_millis(400));
    emu.stop();

    let events = listener.snapshot();
    let mut seen: HashSet<BlockIdExt> = HashSet::new();
    for ev in events {
        if let RecordedEvent::Finalized { block_id, .. } = ev {
            assert!(seen.insert(block_id.clone()), "duplicate finalized block_id: {block_id:?}");
        }
    }
    assert!(seen.len() >= 5, "expected ≥5 unique finalized blocks; got {}", seen.len());
}

// ============================================================================
// Test 8: round-robin normal-load consensus
//
// All N validators take turns leading slots (round robin); every other
// validator validates each candidate. No skips, fast cadence — the system
// produces a steady, ordered stream of finalized blocks with correct
// leader assignment.
// ============================================================================

#[test]
fn test_emulator_round_robin_normal_load() {
    const N: usize = 4;
    let session_id = make_session_id(0x10);
    let shard = ShardIdent::default();
    let validators = build_validators(N);
    let listener = Recorder::new(session_id.clone(), shard.clone(), validators.clone());

    let mut opts = EmulatorOptions::defaults_for_test(session_id, shard);
    opts.leader_rotation = EmulatorLeaderRotation::RoundRobin;
    opts.signer_subset = EmulatorSignerSubset::All;
    opts.deterministic_seed = Some(0xC0DE_BABE_u64);
    opts.initial_seqno = 1;
    opts.initial_params = EmulatorParams {
        slot_interval: EmulatorDelaySpec::new(15, 25, 0.0),
        on_candidate_delay: EmulatorDelaySpec::new(2, 5, 0.0),
        observed_body_delay: EmulatorDelaySpec::new(2, 5, 0.0),
        observed_notar_delay: EmulatorDelaySpec::new(6, 10, 0.0),
        finalized_delay: EmulatorDelaySpec::new(10, 18, 0.0),
    };

    let emu = build_emulator_with(opts, &listener, validators.clone(), 0);
    emu.start(vec![], BlockIdExt::default());
    // ~1.2s @ ~20 ms/slot → ~60 slots, ~15 per validator under N=4.
    thread::sleep(Duration::from_millis(1200));
    emu.stop();

    let events = listener.snapshot();
    let generate_count =
        events.iter().filter(|e| matches!(e, RecordedEvent::GenerateSlot { .. })).count();
    let candidate_count =
        events.iter().filter(|e| matches!(e, RecordedEvent::Candidate { .. })).count();
    let finalized_count =
        events.iter().filter(|e| matches!(e, RecordedEvent::Finalized { .. })).count();

    assert!(
        generate_count >= 5,
        "local should lead at least 5 times under round robin; got {generate_count}"
    );
    // Round-robin with N=4 means remotes lead 3x as often as the local
    // node. Use a slightly relaxed bound (`>= 2 * generate_count`) so that a
    // partial cycle at the end of the run (when stop_flag short-circuits a
    // few remote on_candidate posts mid-flight) doesn't flake.
    assert!(
        candidate_count >= 2 * generate_count,
        "remote leaders should fire on_candidate much more often than local generate_slot; \
         generate={generate_count} candidate={candidate_count}"
    );
    assert!(finalized_count >= 20, "expected steady finalize stream; got {finalized_count}");

    // Round-robin leader assignment: leader[s] = validators[s % N]. Verify
    // every Finalized event matches that mapping and that all N validators
    // appear as leaders.
    let mut led_by: std::collections::HashMap<PublicKeyHash, usize> =
        std::collections::HashMap::new();
    for ev in &events {
        if let RecordedEvent::Finalized {
            slot_from_variant,
            source,
            all_sigs_valid,
            sig_count,
            ..
        } = ev
        {
            let expected_leader_idx = (*slot_from_variant as usize) % N;
            let expected_id = validators[expected_leader_idx].adnl_id.clone();
            assert_eq!(
                source, &expected_id,
                "slot {slot_from_variant}: expected leader idx {expected_leader_idx}, got source \
                 {source}"
            );
            assert!(*all_sigs_valid, "round-robin slot {slot_from_variant} sigs must verify");
            assert_eq!(*sig_count, N, "all validators should sign in normal-load run");
            *led_by.entry(source.clone()).or_default() += 1;
        }
    }
    assert_eq!(
        led_by.len(),
        N,
        "every validator should lead at least one finalized slot under round-robin; \
         led_by={led_by:?}"
    );
    for (id, count) in &led_by {
        assert!(*count >= 3, "validator {id} led only {count} finalized slots — too few");
    }
}

// ============================================================================
// Test 9: high-load consensus — long collations, dropped validations,
// out-of-order finalize delivery
//
// Mimics a production-style stress profile:
//   * `on_candidate_delay` is large (≥3x slot cadence), so remote collation
//     work overlaps multiple slots in-flight.
//   * `observed_notar_delay.skip_probability` drops 40% of "parent_ready"
//     callbacks (some "validations" never land).
//   * `finalized_delay` has wide variance, so finalize callbacks arrive out
//     of slot order.
//
// Verifies:
//   * Significantly fewer parent_ready observations than body observations.
//   * At least one inversion in the finalize delivery sequence (proof of
//     reordering).
//   * All finalized signatures still verify (high load doesn't break
//     correctness).
// ============================================================================

#[test]
fn test_emulator_high_load_with_skips_and_reordering() {
    const N: usize = 4;
    let session_id = make_session_id(0x11);
    let shard = ShardIdent::default();
    let validators = build_validators(N);
    let listener = Recorder::new(session_id.clone(), shard.clone(), validators.clone());

    let mut opts = EmulatorOptions::defaults_for_test(session_id, shard);
    opts.leader_rotation = EmulatorLeaderRotation::RoundRobin;
    opts.signer_subset = EmulatorSignerSubset::All;
    opts.deterministic_seed = Some(0x10AD_BEEF_u64);
    opts.initial_seqno = 1;
    opts.initial_params = EmulatorParams {
        slot_interval: EmulatorDelaySpec::new(15, 25, 0.0),
        on_candidate_delay: EmulatorDelaySpec::new(40, 80, 0.0),
        observed_body_delay: EmulatorDelaySpec::new(5, 15, 0.0),
        observed_notar_delay: EmulatorDelaySpec::new(20, 40, 0.4),
        finalized_delay: EmulatorDelaySpec::new(30, 200, 0.0),
    };

    let emu = build_emulator_with(opts, &listener, validators, 0);
    emu.start(vec![], BlockIdExt::default());
    thread::sleep(Duration::from_millis(2500));
    emu.stop();

    let events = listener.snapshot();
    let body_observed = events
        .iter()
        .filter(|e| matches!(e, RecordedEvent::Observed { flags, .. } if !flags.parent_ready))
        .count();
    let notar_observed = events
        .iter()
        .filter(|e| matches!(e, RecordedEvent::Observed { flags, .. } if flags.parent_ready))
        .count();
    let finalized_count =
        events.iter().filter(|e| matches!(e, RecordedEvent::Finalized { .. })).count();

    assert!(body_observed >= 30, "expected many body observations under load; {body_observed}");
    assert!(finalized_count >= 20, "system must still finalize under load; {finalized_count}");
    // 40% skip on notar means notar count should be materially below body count.
    // Loose CI (≤ 80% of body) absorbs RNG noise; the point estimate is ~60%.
    assert!(
        notar_observed * 5 <= body_observed * 4,
        "missing-validation pressure not observable: body={body_observed} notar={notar_observed}"
    );

    // All finalized signatures must still verify under load.
    for ev in &events {
        if let RecordedEvent::Finalized { all_sigs_valid, slot_from_variant, .. } = ev {
            assert!(*all_sigs_valid, "slot {slot_from_variant} sigs must verify under load");
        }
    }

    // Detect reordering: scan the finalize sequence for at least one
    // inversion (slot[i+1] < slot[i]). With wide finalized_delay variance
    // this is overwhelmingly likely for any run of meaningful length.
    let finalized_slots: Vec<u32> = events
        .iter()
        .filter_map(|e| match e {
            RecordedEvent::Finalized { slot_from_variant, .. } => Some(*slot_from_variant),
            _ => None,
        })
        .collect();
    let mut inversions = 0usize;
    for w in finalized_slots.windows(2) {
        if w[1] < w[0] {
            inversions += 1;
        }
    }
    assert!(
        inversions >= 1,
        "no reordering observed despite wide finalized_delay variance; slots={finalized_slots:?}"
    );
}

// ============================================================================
// Test 10: stall via runtime parameter change
//
// Three phases:
//   1. Normal cadence — finalize counter grows.
//   2. `finalized_delay.skip_probability = 1.0` — every finalize callback
//      is dropped at scheduling time. Slot ticks keep arriving (so the
//      slot chain is not broken), but no `on_block_finalized` fires. The
//      finalize counter must plateau apart from a few in-flight tasks
//      scheduled before the param change.
//   3. Restore — finalize callbacks resume.
//
// Verifies stalls are observable end-to-end via parameter mutation and
// that recovery is clean (no leftover deadlocks, queues drain, signatures
// continue to verify after recovery).
// ============================================================================

#[test]
fn test_emulator_stall_via_param_change() {
    const N: usize = 4;
    let session_id = make_session_id(0x12);
    let shard = ShardIdent::default();
    let validators = build_validators(N);
    let listener = Recorder::new(session_id.clone(), shard.clone(), validators.clone());

    let mut opts = EmulatorOptions::defaults_for_test(session_id, shard);
    opts.leader_rotation = EmulatorLeaderRotation::RoundRobin;
    opts.signer_subset = EmulatorSignerSubset::All;
    opts.deterministic_seed = Some(0x00C0_FFEE_BEAD_u64);
    // Slow cadence on purpose: the recorder's per-finalize signature
    // verification (N Ed25519 verifies, ~1 ms each in debug builds) is the
    // dominant cb-thread cost. Running at <50 ms/slot keeps the cb queue
    // backlog-free so the stall-and-recover transitions are observable
    // without queue-drain noise.
    let healthy_params = EmulatorParams {
        slot_interval: EmulatorDelaySpec::new(50, 70, 0.0),
        on_candidate_delay: EmulatorDelaySpec::new(3, 6, 0.0),
        observed_body_delay: EmulatorDelaySpec::new(3, 6, 0.0),
        observed_notar_delay: EmulatorDelaySpec::new(8, 12, 0.0),
        finalized_delay: EmulatorDelaySpec::new(5, 15, 0.0),
    };
    opts.initial_params = healthy_params.clone();

    let emu = build_emulator_with(opts, &listener, validators, 0);
    emu.start(vec![], BlockIdExt::default());

    let finalized_count = || listener.count_event(|e| matches!(e, RecordedEvent::Finalized { .. }));

    // Phase 1: normal cadence — accumulate finalized events.
    thread::sleep(Duration::from_millis(1000));
    let phase1_finalized = finalized_count();

    // Phase 2: drop every finalize callback at scheduling time.
    let mut stalled = healthy_params.clone();
    stalled.finalized_delay = EmulatorDelaySpec { min_ms: 5, max_ms: 15, skip_probability: 1.0 };
    emu.set_params(stalled).expect("set stalled params");

    // After the param change, give the cb queue enough time to fully
    // drain the residual finalize callbacks that were scheduled with the
    // old (skip=0) delay. With healthy cadence above, max residual is
    // bounded by (finalized_delay.max_ms + cb_drain_margin); 400 ms is
    // comfortably above that even with debug-build crypto latency.
    thread::sleep(Duration::from_millis(400));
    let phase2_start = finalized_count();
    thread::sleep(Duration::from_millis(700));
    let phase2_end = finalized_count();

    // Phase 3: restore — finalize callbacks resume scheduling.
    emu.set_params(healthy_params).expect("restore healthy params");
    thread::sleep(Duration::from_millis(1000));
    emu.stop();
    let phase3_end = finalized_count();

    assert!(
        phase1_finalized >= 8,
        "phase 1 should accumulate a healthy finalize count; got {phase1_finalized}"
    );

    let phase2_delta = phase2_end - phase2_start;
    assert!(
        phase2_delta <= 1,
        "phase 2 (stalled) should produce ~0 finalize events after drain; got \
         phase2_start={phase2_start} phase2_end={phase2_end} (delta {phase2_delta})"
    );

    let phase3_delta = phase3_end - phase2_end;
    assert!(
        phase3_delta >= 8,
        "phase 3 (recovered) should resume finalizing; got phase2_end={phase2_end} \
         phase3_end={phase3_end} (delta {phase3_delta})"
    );

    // All finalized sigs (across all phases) must verify.
    for ev in listener.snapshot() {
        if let RecordedEvent::Finalized { all_sigs_valid, slot_from_variant, .. } = ev {
            assert!(all_sigs_valid, "stall/recover slot {slot_from_variant} sigs failed to verify");
        }
    }
}

// ============================================================================
// Test 11: synchronous wait for stop()
//
// Verifies the Session::stop() contract used by ValidatorGroup:
//   * `stop()` blocks until both internal threads (main + callbacks) have
//     drained their queues and joined.
//   * `stop()` waits for downstream finalize callbacks already scheduled
//     on the timer heap, so no listener event fires after `stop()` returns.
//   * `stop()` is idempotent — a second invocation returns promptly.
//   * `stop_async()` is non-blocking, and a subsequent `stop()` still
//     converges to the synchronous-wait semantics.
// ============================================================================

#[test]
fn test_emulator_stop_is_synchronous() {
    const N: usize = 4;
    let session_id = make_session_id(0x13);
    let shard = ShardIdent::default();
    let validators = build_validators(N);
    let listener = Recorder::new(session_id.clone(), shard.clone(), validators.clone());

    let mut opts = EmulatorOptions::defaults_for_test(session_id, shard);
    opts.leader_rotation = EmulatorLeaderRotation::RoundRobin;
    opts.signer_subset = EmulatorSignerSubset::All;
    opts.deterministic_seed = Some(0x5709_BADD_u64);
    opts.initial_params = EmulatorParams {
        slot_interval: EmulatorDelaySpec::new(10, 15, 0.0),
        on_candidate_delay: EmulatorDelaySpec::new(2, 5, 0.0),
        observed_body_delay: EmulatorDelaySpec::new(5, 10, 0.0),
        observed_notar_delay: EmulatorDelaySpec::new(8, 12, 0.0),
        // Long, fixed-window finalize delay so many finalize callbacks
        // are guaranteed to be in-flight (sitting on the heap) when
        // stop() is invoked. The synchronous-stop drain logic must wait
        // for them before returning.
        finalized_delay: EmulatorDelaySpec::new(120, 150, 0.0),
    };

    let emu = build_emulator_with(opts, &listener, validators, 0);
    emu.start(vec![], BlockIdExt::default());
    // Pump slots for 400 ms: ~30-40 slot ticks, generating many pending
    // finalize callbacks scheduled at +120..150 ms.
    thread::sleep(Duration::from_millis(400));

    let before_stop = listener.count_event(|e| matches!(e, RecordedEvent::Finalized { .. }));
    let stop_started = Instant::now();
    emu.stop();
    let stop_elapsed = stop_started.elapsed();
    let after_stop = listener.count_event(|e| matches!(e, RecordedEvent::Finalized { .. }));

    // 1. stop() drained pending finalize callbacks before returning. We
    //    expect more finalize events after stop() than before it.
    assert!(
        after_stop > before_stop,
        "stop() must drain pending finalize callbacks; before={before_stop} after={after_stop}"
    );

    // 2. stop() waited long enough for at least some of the +120ms-delayed
    //    finalize tasks to fire. The drain logic adds ≥SHUTDOWN_QUIESCE
    //    margin on top of that.
    assert!(
        stop_elapsed >= Duration::from_millis(60),
        "stop() returned suspiciously fast given queued in-flight tasks: {stop_elapsed:?}"
    );

    // 3. No further listener events fire after stop() returns — the
    //    callbacks thread is joined and the queues are quiesced.
    let snapshot_immediately_after = listener.snapshot();
    thread::sleep(Duration::from_millis(300));
    let snapshot_after_sleep = listener.snapshot();
    assert_eq!(
        snapshot_immediately_after.len(),
        snapshot_after_sleep.len(),
        "events fired after stop() returned: before-sleep={} after-sleep={}",
        snapshot_immediately_after.len(),
        snapshot_after_sleep.len()
    );

    // 4. Idempotency: a second stop() returns quickly (no threads to wait
    //    on; just sets the flag and exits the poll loop immediately).
    let stop2_started = Instant::now();
    emu.stop();
    let stop2_elapsed = stop2_started.elapsed();
    assert!(
        stop2_elapsed < Duration::from_millis(50),
        "second stop() should be near-instant; elapsed: {stop2_elapsed:?}"
    );
}

// ============================================================================
// Test 12: stop_async() is non-blocking; stop() afterwards still synchronous
// ============================================================================

#[test]
fn test_emulator_stop_async_then_sync() {
    const N: usize = 4;
    let session_id = make_session_id(0x14);
    let shard = ShardIdent::default();
    let validators = build_validators(N);
    let listener = Recorder::new(session_id.clone(), shard.clone(), validators.clone());

    let mut opts = EmulatorOptions::defaults_for_test(session_id, shard);
    opts.leader_rotation = EmulatorLeaderRotation::RoundRobin;
    opts.signer_subset = EmulatorSignerSubset::All;
    opts.deterministic_seed = Some(0xA5BC_DEFA_u64);
    opts.initial_params = EmulatorParams {
        slot_interval: EmulatorDelaySpec::new(10, 15, 0.0),
        on_candidate_delay: EmulatorDelaySpec::new(2, 5, 0.0),
        observed_body_delay: EmulatorDelaySpec::new(3, 6, 0.0),
        observed_notar_delay: EmulatorDelaySpec::new(6, 10, 0.0),
        finalized_delay: EmulatorDelaySpec::new(80, 120, 0.0),
    };

    let emu = build_emulator_with(opts, &listener, validators, 0);
    emu.start(vec![], BlockIdExt::default());
    thread::sleep(Duration::from_millis(300));

    // stop_async() must return promptly (no thread-wait).
    let async_started = Instant::now();
    emu.stop_async();
    let async_elapsed = async_started.elapsed();
    assert!(
        async_elapsed < Duration::from_millis(20),
        "stop_async() should not block; elapsed: {async_elapsed:?}"
    );

    // stop() called after stop_async() converges to the synchronous-wait
    // semantics (waits for queues to drain).
    let sync_started = Instant::now();
    emu.stop();
    let sync_elapsed = sync_started.elapsed();
    // After stop() returns, no events should fire.
    let len_at_stop = listener.snapshot().len();
    thread::sleep(Duration::from_millis(300));
    let len_after_sleep = listener.snapshot().len();
    assert_eq!(
        len_at_stop, len_after_sleep,
        "events fired after stop() (post-stop_async) returned"
    );
    // Sanity: we ran some, then stopped — there should be at least a few
    // recorded events.
    assert!(len_at_stop >= 10, "expected to see some events; len={len_at_stop}");
    // Logical sanity: sync stop() returned in finite time (loose upper bound
    // mostly to flag deadlock regressions).
    assert!(
        sync_elapsed < Duration::from_secs(5),
        "stop() after stop_async() took too long: {sync_elapsed:?}"
    );
}

// ============================================================================
// Test 13: parent chain inside leader windows
// ============================================================================

#[test]
fn test_emulator_parent_chain_across_leader_windows() {
    const N: usize = 4;
    let session_id = make_session_id(0x15);
    let shard = ShardIdent::default();
    let validators = build_validators(N);
    let listener = Recorder::new(session_id.clone(), shard.clone(), validators.clone());

    let mut opts = EmulatorOptions::defaults_for_test(session_id, shard);
    opts.leader_rotation = EmulatorLeaderRotation::RoundRobin;
    opts.slots_per_leader_window = 3;
    opts.signer_subset = EmulatorSignerSubset::All;
    opts.deterministic_seed = Some(0xFACE_FEED_u64);
    opts.initial_params = EmulatorParams {
        slot_interval: EmulatorDelaySpec::new(15, 20, 0.0),
        on_candidate_delay: EmulatorDelaySpec::ZERO,
        observed_body_delay: EmulatorDelaySpec::ZERO,
        observed_notar_delay: EmulatorDelaySpec::ZERO,
        finalized_delay: EmulatorDelaySpec::ZERO,
    };

    // Use a non-leading local index so the first two leader windows are
    // remote and produce deterministic slot->seqno mapping.
    let emu = build_emulator_with(opts, &listener, validators, 3);
    emu.start(vec![], BlockIdExt::default());
    thread::sleep(Duration::from_millis(500));
    emu.stop();

    let mut finalized: Vec<_> = listener
        .snapshot()
        .into_iter()
        .filter_map(|e| match e {
            RecordedEvent::Finalized {
                block_id,
                slot_from_variant,
                parent,
                candidate_hash,
                all_sigs_valid,
                ..
            } if slot_from_variant < 6 => {
                Some((slot_from_variant, block_id, parent, candidate_hash, all_sigs_valid))
            }
            _ => None,
        })
        .collect();
    finalized.sort_by_key(|(slot, ..)| *slot);

    assert_eq!(finalized.len(), 6, "expected finalized slots 0..5; got {finalized:?}");
    for idx in 0..finalized.len() {
        let (slot, block_id, parent, candidate_hash, all_sigs_valid) = &finalized[idx];
        assert!(*all_sigs_valid, "slot {slot} signatures must verify");
        let expected_parent = if idx == 0 {
            None
        } else {
            Some((finalized[idx - 1].0, finalized[idx - 1].3.clone()))
        };
        assert_eq!(parent, &expected_parent, "slot {slot}: wrong parent; finalized={finalized:?}");
        assert_eq!(
            candidate_hash,
            &compute_candidate_hash(
                block_id,
                &UInt256::default(),
                parent.as_ref().map(|(slot, hash)| (*slot, hash))
            ),
            "slot {slot}: candidate hash should include encoded parent"
        );
    }
}

#[test]
fn test_emulator_parent_chain_skips_rejected_candidates() {
    const N: usize = 4;
    let session_id = make_session_id(0x16);
    let shard = ShardIdent::default();
    let validators = build_validators(N);
    let listener = Recorder::new(session_id.clone(), shard.clone(), validators.clone());
    listener.set_reject_every(Some(2));

    let mut opts = EmulatorOptions::defaults_for_test(session_id, shard);
    opts.leader_rotation = EmulatorLeaderRotation::RoundRobin;
    opts.slots_per_leader_window = 3;
    opts.signer_subset = EmulatorSignerSubset::All;
    opts.deterministic_seed = Some(0x0BAD_5EED_u64);
    opts.initial_params = EmulatorParams {
        slot_interval: EmulatorDelaySpec::new(15, 20, 0.0),
        on_candidate_delay: EmulatorDelaySpec::ZERO,
        observed_body_delay: EmulatorDelaySpec::ZERO,
        observed_notar_delay: EmulatorDelaySpec::ZERO,
        finalized_delay: EmulatorDelaySpec::ZERO,
    };

    let emu = build_emulator_with(opts, &listener, validators, 3);
    emu.start(vec![], BlockIdExt::default());
    thread::sleep(Duration::from_millis(600));
    emu.stop();

    let mut finalized: Vec<_> = listener
        .snapshot()
        .into_iter()
        .filter_map(|e| match e {
            RecordedEvent::Finalized { slot_from_variant, parent, candidate_hash, .. }
                if slot_from_variant <= 6 =>
            {
                Some((slot_from_variant, parent, candidate_hash))
            }
            _ => None,
        })
        .collect();
    finalized.sort_by_key(|(slot, ..)| *slot);

    let slots: Vec<u32> = finalized.iter().map(|(slot, ..)| *slot).collect();
    assert!(
        slots.starts_with(&[0, 2, 4, 6]),
        "every second remote candidate should be rejected; got slots={slots:?}"
    );

    for idx in 0..finalized.len().min(4) {
        let (slot, parent, _) = &finalized[idx];
        let expected_parent = if idx == 0 {
            None
        } else {
            Some((finalized[idx - 1].0, finalized[idx - 1].2.clone()))
        };
        assert_eq!(
            parent, &expected_parent,
            "slot {slot}: accepted chain should skip rejected candidates; finalized={finalized:?}"
        );
    }
}

// ============================================================================
// Test 14: local collation timeout and parameter validation
// ============================================================================

#[test]
fn test_emulator_local_collation_timeout_drops_late_candidate() {
    let session_id = make_session_id(0x17);
    let shard = ShardIdent::default();
    let validators = build_validators(N_VALIDATORS);
    let listener = Recorder::new(session_id.clone(), shard.clone(), validators.clone());
    listener.set_generate_delay(Duration::from_millis(80));

    let mut opts = EmulatorOptions::defaults_for_test(session_id, shard);
    opts.leader_rotation = EmulatorLeaderRotation::FixedLocal;
    opts.local_collation_timeout = Some(Duration::from_millis(15));
    opts.signer_subset = EmulatorSignerSubset::All;
    opts.initial_params = EmulatorParams {
        slot_interval: EmulatorDelaySpec::new(30, 40, 0.0),
        on_candidate_delay: EmulatorDelaySpec::ZERO,
        observed_body_delay: EmulatorDelaySpec::ZERO,
        observed_notar_delay: EmulatorDelaySpec::ZERO,
        finalized_delay: EmulatorDelaySpec::ZERO,
    };

    let emu = build_emulator_with(opts, &listener, validators, 0);
    emu.start(vec![], BlockIdExt::default());
    thread::sleep(Duration::from_millis(250));
    emu.stop();

    let generated = listener.count_event(|e| matches!(e, RecordedEvent::GenerateSlot { .. }));
    let finalized = listener.count_event(|e| matches!(e, RecordedEvent::Finalized { .. }));
    assert!(generated > 0, "timeout test must request at least one collation");
    assert_eq!(finalized, 0, "late local candidates must be dropped after timeout");
}

#[test]
fn test_emulator_rejects_invalid_delay_specs() {
    let session_id = make_session_id(0x18);
    let shard = ShardIdent::default();
    let validators = build_validators(N_VALIDATORS);
    let listener = Recorder::new(session_id.clone(), shard.clone(), validators.clone());

    let listener_ptr: SessionListenerPtr =
        Arc::downgrade(&(listener.clone() as Arc<dyn SessionListener + Send + Sync>));

    let mut opts = EmulatorOptions::defaults_for_test(session_id.clone(), shard.clone());
    opts.initial_params.slot_interval = EmulatorDelaySpec::new(10, 5, 0.0);
    let result = ConsensusCommonFactory::create_consensus_emulator(
        opts,
        validators.clone(),
        0,
        listener_ptr,
    );
    assert!(result.is_err(), "construction should reject max_ms < min_ms");

    let listener_ptr: SessionListenerPtr =
        Arc::downgrade(&(listener.clone() as Arc<dyn SessionListener + Send + Sync>));
    let mut opts = EmulatorOptions::defaults_for_test(session_id, shard);
    opts.initial_params.slot_interval = EmulatorDelaySpec::new(10, 10, 0.0);
    let emu = ConsensusCommonFactory::create_consensus_emulator(opts, validators, 0, listener_ptr)
        .expect("valid emulator");

    let mut invalid = emu.get_params();
    invalid.finalized_delay = EmulatorDelaySpec::new(0, 0, f64::NAN);
    assert!(emu.set_params(invalid).is_err(), "set_params should reject NaN skip probability");
}

// ============================================================================
// Test 15: plan coverage — explicit start gate
// ============================================================================

#[test]
fn test_emulator_start_gate_no_callbacks_before_start() {
    let session_id = make_session_id(0x19);
    let shard = ShardIdent::default();
    let validators = build_validators(N_VALIDATORS);
    let listener = Recorder::new(session_id.clone(), shard.clone(), validators.clone());

    let mut opts = EmulatorOptions::defaults_for_test(session_id, shard);
    opts.leader_rotation = EmulatorLeaderRotation::FixedLocal;
    opts.signer_subset = EmulatorSignerSubset::All;
    opts.initial_params = EmulatorParams {
        slot_interval: EmulatorDelaySpec::new(5, 10, 0.0),
        on_candidate_delay: EmulatorDelaySpec::ZERO,
        observed_body_delay: EmulatorDelaySpec::ZERO,
        observed_notar_delay: EmulatorDelaySpec::ZERO,
        finalized_delay: EmulatorDelaySpec::ZERO,
    };

    let emu = build_emulator_with(opts, &listener, validators, 0);

    thread::sleep(Duration::from_millis(250));
    assert!(
        listener.snapshot().is_empty(),
        "emulator must not fire listener callbacks before start()"
    );

    emu.start(vec![], BlockIdExt::default());
    let deadline = Instant::now() + Duration::from_secs(2);
    while listener.snapshot().is_empty() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    emu.stop();

    assert!(
        !listener.snapshot().is_empty(),
        "emulator should begin callbacks promptly after start()"
    );
}

// ============================================================================
// Test 16: plan coverage — resolver-observation parent_ready sequence
// ============================================================================

#[test]
fn test_emulator_resolver_observation_parent_ready_sequence() {
    let session_id = make_session_id(0x1A);
    let shard = ShardIdent::default();
    let validators = build_validators(N_VALIDATORS);
    let listener = Recorder::new(session_id.clone(), shard.clone(), validators.clone());

    let mut opts = EmulatorOptions::defaults_for_test(session_id, shard);
    opts.leader_rotation = EmulatorLeaderRotation::FixedIndex(1);
    opts.signer_subset = EmulatorSignerSubset::All;
    opts.deterministic_seed = Some(0x0B5E_0B5E_u64);
    opts.initial_params = EmulatorParams {
        slot_interval: EmulatorDelaySpec::new(20, 25, 0.0),
        on_candidate_delay: EmulatorDelaySpec::ZERO,
        observed_body_delay: EmulatorDelaySpec::ZERO,
        observed_notar_delay: EmulatorDelaySpec::new(40, 50, 0.0),
        finalized_delay: EmulatorDelaySpec::new(60, 70, 0.0),
    };

    let emu = build_emulator_with(opts, &listener, validators, 0);
    emu.start(vec![], BlockIdExt::default());
    thread::sleep(Duration::from_millis(500));
    emu.stop();

    let events = listener.snapshot();
    let mut body_before_parent_ready = 0usize;
    let mut parent_ready_after_body = 0usize;
    let mut seen_body = HashSet::new();

    for event in &events {
        if let RecordedEvent::Observed { block_id, flags } = event {
            assert!(flags.body_present, "observed callbacks should report body_present");
            if !flags.parent_ready {
                assert!(
                    seen_body.insert(block_id.clone()),
                    "duplicate body observation for {block_id:?}"
                );
                body_before_parent_ready += 1;
            } else if seen_body.contains(block_id) {
                parent_ready_after_body += 1;
            }
        }
    }

    assert!(
        body_before_parent_ready >= 5,
        "resolver-style body observations should be visible before parent_ready; got {body_before_parent_ready}"
    );
    assert!(
        parent_ready_after_body >= 5,
        "parent_ready observations should follow prior body observations; got {parent_ready_after_body}"
    );
}

// ============================================================================
// Test 17: plan coverage — fresh session monotonic seqno after stop
// ============================================================================

#[test]
fn test_emulator_fresh_session_initial_seqno_monotonic_after_stop() {
    fn run_remote_session(initial_seqno: u32, tag: u8) -> Vec<BlockIdExt> {
        let session_id = make_session_id(tag);
        let shard = ShardIdent::default();
        let validators = build_validators(N_VALIDATORS);
        let listener = Recorder::new(session_id.clone(), shard.clone(), validators.clone());

        let mut opts = EmulatorOptions::defaults_for_test(session_id, shard);
        opts.initial_seqno = initial_seqno;
        opts.leader_rotation = EmulatorLeaderRotation::FixedIndex(1);
        opts.signer_subset = EmulatorSignerSubset::All;
        opts.initial_params = EmulatorParams {
            slot_interval: EmulatorDelaySpec::new(15, 20, 0.0),
            on_candidate_delay: EmulatorDelaySpec::ZERO,
            observed_body_delay: EmulatorDelaySpec::ZERO,
            observed_notar_delay: EmulatorDelaySpec::ZERO,
            finalized_delay: EmulatorDelaySpec::ZERO,
        };

        let emu = build_emulator_with(opts, &listener, validators, 0);
        emu.start(vec![], BlockIdExt::default());
        thread::sleep(Duration::from_millis(250));
        emu.stop();

        listener
            .snapshot()
            .into_iter()
            .filter_map(|event| match event {
                RecordedEvent::Finalized { block_id, .. } => Some(block_id),
                _ => None,
            })
            .collect()
    }

    let first = run_remote_session(100, 0x1B);
    assert!(!first.is_empty(), "first session should finalize at least one block");
    let first_max_seqno = first.iter().map(|id| id.seq_no()).max().unwrap();

    let second = run_remote_session(first_max_seqno + 1, 0x1C);
    assert!(!second.is_empty(), "second session should finalize at least one block");
    let second_min_seqno = second.iter().map(|id| id.seq_no()).min().unwrap();

    assert_eq!(
        second_min_seqno,
        first_max_seqno + 1,
        "fresh emulator should resume from caller-provided next initial_seqno"
    );
}
