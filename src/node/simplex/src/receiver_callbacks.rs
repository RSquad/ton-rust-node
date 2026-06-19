/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! # `ReceiverCallbacks` adapter
//!
//! Per-session network-callback adapter. Lifts `SXRCV` (`Receiver`)
//! network events onto the `SXMAIN` main task queue so that all
//! [`SessionProcessor`] state mutations happen on a single thread.
//!
//! ## Symmetry with [`SessionCallbacks`](crate::session_callbacks::SessionCallbacks)
//!
//! ```text
//!  SXRCV (Receiver)  ──ReceiverCallbacks──►  SXMAIN (SessionProcessor)
//!  SXMAIN (consensus) ──SessionCallbacks──►  SXCB   (SessionListener)
//! ```
//!
//! [`SessionCallbacks`](crate::session_callbacks::SessionCallbacks)
//! owns the outbound edge (consensus → listener via the SXCB worker);
//! this module owns the inbound edge (network → processor via the
//! SXMAIN main task queue). The two aspects never call each other.
//!
//! ## Boundary
//!
//! `ReceiverCallbacks` implements [`ReceiverListener`] and posts
//! closures onto the main task queue. It never mutates session state
//! directly: each callback method serialises into a single
//! `processor.on_X(...)` invocation that runs on the SXMAIN thread.
//!
//! ## Current state
//!
//! Naming-only adapter today (verbatim move of the prior
//! `ReceiverListenerImpl` from `session.rs`). Future receiver-runtime
//! work may split bounds / dedup / rate-limit / signature-ban gating
//! into a dedicated `IngressGate` aspect; that work is intentionally
//! out of scope here.

use crate::{
    receiver::{ReceiverActivitySnapshot, ReceiverListener, StandstillTriggerNotification},
    session_processor::SessionProcessor,
    task_queue::TaskQueuePtr,
    RawVoteData, SessionId, ValidatorWeight,
};
use std::{sync::Arc, time::SystemTime};
use ton_api::ton::consensus::{
    simplex::{Certificate, Vote},
    CandidateData,
};
use ton_block::UInt256;

/// Per-session network-callback adapter (`SXRCV` → `SXMAIN`).
///
/// See module documentation for the lifecycle picture and the
/// symmetric relationship with
/// [`SessionCallbacks`](crate::session_callbacks::SessionCallbacks).
pub(crate) struct ReceiverCallbacks {
    /// Main task queue (`SXMAIN`). Network events are posted here as
    /// closures so they run single-threaded against `SessionProcessor`.
    task_queue: TaskQueuePtr,
    /// Session identifier; used purely for the drop-log diagnostic.
    session_id: SessionId,
}

// ======================================================================
// Construction & teardown
// ======================================================================
// Build the `Arc<ReceiverCallbacks>` handle bound to the main task queue;
// `Drop` only logs.
impl ReceiverCallbacks {
    /// Create a fresh `Arc<ReceiverCallbacks>` bound to the given main
    /// task queue.
    pub(crate) fn create(task_queue: TaskQueuePtr, session_id: SessionId) -> Arc<Self> {
        Arc::new(Self { task_queue, session_id })
    }
}

impl Drop for ReceiverCallbacks {
    fn drop(&mut self) {
        log::debug!("Dropped ReceiverCallbacks for session {}", self.session_id.to_hex_string());
    }
}

// ======================================================================
// ReceiverListener — SXRCV → SXMAIN bridge
// ======================================================================
// Each inbound network event is serialised into a single
// `processor.on_X(...)` closure posted onto the main task queue, so all
// state mutation happens on the SXMAIN thread.
impl ReceiverListener for ReceiverCallbacks {
    /// Handle incoming vote from the network
    fn on_vote(&self, source_idx: u32, vote: Vote, raw_vote: RawVoteData) {
        self.task_queue.post_closure(Box::new(move |processor: &mut SessionProcessor| {
            processor.on_vote(source_idx, vote, raw_vote);
        }));
    }

    /// Handle incoming block candidate (from broadcast or query response)
    fn on_candidate_received(
        &self,
        source_idx: u32,
        candidate: CandidateData,
        notar_cert: Option<Vec<u8>>,
    ) {
        self.task_queue.post_closure(Box::new(move |processor: &mut SessionProcessor| {
            processor.on_candidate_received(source_idx, candidate, notar_cert);
        }));
    }

    fn on_candidate_notar_received(
        &self,
        source_idx: u32,
        slot: crate::block::SlotIndex,
        block_hash: UInt256,
        notar_cert: Vec<u8>,
    ) {
        self.task_queue.post_closure(Box::new(move |processor: &mut SessionProcessor| {
            processor.on_candidate_notar_received(source_idx, slot, block_hash, notar_cert);
        }));
    }

    /// Handle activity updates from the receiver
    fn on_activity(
        &self,
        active_weight: ValidatorWeight,
        last_activity: Vec<Option<SystemTime>>,
        snapshot: ReceiverActivitySnapshot,
    ) {
        self.task_queue.post_closure(Box::new(move |processor: &mut SessionProcessor| {
            processor.on_activity(active_weight, last_activity, snapshot);
        }));
    }

    fn on_standstill_trigger(&self, notification: StandstillTriggerNotification) {
        self.task_queue.post_closure(Box::new(move |processor: &mut SessionProcessor| {
            processor.on_standstill_trigger(notification);
        }));
    }

    /// Handle incoming certificate from network
    fn on_certificate(&self, source_idx: u32, certificate: Certificate) {
        self.task_queue.post_closure(Box::new(move |processor: &mut SessionProcessor| {
            processor.on_certificate(source_idx, certificate);
        }));
    }

    /// Handle RequestCandidate cache miss by delegating to SessionProcessor
    fn on_candidate_query_fallback(
        &self,
        slot: crate::block::SlotIndex,
        block_hash: UInt256,
        want_candidate: bool,
        want_notar: bool,
        response_callback: consensus_common::QueryResponseCallback,
    ) {
        self.task_queue.post_closure(Box::new(move |processor: &mut SessionProcessor| {
            processor.handle_candidate_query_fallback(
                slot,
                block_hash,
                want_candidate,
                want_notar,
                response_callback,
            );
        }));
    }
}
