/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Unit tests for LossyOverlay network impairment simulation.

use super::*;
use crate::ConsensusCommonFactory;
use std::sync::atomic::{AtomicU32, Ordering};
use ton_block::Ed25519KeyOption;

/// Simple test listener that counts received packets.
struct CountingListener {
    messages_received: AtomicU32,
    broadcasts_received: AtomicU32,
    queries_received: AtomicU32,
}

impl CountingListener {
    fn new() -> Self {
        Self {
            messages_received: AtomicU32::new(0),
            broadcasts_received: AtomicU32::new(0),
            queries_received: AtomicU32::new(0),
        }
    }
}

impl ConsensusOverlayListener for CountingListener {
    fn on_message(&self, _adnl_id: PublicKeyHash, _data: &BlockPayloadPtr) {
        self.messages_received.fetch_add(1, Ordering::SeqCst);
    }

    fn on_broadcast(&self, _source_key_hash: PublicKeyHash, _data: &BlockPayloadPtr) {
        self.broadcasts_received.fetch_add(1, Ordering::SeqCst);
    }

    fn on_query(
        &self,
        _adnl_id: PublicKeyHash,
        _data: &BlockPayloadPtr,
        callback: QueryResponseCallback,
    ) {
        self.queries_received.fetch_add(1, Ordering::SeqCst);
        callback(Ok(ConsensusCommonFactory::create_block_payload(vec![42])));
    }
}

fn create_test_key() -> PrivateKey {
    Ed25519KeyOption::generate().expect("Failed to generate key")
}

fn create_test_data() -> BlockPayloadPtr {
    ConsensusCommonFactory::create_block_payload(vec![1, 2, 3, 4])
}

#[test]
fn test_lossy_overlay_opts_default_is_passthrough() {
    let opts = LossyOverlayOpts::default();

    assert_eq!(opts.lost_broadcast_probability, 0.0);
    assert_eq!(opts.lost_message_probability, 0.0);
    assert_eq!(opts.lost_query_probability, 0.0);
    assert_eq!(opts.delay_broadcast_ms_min, 0);
    assert_eq!(opts.delay_broadcast_ms_max, 0);
    assert_eq!(opts.delay_message_ms_min, 0);
    assert_eq!(opts.delay_message_ms_max, 0);
    assert_eq!(opts.delay_query_ms_min, 0);
    assert_eq!(opts.delay_query_ms_max, 0);
}

#[test]
fn test_lossy_overlay_listener_broadcast_passthrough() {
    let inner = Arc::new(CountingListener::new());
    let inner_weak = Arc::downgrade(&inner);

    let lossy = LossyOverlayListener {
        inner: inner_weak,
        opts: LossyOverlayOpts::default(),
        local_id: create_test_key(),
    };

    let sender_id = create_test_key().id().clone();
    let data = create_test_data();

    for _ in 0..10 {
        lossy.on_broadcast(sender_id.clone(), &data);
    }

    assert_eq!(inner.broadcasts_received.load(Ordering::SeqCst), 10);
}

#[test]
fn test_lossy_overlay_listener_message_passthrough() {
    let inner = Arc::new(CountingListener::new());
    let inner_weak = Arc::downgrade(&inner);

    let lossy = LossyOverlayListener {
        inner: inner_weak,
        opts: LossyOverlayOpts::default(),
        local_id: create_test_key(),
    };

    let sender_id = create_test_key().id().clone();
    let data = create_test_data();

    for _ in 0..10 {
        lossy.on_message(sender_id.clone(), &data);
    }

    assert_eq!(inner.messages_received.load(Ordering::SeqCst), 10);
}

#[test]
fn test_lossy_overlay_listener_query_passthrough() {
    let inner = Arc::new(CountingListener::new());
    let inner_weak = Arc::downgrade(&inner);

    let lossy = LossyOverlayListener {
        inner: inner_weak,
        opts: LossyOverlayOpts::default(),
        local_id: create_test_key(),
    };

    let sender_id = create_test_key().id().clone();
    let data = create_test_data();
    let responses_received = Arc::new(AtomicU32::new(0));

    for _ in 0..10 {
        let responses = responses_received.clone();
        lossy.on_query(
            sender_id.clone(),
            &data,
            Box::new(move |result| {
                if result.is_ok() {
                    responses.fetch_add(1, Ordering::SeqCst);
                }
            }),
        );
    }

    assert_eq!(inner.queries_received.load(Ordering::SeqCst), 10);
    assert_eq!(responses_received.load(Ordering::SeqCst), 10);
}

#[test]
fn test_lossy_overlay_listener_broadcast_100_percent_loss() {
    let inner = Arc::new(CountingListener::new());
    let inner_weak = Arc::downgrade(&inner);

    let lossy = LossyOverlayListener {
        inner: inner_weak,
        opts: LossyOverlayOpts { lost_broadcast_probability: 1.0, ..Default::default() },
        local_id: create_test_key(),
    };

    let sender_id = create_test_key().id().clone();
    let data = create_test_data();

    for _ in 0..10 {
        lossy.on_broadcast(sender_id.clone(), &data);
    }

    assert_eq!(inner.broadcasts_received.load(Ordering::SeqCst), 0);
}

#[test]
fn test_lossy_overlay_listener_message_100_percent_loss() {
    let inner = Arc::new(CountingListener::new());
    let inner_weak = Arc::downgrade(&inner);

    let lossy = LossyOverlayListener {
        inner: inner_weak,
        opts: LossyOverlayOpts { lost_message_probability: 1.0, ..Default::default() },
        local_id: create_test_key(),
    };

    let sender_id = create_test_key().id().clone();
    let data = create_test_data();

    for _ in 0..10 {
        lossy.on_message(sender_id.clone(), &data);
    }

    assert_eq!(inner.messages_received.load(Ordering::SeqCst), 0);
}

#[test]
fn test_lossy_overlay_listener_query_100_percent_loss() {
    let inner = Arc::new(CountingListener::new());
    let inner_weak = Arc::downgrade(&inner);

    let lossy = LossyOverlayListener {
        inner: inner_weak,
        opts: LossyOverlayOpts { lost_query_probability: 1.0, ..Default::default() },
        local_id: create_test_key(),
    };

    let sender_id = create_test_key().id().clone();
    let data = create_test_data();
    let errors_received = Arc::new(AtomicU32::new(0));

    for _ in 0..10 {
        let errors = errors_received.clone();
        lossy.on_query(
            sender_id.clone(),
            &data,
            Box::new(move |result| {
                if result.is_err() {
                    errors.fetch_add(1, Ordering::SeqCst);
                }
            }),
        );
    }

    assert_eq!(inner.queries_received.load(Ordering::SeqCst), 0);
    assert_eq!(errors_received.load(Ordering::SeqCst), 10);
}

#[test]
fn test_lossy_overlay_listener_broadcast_delay() {
    use std::time::Instant;

    let inner = Arc::new(CountingListener::new());
    let inner_weak = Arc::downgrade(&inner);

    let lossy = LossyOverlayListener {
        inner: inner_weak,
        opts: LossyOverlayOpts {
            delay_broadcast_ms_min: 50,
            delay_broadcast_ms_max: 100,
            ..Default::default()
        },
        local_id: create_test_key(),
    };

    let sender_id = create_test_key().id().clone();
    let data = create_test_data();

    let start = Instant::now();
    lossy.on_broadcast(sender_id, &data);

    // Wait for delayed delivery
    std::thread::sleep(std::time::Duration::from_millis(150));

    let elapsed = start.elapsed();

    assert_eq!(inner.broadcasts_received.load(Ordering::SeqCst), 1);
    assert!(elapsed.as_millis() >= 50);
}
