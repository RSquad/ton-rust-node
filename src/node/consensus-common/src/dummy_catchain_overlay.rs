/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    BlockPayloadPtr, ConsensusNode, ConsensusOverlay, ConsensusOverlayListenerPtr,
    ConsensusOverlayLogReplayListenerPtr, ConsensusOverlayManager, ConsensusOverlayManagerPtr,
    ConsensusOverlayPtr, OverlayTransportType, PrivateKey, PublicKeyHash, QueryResponseCallback,
    Result,
};
use adnl::PrivateOverlayShortId;
use std::sync::Arc;

pub(crate) struct DummyConsensusOverlay;

impl ConsensusOverlay for DummyConsensusOverlay {
    fn get_impl(&self) -> &dyn std::any::Any {
        self
    }

    fn send_message(
        &self,
        receiver_id: &PublicKeyHash,
        sender_id: &PublicKeyHash,
        message: &BlockPayloadPtr,
        _is_retransmission: bool,
    ) {
        log::trace!(
            "DummyConsensusOverlay: send message {:?} -> {:?}: {:?}",
            sender_id,
            receiver_id,
            message
        );
    }

    fn send_message_multicast(
        &self,
        receiver_ids: &[PublicKeyHash],
        sender_id: &PublicKeyHash,
        message: &BlockPayloadPtr,
        _is_retransmission: bool,
    ) {
        log::trace!(
            "DummyConsensusOverlay: send message multicast {:?} -> {:?}: {:?}",
            sender_id,
            receiver_ids,
            message
        );
    }

    fn send_query(
        &self,
        receiver_id: &PublicKeyHash,
        sender_id: &PublicKeyHash,
        name: &str,
        _timeout: std::time::Duration,
        message: &BlockPayloadPtr,
        _response_callback: QueryResponseCallback,
    ) {
        log::trace!(
            "DummyConsensusOverlay: send query {} {:?} -> {:?}: {:?}",
            name,
            sender_id,
            receiver_id,
            message
        );
    }

    fn send_query_via_rldp(
        &self,
        dst_adnl_id: PublicKeyHash,
        name: String,
        _response_callback: QueryResponseCallback,
        _timeout: std::time::SystemTime,
        query: BlockPayloadPtr,
        _max_answer_size: u64,
        _v2: bool,
    ) {
        log::trace!(
            "DummyConsensusOverlay: send query '{}' via RLDP -> {}: {:?}",
            name,
            dst_adnl_id,
            query
        );
    }

    fn send_broadcast_fec_ex(
        &self,
        sender_id: &PublicKeyHash,
        send_as: &PublicKeyHash,
        payload: BlockPayloadPtr,
        _extra: Option<Vec<u8>>,
    ) {
        log::trace!(
            "DummyConsensusOverlay: send broadcast_fec_ex {:?}/{:?}: {:?}",
            sender_id,
            send_as,
            payload
        );
    }
}

pub(crate) struct DummyConsensusOverlayManager;

impl ConsensusOverlayManager for DummyConsensusOverlayManager {
    fn start_overlay(
        &self,
        _local_validator_key: &PrivateKey,
        _overlay_short_id: &Arc<PrivateOverlayShortId>,
        _nodes: &[ConsensusNode],
        _overlay_listener: ConsensusOverlayListenerPtr,
        _log_replay_listener: ConsensusOverlayLogReplayListenerPtr,
        _transport_type: OverlayTransportType,
        _block_sync_params: Option<crate::BlockSyncOverlayParams>,
    ) -> Result<ConsensusOverlayPtr> {
        Ok(Arc::new(DummyConsensusOverlay))
    }

    fn stop_overlay(
        &self,
        _overlay_short_id: &Arc<PrivateOverlayShortId>,
        _overlay: &ConsensusOverlayPtr,
    ) {
    }
}

impl DummyConsensusOverlayManager {
    pub(crate) fn create() -> ConsensusOverlayManagerPtr {
        Arc::new(DummyConsensusOverlayManager)
    }
}
