/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Focused unit tests for [`crate::session_callbacks::SessionCallbacks`].
//!
//! Included directly from `session_callbacks.rs` via `#[path]` so tests can
//! reach the private `invoke` helper and confirm the suppression gate / SXCB
//! dispatch decisions without widening the public-by-crate surface.
//! Mirrors the convention used by `tests/test_session_telemetry.rs` and
//! `tests/test_session_runtime.rs`.
//!
//! Scope:
//! - `use_callback_thread` routing — inline vs SXCB queue dispatch.
//! - `stop_flag` suppression gate — silently drops callbacks on both paths.
//! - `notify_candidate` / `notify_candidate_observed` /
//!   `notify_generate_slot` / `notify_block_finalized` wrappers correctly
//!   reach the listener with the supplied payload.
//! - `notify_*` is a no-op when the `SessionListenerPtr` weak handle has
//!   already dropped (listener torn down before dispatch).
//! - `Debug` impl reports the current `stop_flag` boolean value.
//! - `run_worker_loop` (the relocated `SXCB` loop body) pulls posted
//!   closures, exits cleanly on `should_stop_flag`, and records the stop
//!   via `is_stopped_flag`. This is the smoke test for the
//!   `use_callback_thread=true` post-move spawn integration in
//!   `SessionImpl::create`.

use super::*;
use crate::{
    task_queue::{CallbackTaskPtr, TaskQueue},
    BlockSourceInfo, MetricsHandle, SessionId, ValidatorBlockCandidateDecisionCallback,
};
use consensus_common::{
    AsyncCollationRequest, AsyncRequest, BlockCandidatePriority, CandidateObservedFlags,
    CollationParentHint, ConsensusCommonFactory, SessionListener, ValidatorBlockCandidateCallback,
};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime},
};
use ton_block::{BlockIdExt, Ed25519KeyOption, ShardIdent, UInt256, ZeroizingBytes};

/*
    Test scaffolding — mock queue, listener, async-request
*/

/// In-memory mock `CallbackTaskQueue` that records every posted closure
/// without executing it. Lets `invoke`-dispatch tests assert the routing
/// decision (inline vs SXCB) without depending on a real crossbeam channel.
struct RecordingCallbackQueue {
    posted: Mutex<VecDeque<CallbackTaskPtr>>,
}

impl RecordingCallbackQueue {
    fn new() -> Arc<Self> {
        Arc::new(Self { posted: Mutex::new(VecDeque::new()) })
    }

    fn len(&self) -> usize {
        self.posted.lock().unwrap().len()
    }

    /// Pop and run every queued task — used by listener-roundtrip tests so
    /// they can exercise the closure body that `invoke` produced.
    fn drain_and_run(&self) {
        while let Some(task) = self.posted.lock().unwrap().pop_front() {
            task();
        }
    }
}

impl TaskQueue<CallbackTaskPtr> for RecordingCallbackQueue {
    fn is_overloaded(&self) -> bool {
        false
    }

    fn is_empty(&self) -> bool {
        self.posted.lock().unwrap().is_empty()
    }

    fn post_closure(&self, task: CallbackTaskPtr) {
        self.posted.lock().unwrap().push_back(task);
    }

    fn pull_closure(
        &self,
        _timeout: Duration,
        _last_warn_dump_time: &mut SystemTime,
    ) -> Option<CallbackTaskPtr> {
        self.posted.lock().unwrap().pop_front()
    }

    fn flush(&self) {
        self.posted.lock().unwrap().clear();
    }
}

/// Mock listener that records each callback for verification. Single-shot
/// records are sufficient because the focused tests dispatch one
/// `notify_*` per `SessionCallbacks` instance.
#[derive(Default)]
struct RecordingListener {
    candidate_calls: AtomicUsize,
    candidate_observed_calls: AtomicUsize,
    generate_slot_calls: AtomicUsize,
    block_finalized_calls: AtomicUsize,

    /// Most-recently-observed `root_hash` from `on_candidate`.
    last_candidate_root_hash: Mutex<Option<UInt256>>,
    /// Most-recently-observed `block_id` from `on_candidate_observed`.
    last_candidate_observed_block_id: Mutex<Option<BlockIdExt>>,
    /// Most-recently-observed `flags` from `on_candidate_observed`.
    last_candidate_observed_flags: Mutex<Option<CandidateObservedFlags>>,
    /// Most-recently-observed `block_id` from `on_block_finalized`.
    last_block_finalized_id: Mutex<Option<BlockIdExt>>,
    /// Most-recently-observed `parent` from `on_generate_slot`.
    last_generate_slot_parent: Mutex<Option<CollationParentHint>>,
}

impl SessionListener for RecordingListener {
    fn on_candidate(
        &self,
        _source_info: BlockSourceInfo,
        root_hash: BlockHash,
        _data: BlockPayloadPtr,
        _collated_data: BlockPayloadPtr,
        _callback: ValidatorBlockCandidateDecisionCallback,
    ) {
        self.candidate_calls.fetch_add(1, Ordering::Relaxed);
        *self.last_candidate_root_hash.lock().unwrap() = Some(root_hash);
    }

    fn on_generate_slot(
        &self,
        _source_info: BlockSourceInfo,
        _request: AsyncCollationRequestPtr,
        parent: CollationParentHint,
        _callback: ValidatorBlockCandidateCallback,
    ) {
        self.generate_slot_calls.fetch_add(1, Ordering::Relaxed);
        *self.last_generate_slot_parent.lock().unwrap() = Some(parent);
    }

    fn on_block_committed(
        &self,
        _source_info: BlockSourceInfo,
        _root_hash: UInt256,
        _file_hash: UInt256,
        _data: BlockPayloadPtr,
        _signatures: BlockSignaturesVariant,
        _approve_signatures: Vec<(PublicKeyHash, BlockPayloadPtr)>,
        _stats: consensus_common::SessionStats,
    ) {
        panic!("on_block_committed must not be called for simplex SessionCallbacks tests");
    }

    fn on_block_skipped(&self, _round: u32) {}

    fn get_approved_candidate(
        &self,
        _source: consensus_common::PublicKey,
        _root_hash: UInt256,
        _file_hash: UInt256,
        _collated_data_hash: UInt256,
        _callback: ValidatorBlockCandidateCallback,
    ) {
        panic!("get_approved_candidate must not be called for simplex SessionCallbacks tests");
    }

    fn on_candidate_observed(
        &self,
        block_id: BlockIdExt,
        _data: BlockPayloadPtr,
        _collated_data: BlockPayloadPtr,
        flags: CandidateObservedFlags,
    ) {
        self.candidate_observed_calls.fetch_add(1, Ordering::Relaxed);
        *self.last_candidate_observed_block_id.lock().unwrap() = Some(block_id);
        *self.last_candidate_observed_flags.lock().unwrap() = Some(flags);
    }

    fn on_block_finalized(
        &self,
        block_id: BlockIdExt,
        _source_info: BlockSourceInfo,
        _root_hash: BlockHash,
        _file_hash: BlockHash,
        _data: BlockPayloadPtr,
        _signatures: BlockSignaturesVariant,
        _approve_signatures: Vec<(PublicKeyHash, BlockPayloadPtr)>,
    ) {
        self.block_finalized_calls.fetch_add(1, Ordering::Relaxed);
        *self.last_block_finalized_id.lock().unwrap() = Some(block_id);
    }
}

/// Minimal `AsyncCollationRequest` mock — `notify_generate_slot` takes an
/// `AsyncCollationRequestPtr` payload but never invokes any of its methods inside
/// `SessionCallbacks`; the trait is only satisfied so the listener can
/// receive a live handle.
struct DummyAsyncRequest;

impl AsyncRequest for DummyAsyncRequest {
    fn cancel(&self) {}
    fn get_request_id(&self) -> u32 {
        0
    }
    fn is_cancelled(&self) -> bool {
        false
    }
    fn get_creation_time(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH
    }
}

impl AsyncCollationRequest for DummyAsyncRequest {}

/*
    Construction helpers
*/

/// Build a `SessionCallbacks` wired up to a recording mock queue and the
/// given session listener handle.
fn make_callbacks_with_listener(
    stop_flag: Arc<AtomicBool>,
    use_callback_thread: bool,
    listener: SessionListenerPtr,
) -> (Arc<SessionCallbacks>, Arc<RecordingCallbackQueue>) {
    let queue = RecordingCallbackQueue::new();
    let callbacks = Arc::new(SessionCallbacks::new(
        SessionId::default(),
        stop_flag,
        use_callback_thread,
        queue.clone(),
        listener,
    ));
    (callbacks, queue)
}

/// Build a `SessionCallbacks` wired up to a recording mock queue, with a
/// dangling listener handle.
///
/// For tests that exercise queue routing / suppression / `Debug` and never
/// observe listener delivery: the stored `Weak` is intentionally dead so
/// any accidental dispatch is a silent no-op rather than a panic.
fn make_callbacks(
    stop_flag: Arc<AtomicBool>,
    use_callback_thread: bool,
) -> (Arc<SessionCallbacks>, Arc<RecordingCallbackQueue>) {
    let dead_listener: SessionListenerPtr = {
        let listener: Arc<RecordingListener> = Arc::new(RecordingListener::default());
        Arc::downgrade(&listener) as SessionListenerPtr
        // `listener` drops here, leaving the `Weak` dangling.
    };
    make_callbacks_with_listener(stop_flag, use_callback_thread, dead_listener)
}

/// Build a recording listener wired to a `SessionListenerPtr`.
///
/// The strong `Arc` is returned so the test can keep the listener alive
/// (`SessionListenerPtr` is a `Weak`); dropping the `Arc` invalidates the
/// dispatch path and is tested separately.
fn make_listener() -> (Arc<RecordingListener>, SessionListenerPtr) {
    let listener: Arc<RecordingListener> = Arc::new(RecordingListener::default());
    let weak: SessionListenerPtr = Arc::downgrade(&listener) as SessionListenerPtr;
    (listener, weak)
}

/// Synthesize a `BlockSourceInfo` for tests that need one but do not
/// inspect it. Mirrors `make_roundless_collation_source_info` from
/// `session_processor.rs`.
fn make_test_source_info() -> BlockSourceInfo {
    let key = Ed25519KeyOption::<ZeroizingBytes>::generate().expect("key gen");
    BlockSourceInfo {
        source: key,
        priority: BlockCandidatePriority { round: 0, first_block_round: 0, priority: 0 },
    }
}

fn make_test_block_id(seqno: u32) -> BlockIdExt {
    BlockIdExt::with_params(
        ShardIdent::masterchain(),
        seqno,
        UInt256::default(),
        UInt256::default(),
    )
}

fn make_test_signatures() -> BlockSignaturesVariant {
    // `default()` yields the `Ordinary` variant with an empty signature
    // set; sufficient for closure-dispatch tests that never inspect the
    // payload contents.
    BlockSignaturesVariant::default()
}

/*
    `invoke` routing — inline vs SXCB dispatch
*/

#[test]
fn invoke_runs_closure_inline_when_callback_thread_disabled() {
    let stop = Arc::new(AtomicBool::new(false));
    let (callbacks, queue) = make_callbacks(stop, /* use_callback_thread */ false);

    let counter = Arc::new(AtomicUsize::new(0));
    let counter_for_closure = counter.clone();

    callbacks.invoke(move || {
        counter_for_closure.fetch_add(1, Ordering::Relaxed);
    });

    // Inline path — the closure ran on the calling thread before `invoke`
    // returned, and nothing landed on the SXCB queue.
    assert_eq!(counter.load(Ordering::Relaxed), 1);
    assert_eq!(queue.len(), 0);
}

#[test]
fn invoke_posts_to_queue_when_callback_thread_enabled() {
    let stop = Arc::new(AtomicBool::new(false));
    let (callbacks, queue) = make_callbacks(stop, /* use_callback_thread */ true);

    let counter = Arc::new(AtomicUsize::new(0));
    let counter_for_closure = counter.clone();

    callbacks.invoke(move || {
        counter_for_closure.fetch_add(1, Ordering::Relaxed);
    });

    // Worker path — closure must be deferred onto the SXCB queue and must
    // NOT have executed on the calling thread yet.
    assert_eq!(counter.load(Ordering::Relaxed), 0);
    assert_eq!(queue.len(), 1);

    // Running the queued task delivers the side effect.
    queue.drain_and_run();
    assert_eq!(counter.load(Ordering::Relaxed), 1);
}

#[test]
fn invoke_suppresses_when_stop_flag_set_inline_path() {
    let stop = Arc::new(AtomicBool::new(true));
    let (callbacks, queue) = make_callbacks(stop, /* use_callback_thread */ false);

    let counter = Arc::new(AtomicUsize::new(0));
    let counter_for_closure = counter.clone();

    callbacks.invoke(move || {
        counter_for_closure.fetch_add(1, Ordering::Relaxed);
    });

    // Stop flag is set — closure must be dropped on the floor.
    assert_eq!(counter.load(Ordering::Relaxed), 0);
    assert_eq!(queue.len(), 0);
}

#[test]
fn invoke_suppresses_when_stop_flag_set_worker_path() {
    let stop = Arc::new(AtomicBool::new(true));
    let (callbacks, queue) = make_callbacks(stop, /* use_callback_thread */ true);

    let counter = Arc::new(AtomicUsize::new(0));
    let counter_for_closure = counter.clone();

    callbacks.invoke(move || {
        counter_for_closure.fetch_add(1, Ordering::Relaxed);
    });

    // Suppression must short-circuit BEFORE the closure reaches the
    // queue — `len` stays at zero even with `use_callback_thread=true`.
    assert_eq!(counter.load(Ordering::Relaxed), 0);
    assert_eq!(queue.len(), 0);
}

#[test]
fn use_callback_thread_accessor_returns_construction_argument() {
    let stop = Arc::new(AtomicBool::new(false));

    let (callbacks_inline, _) = make_callbacks(stop.clone(), false);
    let (callbacks_worker, _) = make_callbacks(stop, true);

    assert!(!callbacks_inline.use_callback_thread());
    assert!(callbacks_worker.use_callback_thread());
}

/*
    `notify_*` wrappers — payload reaches the listener
*/

#[test]
fn notify_candidate_dispatches_to_listener_inline() {
    let stop = Arc::new(AtomicBool::new(false));
    let (listener, listener_ptr) = make_listener();
    let (callbacks, _queue) = make_callbacks_with_listener(stop, false, listener_ptr);

    let root_hash = UInt256::from_slice(&[7u8; 32]);
    let decision_callback: ValidatorBlockCandidateDecisionCallback =
        Box::new(|_| { /* tests do not invoke the decision callback */ });

    callbacks.notify_candidate(
        make_test_source_info(),
        root_hash.clone(),
        ConsensusCommonFactory::create_empty_block_payload(),
        ConsensusCommonFactory::create_empty_block_payload(),
        decision_callback,
    );

    assert_eq!(listener.candidate_calls.load(Ordering::Relaxed), 1);
    assert_eq!(
        *listener.last_candidate_root_hash.lock().unwrap(),
        Some(root_hash),
        "candidate root_hash payload must reach the listener verbatim",
    );
}

#[test]
fn notify_candidate_observed_dispatches_to_listener_inline() {
    let stop = Arc::new(AtomicBool::new(false));
    let (listener, listener_ptr) = make_listener();
    let (callbacks, _queue) = make_callbacks_with_listener(stop, false, listener_ptr);

    let block_id = make_test_block_id(7);
    let flags =
        CandidateObservedFlags { body_present: true, parent_ready: true, local_collated: false };

    callbacks.notify_candidate_observed(
        block_id.clone(),
        ConsensusCommonFactory::create_empty_block_payload(),
        ConsensusCommonFactory::create_empty_block_payload(),
        flags,
    );

    assert_eq!(listener.candidate_observed_calls.load(Ordering::Relaxed), 1);
    assert_eq!(*listener.last_candidate_observed_block_id.lock().unwrap(), Some(block_id));
    assert_eq!(*listener.last_candidate_observed_flags.lock().unwrap(), Some(flags));
}

#[test]
fn notify_generate_slot_dispatches_to_listener_inline() {
    let stop = Arc::new(AtomicBool::new(false));
    let (listener, listener_ptr) = make_listener();
    let (callbacks, _queue) = make_callbacks_with_listener(stop, false, listener_ptr);

    let request: AsyncCollationRequestPtr = Arc::new(DummyAsyncRequest);
    let parent = CollationParentHint::Implicit;
    let generation_callback: ValidatorBlockCandidateCallback =
        Box::new(|_: consensus_common::Result<consensus_common::ValidatorBlockCandidatePtr>| {});

    callbacks.notify_generate_slot(make_test_source_info(), request, parent, generation_callback);

    assert_eq!(listener.generate_slot_calls.load(Ordering::Relaxed), 1);
    assert!(matches!(
        *listener.last_generate_slot_parent.lock().unwrap(),
        Some(CollationParentHint::Implicit)
    ));
}

#[test]
fn notify_block_finalized_dispatches_to_listener_inline() {
    let stop = Arc::new(AtomicBool::new(false));
    let (listener, listener_ptr) = make_listener();
    let (callbacks, _queue) = make_callbacks_with_listener(stop, false, listener_ptr);

    let block_id = make_test_block_id(11);
    let root_hash = UInt256::from_slice(&[1u8; 32]);
    let file_hash = UInt256::from_slice(&[2u8; 32]);

    callbacks.notify_block_finalized(
        block_id.clone(),
        make_test_source_info(),
        root_hash,
        file_hash,
        ConsensusCommonFactory::create_empty_block_payload(),
        make_test_signatures(),
        Vec::new(),
    );

    assert_eq!(listener.block_finalized_calls.load(Ordering::Relaxed), 1);
    assert_eq!(*listener.last_block_finalized_id.lock().unwrap(), Some(block_id));
}

#[test]
fn notify_candidate_drops_silently_when_listener_dropped_before_dispatch_inline() {
    let stop = Arc::new(AtomicBool::new(false));

    // Build a listener and immediately drop the strong reference — the
    // `Weak` is left dangling, then handed to the callbacks aspect.
    let listener_ptr: SessionListenerPtr = {
        let listener: Arc<RecordingListener> = Arc::new(RecordingListener::default());
        Arc::downgrade(&listener) as SessionListenerPtr
        // `listener` drops here.
    };
    let (callbacks, _queue) = make_callbacks_with_listener(stop, false, listener_ptr);

    // Must not panic — closure body checks `self.listener.upgrade()` and
    // exits when the upgrade fails. Nothing observable to assert besides
    // "no panic" because there is no listener left to record into.
    callbacks.notify_candidate(
        make_test_source_info(),
        UInt256::default(),
        ConsensusCommonFactory::create_empty_block_payload(),
        ConsensusCommonFactory::create_empty_block_payload(),
        Box::new(|_| {}),
    );
}

#[test]
fn notify_block_finalized_drops_silently_when_listener_dropped_before_dispatch_worker() {
    let stop = Arc::new(AtomicBool::new(false));

    let listener_ptr: SessionListenerPtr = {
        let listener: Arc<RecordingListener> = Arc::new(RecordingListener::default());
        Arc::downgrade(&listener) as SessionListenerPtr
    };
    let (callbacks, queue) =
        make_callbacks_with_listener(stop, /* use_callback_thread */ true, listener_ptr);

    callbacks.notify_block_finalized(
        make_test_block_id(1),
        make_test_source_info(),
        UInt256::default(),
        UInt256::default(),
        ConsensusCommonFactory::create_empty_block_payload(),
        make_test_signatures(),
        Vec::new(),
    );

    // The closure was posted (suppression gate is for `stop_flag`, not for
    // expired listeners). Draining must run it without panicking.
    assert_eq!(queue.len(), 1);
    queue.drain_and_run();
}

/*
    `Debug` impl smoke test
*/

#[test]
fn debug_impl_reports_current_stop_flag_value() {
    let stop = Arc::new(AtomicBool::new(false));
    let (callbacks, _queue) = make_callbacks(stop.clone(), true);

    let formatted_before = format!("{:?}", callbacks);
    assert!(formatted_before.contains("SessionCallbacks"));
    assert!(formatted_before.contains("use_callback_thread"));
    assert!(formatted_before.contains("stop_flag: false"));

    stop.store(true, Ordering::Relaxed);

    let formatted_after = format!("{:?}", callbacks);
    assert!(
        formatted_after.contains("stop_flag: true"),
        "Debug impl must reflect runtime stop_flag changes (got: {})",
        formatted_after
    );
}

/*
    Worker loop smoke test — exercises the relocated SXCB loop body.

    This is the post-move integration test required by the plan: it spawns
    `SessionCallbacks::run_worker_loop` on a dedicated thread (the same
    spawn site that `SessionImpl::create` uses when `use_callback_thread=true`),
    pushes a handful of closures, waits for them to execute, and joins the
    thread by setting `should_stop_flag`. The cleanup path stores `true`
    into `is_stopped_flag` and flushes the queue.
*/

#[test]
fn run_worker_loop_processes_queued_closures_then_exits_on_stop() {
    // Use the real crossbeam-backed queue so `pull_closure` behaves as it
    // does in production. The mock recorder is intentionally not used
    // here — the worker loop polls with a 100 ms timeout and we want true
    // FIFO pull semantics.
    let metrics_receiver = MetricsHandle::new(None);
    let queue = crate::session::SessionImpl::create_callback_task_queue(metrics_receiver.clone());

    let stop_flag = Arc::new(AtomicBool::new(false));
    // This test exercises only the worker-loop closure draining, never
    // listener delivery; a dangling listener handle suffices.
    let dead_listener: SessionListenerPtr = {
        let listener: Arc<RecordingListener> = Arc::new(RecordingListener::default());
        Arc::downgrade(&listener) as SessionListenerPtr
    };
    let callbacks = Arc::new(SessionCallbacks::new(
        SessionId::default(),
        stop_flag.clone(),
        /* use_callback_thread */ true,
        queue.clone(),
        dead_listener,
    ));

    // Post three closures that bump a shared counter when executed.
    let counter = Arc::new(AtomicUsize::new(0));
    for _ in 0..3 {
        let counter_for_closure = counter.clone();
        callbacks.invoke(move || {
            counter_for_closure.fetch_add(1, Ordering::Relaxed);
        });
    }

    // Spawn the worker loop on a side thread (mirrors `SessionImpl::create`).
    let should_stop_flag = Arc::new(AtomicBool::new(false));
    let is_stopped_flag = Arc::new(AtomicBool::new(false));
    let callbacks_for_thread = callbacks.clone();
    let should_stop_for_thread = should_stop_flag.clone();
    let is_stopped_for_thread = is_stopped_flag.clone();

    let worker = thread::Builder::new()
        .name("test-sxcb-worker".to_string())
        .spawn(move || {
            callbacks_for_thread.run_worker_loop(
                should_stop_for_thread,
                is_stopped_for_thread,
                metrics_receiver,
            );
        })
        .expect("worker thread must spawn");

    // Wait for the loop to drain the three queued closures. Bound the
    // wait so a regression on the relocated `pull_closure` path fails the
    // test instead of hanging forever.
    let deadline = Instant::now() + Duration::from_secs(5);
    while counter.load(Ordering::Relaxed) < 3 {
        if Instant::now() >= deadline {
            // Trigger the loop to exit so the thread can be joined cleanly
            // before we panic; otherwise the spawned thread leaks across
            // tests.
            should_stop_flag.store(true, Ordering::Relaxed);
            let _ = worker.join();
            panic!(
                "worker loop did not process posted closures within 5s (counter={})",
                counter.load(Ordering::Relaxed)
            );
        }
        thread::sleep(Duration::from_millis(10));
    }

    // Signal stop and join. The loop checks `should_stop_flag` at the top
    // of every iteration; the worst case wait is `MAX_TIMEOUT` (100 ms)
    // inside `pull_closure` plus drain time, so a 2-second wait covers
    // ample slack.
    should_stop_flag.store(true, Ordering::Relaxed);
    worker.join().expect("worker thread must join");

    assert_eq!(counter.load(Ordering::Relaxed), 3);
    assert!(
        is_stopped_flag.load(Ordering::Acquire),
        "is_stopped_flag must be set by run_worker_loop on clean exit",
    );
}
