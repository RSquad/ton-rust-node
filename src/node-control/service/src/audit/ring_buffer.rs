/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::audit::AuditEvent;
use parking_lot::RwLock;
use std::{collections::VecDeque, sync::Arc};

/// Fixed-capacity FIFO buffer of recent audit events for the REST read-path.
///
/// Populated on every [`crate::audit::log::AuditLog::record`] call — independent of the
/// JSONL writer task. Events are available immediately and remain visible even when the
/// writer queue is full.
///
/// Uses `parking_lot::RwLock` for a short, purely in-memory critical section — no I/O,
/// no `.await`, so a synchronous lock is appropriate (a `Mutex` would work equally well).
/// The lock is **never held across an `.await` point**.
pub struct AuditEventBuffer {
    inner: RwLock<VecDeque<AuditEvent>>,
    capacity: usize,
}

impl AuditEventBuffer {
    /// Creates a new buffer wrapped in `Arc`. `capacity` is the maximum number of events
    /// retained; when full, the oldest event is evicted (FIFO).
    pub fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            inner: RwLock::new(VecDeque::with_capacity(capacity.max(1))),
            capacity: capacity.max(1),
        })
    }

    /// Appends `event`, evicting the oldest entry when the buffer is at capacity.
    pub fn push(&self, event: AuditEvent) {
        let mut buf = self.inner.write();
        Self::push_locked(&mut buf, self.capacity, event);
    }

    /// Atomically checks deduplication and pushes under one write lock.
    ///
    /// Returns `false` when an event with the same [`AuditEvent::dedup_identity`] is already
    /// buffered (e.g. repeated `elections.stake_skipped` for the same node/election/reason).
    /// Returns `true` when the event was appended.
    pub fn push_unless_dedup_duplicate(&self, event: AuditEvent) -> bool {
        let mut buf = self.inner.write();
        if let Some(key) = event.dedup_identity()
            && buf.iter().any(|e| e.dedup_identity() == Some(key))
        {
            return false;
        }
        Self::push_locked(&mut buf, self.capacity, event);
        true
    }

    fn push_locked(buf: &mut VecDeque<AuditEvent>, capacity: usize, event: AuditEvent) {
        if buf.len() == capacity {
            buf.pop_front();
        }
        buf.push_back(event);
    }

    /// Returns a point-in-time snapshot of all buffered events (oldest first).
    pub fn snapshot(&self) -> Vec<AuditEvent> {
        self.inner.read().iter().cloned().collect()
    }

    /// Filters under the read lock — avoids cloning events that don't match `predicate`.
    pub fn filter_collect<F>(&self, predicate: F) -> Vec<AuditEvent>
    where
        F: Fn(&AuditEvent) -> bool,
    {
        self.inner.read().iter().filter(|e| predicate(e)).cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.inner.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{AuditActor, AuditEvent, AuditSource, StakeSkipReason};
    use std::sync::Arc;

    fn ev(tag: &str) -> AuditEvent {
        AuditEvent::system_service_started(tag)
    }

    // ── original tests ────────────────────────────────────────────────────────

    #[test]
    fn len_and_is_empty() {
        let buf = AuditEventBuffer::new(5);
        assert!(buf.is_empty());
        buf.push(ev("x"));
        assert_eq!(buf.len(), 1);
        assert!(!buf.is_empty());
    }

    // ── required new tests ────────────────────────────────────────────────────

    #[test]
    fn push_below_capacity_keeps_all() {
        let buf = AuditEventBuffer::new(5);
        for i in 0..4 {
            buf.push(ev(&format!("{i}")));
        }
        assert_eq!(buf.len(), 4);
        assert_eq!(buf.snapshot().len(), 4);
    }

    #[test]
    fn push_at_capacity_evicts_oldest() {
        let buf = AuditEventBuffer::new(3);
        let a = ev("a");
        let b = ev("b");
        let c = ev("c");
        let d = ev("d");
        let a_id = a.id;
        let d_id = d.id;
        buf.push(a);
        buf.push(b);
        buf.push(c);
        // at capacity, push d — must evict a
        buf.push(d);

        let snap = buf.snapshot();
        assert_eq!(snap.len(), 3, "len must stay at capacity");
        assert!(!snap.iter().any(|e| e.id == a_id), "oldest (a) must be evicted");
        assert!(snap.iter().any(|e| e.id == d_id), "newest (d) must be present");
    }

    #[test]
    fn snapshot_returns_in_order() {
        let buf = AuditEventBuffer::new(10);
        let ids: Vec<_> = (0..5)
            .map(|i| {
                let e = ev(&format!("{i}"));
                let id = e.id;
                buf.push(e);
                id
            })
            .collect();

        let snap = buf.snapshot();
        assert_eq!(snap.len(), 5);
        for (i, expected_id) in ids.iter().enumerate() {
            assert_eq!(&snap[i].id, expected_id, "event at index {i} is out of order");
        }
    }

    #[test]
    fn filter_collect_only_matching_events() {
        let buf = AuditEventBuffer::new(20);
        for _ in 0..5 {
            buf.push(ev("system"));
        }
        let matched = buf.filter_collect(|e| e.payload.source() == AuditSource::System);
        assert_eq!(matched.len(), 5);

        let unmatched = buf.filter_collect(|e| e.payload.source() == AuditSource::Elections);
        assert!(unmatched.is_empty());
    }

    #[test]
    fn concurrent_push_and_snapshot_no_panic() {
        let buf = Arc::new(AuditEventBuffer::new(50));
        let mut handles = vec![];

        // 8 producer threads each push 100 events
        for t in 0..8u8 {
            let b = buf.clone();
            handles.push(std::thread::spawn(move || {
                for i in 0..100u32 {
                    b.push(AuditEvent::system_service_started(&format!("t{t}-{i}")));
                }
            }));
        }

        // 1 reader thread continuously snapshots while producers are active
        let b = buf.clone();
        handles.push(std::thread::spawn(move || {
            for _ in 0..200 {
                let snap = b.snapshot();
                // capacity is 50 — snapshot must never exceed it
                assert!(snap.len() <= 50);
            }
        }));

        for h in handles {
            h.join().expect("thread panicked");
        }
        // Final state: buffer should be full (800 pushes into cap=50)
        assert_eq!(buf.len(), 50);
    }

    #[test]
    fn zero_capacity_buffer_silently_drops() {
        // capacity=0 is normalised to 1 internally; no crash, snapshot is non-empty after push
        let buf = AuditEventBuffer::new(0);
        assert_eq!(buf.capacity(), 1, "capacity normalised to 1");
        buf.push(ev("a"));
        buf.push(ev("b")); // evicts first, keeps second
        let snap = buf.snapshot();
        assert_eq!(snap.len(), 1, "only the latest event is retained");
    }

    #[test]
    fn push_unless_dedup_duplicate_keeps_one_per_node_election_reason() {
        let buf = AuditEventBuffer::new(10);
        let actor = AuditActor::service("elections-task");
        let election_id = 99;

        let first = AuditEvent::elections_stake_skipped(
            actor.clone(),
            "node0",
            election_id,
            StakeSkipReason::ElectionsDisabled,
            None,
            None,
        );
        let duplicate = AuditEvent::elections_stake_skipped(
            actor.clone(),
            "node0",
            election_id,
            StakeSkipReason::ElectionsDisabled,
            None,
            None,
        );
        let different_reason = AuditEvent::elections_stake_skipped(
            actor,
            "node0",
            election_id,
            StakeSkipReason::LowWalletBalance,
            None,
            None,
        );

        assert!(buf.push_unless_dedup_duplicate(first));
        assert!(!buf.push_unless_dedup_duplicate(duplicate));
        assert!(buf.push_unless_dedup_duplicate(different_reason));
        assert_eq!(buf.len(), 2);
    }
}
