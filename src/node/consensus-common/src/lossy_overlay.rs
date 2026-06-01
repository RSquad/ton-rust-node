/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! # Lossy Overlay for Network Simulation
//!
//! This module provides wrappers around `ConsensusOverlayManager` and `ConsensusOverlayListener`
//! that simulate network impairments:
//!
//! - **Packet loss**: Randomly drop broadcasts, messages, or queries with configurable probability
//! - **Latency**: Add random delays to broadcasts, messages, or query responses
//!
//! ## Usage
//!
//! ```ignore
//! use consensus_common::{LossyOverlayManager, LossyOverlayOpts};
//!
//! let opts = LossyOverlayOpts {
//!     // Broadcast impairment (votes, certificates, candidates)
//!     lost_broadcast_probability: 0.1,  // 10% packet loss
//!     delay_broadcast_ms_min: 10,
//!     delay_broadcast_ms_max: 100,
//!
//!     // Message impairment (point-to-point)
//!     lost_message_probability: 0.05,   // 5% packet loss
//!     delay_message_ms_min: 0,
//!     delay_message_ms_max: 50,
//!
//!     // Query impairment (requestCandidate, etc.)
//!     lost_query_probability: 0.02,     // 2% drop (returns error)
//!     delay_query_ms_min: 0,
//!     delay_query_ms_max: 200,
//! };
//!
//! let lossy_manager = LossyOverlayManager::create(inner_manager, opts);
//! ```
//!
//! ## Notes
//!
//! - For queries, "loss" means the query handler is not invoked and an error is returned
//! - All delays spawn a thread to sleep before forwarding, which may be resource-intensive
//! - This is intended for testing purposes only

use super::{
    BlockPayloadPtr, ConsensusNode, ConsensusOverlayListener, ConsensusOverlayListenerPtr,
    ConsensusOverlayLogReplayListenerPtr, ConsensusOverlayManager, ConsensusOverlayManagerPtr,
    ConsensusOverlayPtr, OverlayTransportType, PrivateKey, PrivateOverlayShortId, PublicKeyHash,
    QueryResponseCallback,
};
use rand::Rng;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    thread,
};
use ton_block::Result;

/// Configuration options for lossy overlay network simulation.
///
/// All probabilities are in range [0.0, 1.0] where 0.0 means no loss and 1.0 means 100% loss.
/// Delay ranges are in milliseconds; if min == max == 0, no delay is applied.
#[derive(Clone, Debug)]
pub struct LossyOverlayOpts {
    // --- Broadcast impairment (votes, certificates, candidate broadcasts) ---
    /// Probability of dropping a broadcast (0.0 = no loss, 1.0 = 100% loss)
    pub lost_broadcast_probability: f64,
    /// Minimum delay before forwarding a broadcast (ms)
    pub delay_broadcast_ms_min: u64,
    /// Maximum delay before forwarding a broadcast (ms). If 0, no delay is applied.
    pub delay_broadcast_ms_max: u64,

    // --- Message impairment (point-to-point messages) ---
    /// Probability of dropping a message (0.0 = no loss, 1.0 = 100% loss)
    pub lost_message_probability: f64,
    /// Minimum delay before forwarding a message (ms)
    pub delay_message_ms_min: u64,
    /// Maximum delay before forwarding a message (ms). If 0, no delay is applied.
    pub delay_message_ms_max: u64,

    // --- Query impairment (requestCandidate, etc.) ---
    /// Probability of dropping a query (0.0 = no loss, 1.0 = 100% loss).
    /// When dropped, an error response is returned.
    pub lost_query_probability: f64,
    /// Minimum delay before processing a query (ms)
    pub delay_query_ms_min: u64,
    /// Maximum delay before processing a query (ms). If 0, no delay is applied.
    pub delay_query_ms_max: u64,
}

impl Default for LossyOverlayOpts {
    /// Returns default options with no loss and no delay (pass-through behavior).
    fn default() -> Self {
        Self {
            lost_broadcast_probability: 0.0,
            delay_broadcast_ms_min: 0,
            delay_broadcast_ms_max: 0,
            lost_message_probability: 0.0,
            delay_message_ms_min: 0,
            delay_message_ms_max: 0,
            lost_query_probability: 0.0,
            delay_query_ms_min: 0,
            delay_query_ms_max: 0,
        }
    }
}

/// Wrapper around ConsensusOverlayListener that applies loss and delay to packets.
struct LossyOverlayListener {
    /// The underlying listener to forward packets to.
    inner: ConsensusOverlayListenerPtr,
    /// Loss and delay options.
    opts: LossyOverlayOpts,
    /// Local node ID for logging.
    local_id: PrivateKey,
}

impl LossyOverlayListener {
    /// Helper to compute a random delay within the specified range.
    fn compute_delay(min_ms: u64, max_ms: u64) -> std::time::Duration {
        debug_assert!(min_ms <= max_ms, "delay min ({min_ms}) must be <= max ({max_ms})");
        if max_ms == 0 {
            std::time::Duration::ZERO
        } else {
            let (lo, hi) = if min_ms <= max_ms { (min_ms, max_ms) } else { (max_ms, min_ms) };
            let mut rng = rand::thread_rng();
            let range = (hi - lo) as f64;
            let delay_ms = lo + (rng.gen::<f64>() * range) as u64;
            std::time::Duration::from_millis(delay_ms)
        }
    }

    /// Helper to check if a packet should be dropped based on probability.
    fn should_drop(probability: f64) -> bool {
        if probability <= 0.0 {
            false
        } else {
            let mut rng = rand::thread_rng();
            rng.gen::<f64>() < probability
        }
    }
}

impl ConsensusOverlayListener for LossyOverlayListener {
    fn on_message(&self, adnl_id: PublicKeyHash, data: &BlockPayloadPtr) {
        // Check for message loss
        if Self::should_drop(self.opts.lost_message_probability) {
            log::trace!(
                "LossyOverlayListener {}: dropping message from {} (probability: {})",
                self.local_id.id(),
                adnl_id,
                self.opts.lost_message_probability
            );
            return;
        }

        let delay =
            Self::compute_delay(self.opts.delay_message_ms_min, self.opts.delay_message_ms_max);

        if delay.is_zero() {
            // No delay, forward immediately
            if let Some(inner) = self.inner.upgrade() {
                inner.on_message(adnl_id, data);
            }
        } else {
            // Apply delay
            log::trace!(
                "LossyOverlayListener {}: delaying message from {} by {:?}",
                self.local_id.id(),
                adnl_id,
                delay
            );

            let inner = self.inner.clone();
            let data = data.clone();
            let selfid = self.local_id.clone();
            std::thread::spawn(move || {
                thread::sleep(delay);
                if let Some(inner) = inner.upgrade() {
                    inner.on_message(adnl_id, &data);
                } else {
                    log::trace!(
                        "LossyOverlayListener {}: message from {} dropped (listener gone after {:?} delay)",
                        selfid.id(),
                        adnl_id,
                        delay
                    );
                }
            });
        }
    }

    fn on_broadcast(
        &self,
        source_key_hash: PublicKeyHash,
        data: &BlockPayloadPtr,
        source: crate::BroadcastSource,
    ) {
        // Check for broadcast loss
        if Self::should_drop(self.opts.lost_broadcast_probability) {
            log::trace!(
                "LossyOverlayListener {}: dropping broadcast from {} (probability: {})",
                self.local_id.id(),
                source_key_hash,
                self.opts.lost_broadcast_probability
            );
            return;
        }

        let delay =
            Self::compute_delay(self.opts.delay_broadcast_ms_min, self.opts.delay_broadcast_ms_max);

        if delay.is_zero() {
            // No delay, forward immediately
            if let Some(inner) = self.inner.upgrade() {
                inner.on_broadcast(source_key_hash, data, source);
            }
        } else {
            // Apply delay
            log::trace!(
                "LossyOverlayListener {}: delaying broadcast from {} by {:?}",
                self.local_id.id(),
                source_key_hash,
                delay
            );

            let inner = self.inner.clone();
            let data = data.clone();
            let selfid = self.local_id.clone();
            std::thread::spawn(move || {
                thread::sleep(delay);
                if let Some(inner) = inner.upgrade() {
                    inner.on_broadcast(source_key_hash, &data, source);
                } else {
                    log::trace!(
                        "LossyOverlayListener {}: broadcast from {} dropped (listener gone after {:?} delay)",
                        selfid.id(),
                        source_key_hash,
                        delay
                    );
                }
            });
        }
    }

    fn on_query(
        &self,
        adnl_id: PublicKeyHash,
        data: &BlockPayloadPtr,
        response_callback: QueryResponseCallback,
    ) {
        // Check for query loss
        if Self::should_drop(self.opts.lost_query_probability) {
            log::trace!(
                "LossyOverlayListener {}: dropping query from {} (probability: {})",
                self.local_id.id(),
                adnl_id,
                self.opts.lost_query_probability
            );
            // Return an error to the caller
            response_callback(Err(ton_block::error!("LossyOverlay: simulated query drop")));
            return;
        }

        let delay = Self::compute_delay(self.opts.delay_query_ms_min, self.opts.delay_query_ms_max);

        if delay.is_zero() {
            // No delay, forward immediately
            if let Some(inner) = self.inner.upgrade() {
                inner.on_query(adnl_id, data, response_callback);
            } else {
                response_callback(Err(ton_block::error!("LossyOverlay: inner listener gone")));
            }
        } else {
            // Apply delay
            log::trace!(
                "LossyOverlayListener {}: delaying query from {} by {:?}",
                self.local_id.id(),
                adnl_id,
                delay
            );

            let inner = self.inner.clone();
            let data = data.clone();
            let selfid = self.local_id.clone();
            std::thread::spawn(move || {
                thread::sleep(delay);
                if let Some(inner) = inner.upgrade() {
                    inner.on_query(adnl_id, &data, response_callback);
                } else {
                    log::trace!(
                        "LossyOverlayListener {}: query from {} dropped (listener gone after {:?} delay)",
                        selfid.id(),
                        adnl_id,
                        delay
                    );
                    response_callback(Err(ton_block::error!(
                        "LossyOverlay: inner listener gone after delay"
                    )));
                }
            });
        }
    }
}

/// Wrapper around ConsensusOverlayManager that wraps listeners with lossy behavior.
///
/// Keeps strong references to lossy listeners so the weak refs passed to the
/// inner overlay manager remain valid for the overlay's lifetime.
pub struct LossyOverlayManager {
    inner: ConsensusOverlayManagerPtr,
    opts: LossyOverlayOpts,
    /// Retains lossy listener Arcs so the weak refs handed to the inner manager stay alive.
    /// Cleaned up in stop_overlay.
    listeners: Mutex<HashMap<Arc<PrivateOverlayShortId>, Arc<LossyOverlayListener>>>,
}

impl LossyOverlayManager {
    pub fn create(
        inner: ConsensusOverlayManagerPtr,
        opts: LossyOverlayOpts,
    ) -> ConsensusOverlayManagerPtr {
        Arc::new(Self { inner, opts, listeners: Mutex::new(HashMap::new()) })
    }
}

impl ConsensusOverlayManager for LossyOverlayManager {
    fn start_overlay(
        &self,
        local_id: &PrivateKey,
        overlay_short_id: &Arc<PrivateOverlayShortId>,
        nodes: &[ConsensusNode],
        overlay_listener: ConsensusOverlayListenerPtr,
        log_replay_listener: ConsensusOverlayLogReplayListenerPtr,
        transport_type: OverlayTransportType,
        block_sync_params: Option<crate::BlockSyncOverlayParams>,
    ) -> Result<ConsensusOverlayPtr> {
        let lossy_listener = Arc::new(LossyOverlayListener {
            inner: overlay_listener,
            opts: self.opts.clone(),
            local_id: local_id.clone(),
        });
        let lossy_listener_weak = Arc::downgrade(&lossy_listener);

        let overlay = self.inner.start_overlay(
            local_id,
            overlay_short_id,
            nodes,
            lossy_listener_weak,
            log_replay_listener,
            transport_type,
            block_sync_params,
        )?;

        self.listeners
            .lock()
            .expect("LossyOverlayManager: listeners lock poisoned")
            .insert(overlay_short_id.clone(), lossy_listener);

        Ok(overlay)
    }

    fn stop_overlay(
        &self,
        overlay_short_id: &Arc<PrivateOverlayShortId>,
        overlay: &ConsensusOverlayPtr,
    ) {
        self.listeners
            .lock()
            .expect("LossyOverlayManager: listeners lock poisoned")
            .remove(overlay_short_id);
        self.inner.stop_overlay(overlay_short_id, overlay);
    }
}

#[cfg(test)]
#[path = "tests/test_lossy_overlay.rs"]
mod tests;
