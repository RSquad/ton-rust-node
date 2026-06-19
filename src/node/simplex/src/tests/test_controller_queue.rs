/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Tests for the [`ControllerQueue`] task-posting seam.
//!
//! Included directly from `controller_queue.rs` via `#[path]` so they share its
//! module scope (`use super::*`) and reach its items without extra visibility.
//! They prove the seam end-to-end against a trivial stand-in controller, its
//! borrowing backend view, and a lock-free recording fake — with no
//! `SessionProcessor` anywhere.

use super::*;
use crossbeam::queue::SegQueue;
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

/// Backend view a `Probe` deferred task receives — the test analogue of
/// `dyn ValidationBackend`. The drain owner (here, the test) builds it *fresh
/// per task* and hands it over by `&mut`, mirroring how the production adapter
/// rebuilds a borrowing backend from `&mut SessionProcessor` on every re-entry.
trait ProbeBackend {
    fn tag(&self) -> u32;
    /// Mutating effect proving a task can drive backend state through the
    /// `&mut C::Backend` ref. The production analogue is a backend that mutates
    /// disjoint `SessionProcessor` fields during a re-entry (e.g. lowering the
    /// main-loop wake horizon, advancing the collation pipeline).
    fn bump(&mut self);
}

/// Concrete borrowing backend the test constructs fresh at each "drain". Holds a
/// `&mut` to shared backing state — the test analogue of a disjoint
/// `SessionProcessor` field the split-borrow adapter lends the task. Because a
/// fresh view is built per task, a mutation lands in the backing state and is
/// observed by the next task's freshly-built view (exactly as production
/// effects persist in `SessionProcessor`, not in the transient backend).
struct ProbeBackendView<'a> {
    store: &'a mut u32,
}
impl ProbeBackend for ProbeBackendView<'_> {
    fn tag(&self) -> u32 {
        *self.store
    }
    fn bump(&mut self) {
        *self.store += 1;
    }
}

/// Minimal stand-in controller: the seam is generic, so the mechanism is proven
/// without a real controller (which would need its full dependency fixture).
#[derive(Default)]
struct Probe {
    hits: Vec<u32>,
}

/// Declares the stand-in controller's borrowing backend view, exactly as a real
/// controller does (see `ValidationController`).
impl Controlled for Probe {
    type Backend<'b> = dyn ProbeBackend + 'b;
}

/// Recording [`ControllerQueue`] fake: enqueues posted tasks into lock-free
/// crossbeam queues instead of running them, so a test can replay them against
/// a `&mut C` it owns plus a backend view it builds. No mutex, no
/// `SessionProcessor` — this is exactly what a controller unit test substitutes
/// for the real session main loop.
struct RecordingQueue<C: Controlled> {
    immediate: SegQueue<ControllerTask<C>>,
    delayed: SegQueue<(SystemTime, ControllerTask<C>)>,
}

impl<C: Controlled> RecordingQueue<C> {
    /// Fresh recorder wrapped in the `Arc` controllers store.
    fn new() -> Arc<Self> {
        Arc::new(Self { immediate: SegQueue::new(), delayed: SegQueue::new() })
    }

    /// Number of immediate tasks recorded and not yet replayed.
    fn immediate_count(&self) -> usize {
        self.immediate.len()
    }

    /// Number of delayed tasks recorded and not yet replayed.
    fn delayed_count(&self) -> usize {
        self.delayed.len()
    }

    /// Drain every recorded immediate task (FIFO), handing each to `run_one`.
    ///
    /// `run_one` is the drain owner's hook to build a *fresh* borrowing backend
    /// and run the task against its `&mut C` — exactly what the production
    /// composition root does per re-entry. The backend ref is `&mut` and
    /// invariant in its lifetime, so a single view cannot be reused across the
    /// loop; rebuilding per task is both required and faithful.
    fn run_immediate(&self, mut run_one: impl FnMut(ControllerTask<C>)) {
        while let Some(task) = self.immediate.pop() {
            run_one(task);
        }
    }

    /// Drain every recorded delayed task (FIFO) via `run_one`, returning the
    /// scheduled `at` times in post order (timing itself is the runtime's
    /// concern, so the fake just replays).
    fn run_delayed(&self, mut run_one: impl FnMut(ControllerTask<C>)) -> Vec<SystemTime> {
        let mut times = Vec::new();
        while let Some((at, task)) = self.delayed.pop() {
            times.push(at);
            run_one(task);
        }
        times
    }
}

impl<C: Controlled> ControllerQueue<C> for RecordingQueue<C> {
    fn post_boxed(&self, task: ControllerTask<C>) {
        self.immediate.push(task);
    }

    fn post_delayed_boxed(&self, at: SystemTime, task: ControllerTask<C>) {
        self.delayed.push((at, task));
    }
}

#[test]
fn post_records_then_replays_against_target_in_order() {
    let queue = RecordingQueue::<Probe>::new();

    queue.post(|p: &mut Probe, _b| p.hits.push(1));
    queue.post(|p: &mut Probe, _b| p.hits.push(2));
    assert_eq!(queue.immediate_count(), 2);

    let mut probe = Probe::default();
    assert!(probe.hits.is_empty(), "tasks must not run until replayed");

    let mut store = 0;
    queue.run_immediate(|task| {
        let mut backend = ProbeBackendView { store: &mut store };
        task(&mut probe, &mut backend);
    });
    assert_eq!(probe.hits, vec![1, 2], "tasks replay in post order against &mut target");
    assert_eq!(queue.immediate_count(), 0, "replay drains the recorder");
}

#[test]
fn post_delayed_records_timestamp_and_replays() {
    let queue = RecordingQueue::<Probe>::new();
    let at = SystemTime::UNIX_EPOCH + Duration::from_secs(42);

    queue.post_delayed(at, |p: &mut Probe, _b| p.hits.push(9));
    assert_eq!(queue.delayed_count(), 1);

    let mut probe = Probe::default();
    let mut store = 0;
    let times = queue.run_delayed(|task| {
        let mut backend = ProbeBackendView { store: &mut store };
        task(&mut probe, &mut backend);
    });
    assert_eq!(times, vec![at], "delayed posts retain their target time");
    assert_eq!(probe.hits, vec![9]);
    assert_eq!(queue.delayed_count(), 0);
}

#[test]
fn backend_view_is_delivered_to_each_task() {
    // The borrowing backend the drain builds is handed to every task; a task
    // reads through it exactly as a real controller reads `dyn ValidationBackend`.
    let queue = RecordingQueue::<Probe>::new();
    queue.post(|p: &mut Probe, b: &mut dyn ProbeBackend| p.hits.push(b.tag()));
    queue.post(|p: &mut Probe, b: &mut dyn ProbeBackend| p.hits.push(b.tag() + 1));

    let mut probe = Probe::default();
    let mut store = 100;
    queue.run_immediate(|task| {
        let mut backend = ProbeBackendView { store: &mut store };
        task(&mut probe, &mut backend);
    });
    assert_eq!(probe.hits, vec![100, 101], "each task observes the supplied backend view");
}

#[test]
fn task_can_mutate_through_backend_ref() {
    // The backend ref handed to a task is `&mut`, so a task can drive backend
    // *effects*, not just reads. This mirrors a production backend that mutates
    // disjoint `SessionProcessor` state during a re-entry: the effect lands in
    // the backing state (here `store`), so the next task's freshly-built view
    // observes it — just as production effects persist in `SessionProcessor`.
    let queue = RecordingQueue::<Probe>::new();
    queue.post(|_p: &mut Probe, b: &mut dyn ProbeBackend| b.bump());
    queue.post(|p: &mut Probe, b: &mut dyn ProbeBackend| p.hits.push(b.tag()));

    let mut probe = Probe::default();
    let mut store = 10;
    queue.run_immediate(|task| {
        let mut backend = ProbeBackendView { store: &mut store };
        task(&mut probe, &mut backend);
    });

    assert_eq!(store, 11, "task mutated the backing state through the &mut backend ref");
    assert_eq!(probe.hits, vec![11], "a later task's fresh view observes the mutation");
}

#[test]
fn handle_is_send_sync_and_posts_back_from_another_thread() {
    // Mirrors an off-thread async callback that holds a clone and posts a
    // controller-targeting closure back to the main loop.
    let queue = RecordingQueue::<Probe>::new();
    let _coerces: ControllerQueuePtr<Probe> = queue.clone();

    let worker = queue.clone();
    std::thread::spawn(move || worker.post(|p: &mut Probe, _b| p.hits.push(7)))
        .join()
        .expect("worker thread must not panic");

    let mut probe = Probe::default();
    let mut store = 0;
    queue.run_immediate(|task| {
        let mut backend = ProbeBackendView { store: &mut store };
        task(&mut probe, &mut backend);
    });
    assert_eq!(probe.hits, vec![7]);
}
