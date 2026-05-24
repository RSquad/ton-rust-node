/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

/// Wall-clock abstraction. Production uses [`SystemClock`]; tests use [`MockClock`].
pub trait Clock: Send + Sync {
    /// Unix timestamp in seconds.
    fn now(&self) -> u64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> u64 {
        crate::time_format::now()
    }
}

#[derive(Clone, Default)]
pub struct MockClock {
    now: Arc<AtomicU64>,
}

impl MockClock {
    pub fn new(initial: u64) -> Self {
        Self { now: Arc::new(AtomicU64::new(initial)) }
    }

    pub fn set(&self, t: u64) {
        self.now.store(t, Ordering::Relaxed);
    }

    pub fn advance(&self, secs: u64) {
        self.now.fetch_add(secs, Ordering::Relaxed);
    }
}

impl Clock for MockClock {
    fn now(&self) -> u64 {
        self.now.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_clock_set_and_advance() {
        let clock = MockClock::new(100);
        assert_eq!(clock.now(), 100);
        clock.advance(50);
        assert_eq!(clock.now(), 150);
        clock.set(1000);
        assert_eq!(clock.now(), 1000);
    }

    #[test]
    fn mock_clock_clones_share_state() {
        let clock = MockClock::new(0);
        let clone = clock.clone();
        clock.set(42);
        assert_eq!(clone.now(), 42);
    }
}
