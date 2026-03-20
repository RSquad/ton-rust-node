/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    engine_traits::EngineOperations,
    network::{
        check_block_candidate_data, check_sync_for_listen_bcasts,
        decompress_and_check_candidate_data, decompress_block_broadcast,
        decompress_block_broadcast_v2, node_network::NetworkContext, overlay_client::OverlayClient,
    },
};
use adnl::{
    common::{hash, spawn_cancelable, TaggedByteSlice},
    node::AdnlSendMethod,
    DhtSearchPolicy, OverlayNode, OverlayShortId,
};
use std::sync::Arc;
use ton_api::{
    ton::{
        overlay::membercertificate::MemberCertificate, pub_::publickey::Overlay as OverlayKey,
        ton_node::Broadcast,
    },
    BoxedSerialize,
};
use ton_block::{fail, KeyId, KeyOption, Result, ShardIdent, ValidatorSet};

const MAX_FAST_SYNC_OVERLAY_CLIENTS: usize = 5;

pub struct FastSyncOverlayClient {
    id: Arc<OverlayShortId>,
    shard: ShardIdent,
    client: Arc<OverlayClient>,
    engine: Arc<dyn EngineOperations>,
}

impl FastSyncOverlayClient {
    pub async fn new(
        shard: ShardIdent,
        validators: &ValidatorSet,
        key: Option<&Arc<dyn KeyOption>>,
        certificate: Option<MemberCertificate>,
        cancellation_token: tokio_util::sync::CancellationToken,
        network_context: Arc<NetworkContext>,
        engine: Arc<dyn EngineOperations>,
        policy: DhtSearchPolicy,
        default_rldp_roundtrip: Option<u32>,
    ) -> Result<Arc<Self>> {
        let mut root_members = Vec::new();
        for vd in validators.list() {
            root_members.push(vd.adnl_addr().clone());
        }
        let id = ton_api::ton::ton_node::fastsyncoverlayid::FastSyncOverlayId {
            zero_state_file_hash: engine.zerostate_id()?.file_hash().clone(),
            shard: (&shard).into(),
        };
        let id_full = hash(id)?;
        let overlay_key = OverlayKey { name: id_full.clone().into() };
        let id_short = OverlayShortId::from_data(hash(overlay_key)?);

        log::info!(
            "Creating fast sync overlay client for shard {shard}, id: {id_short}, adnl id: {}",
            key.map(|k| k.id().to_string()).unwrap_or_default()
        );

        let client = OverlayClient::new_semiprivate(
            id_short.clone(),
            id_full,
            root_members,
            key,
            certificate,
            network_context,
            cancellation_token.clone(),
            policy,
            default_rldp_roundtrip,
            MAX_FAST_SYNC_OVERLAY_CLIENTS,
        )
        .await?;

        let result = Arc::new(Self { id: id_short, shard, client, engine });

        result.clone().listen_broadcasts(cancellation_token.clone());

        Ok(result)
    }

    pub fn id(&self) -> &OverlayShortId {
        &self.id
    }

    pub fn client(&self) -> &Arc<OverlayClient> {
        &self.client
    }

    pub fn stop(&self) {
        log::debug!("Stopping fast sync overlay {} {}", self.shard, self.id);
        self.client.delete().ok();
    }

    fn listen_broadcasts(self: Arc<Self>, cancellation_token: tokio_util::sync::CancellationToken) {
        spawn_cancelable(cancellation_token.clone(), async move {
            log::debug!(
                "Started listening broadcasts in fast sync overlay {} {}",
                self.shard,
                self.id
            );
            loop {
                if self.engine.check_stop() {
                    break;
                }
                if !check_sync_for_listen_bcasts(self.engine.as_ref()) {
                    log::debug!(
                        "Node is not synced, pause processing broadcasts for fast sync overlay {} {}",
                        self.shard,
                        self.id
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    continue;
                }
                match self.client.wait_broadcast().await {
                    Err(e) => {
                        log::error!(
                            "Error while wait_broadcast in fast sync overlay {} {}: {e}",
                            self.shard,
                            self.id
                        );
                        // Wait for the graceful loop exit
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    }
                    Ok(None) => {
                        log::debug!(
                            "Finish listening broadcasts in fast sync overlay {} {}",
                            self.shard,
                            self.id
                        );
                        break;
                    }
                    Ok(Some((broadcast, src))) => {
                        log::trace!(
                            "Received broadcast {:08x} in fast sync overlay {} {} from {src}",
                            broadcast.bare_object().constructor(),
                            self.shard,
                            self.id
                        );
                        if let Err(e) = self.process_broadcast(&src, broadcast).await {
                            log::warn!(
                                "Error while processing broadcast from {src} \
                                in fast sync overlay {} {}: {e}",
                                self.shard,
                                self.id
                            );
                        }
                    }
                }
            }
        });
    }

    async fn process_broadcast(&self, src: &Arc<KeyId>, broadcast: Broadcast) -> Result<()> {
        match broadcast {
            Broadcast::TonNode_BlockBroadcast(broadcast) => {
                self.engine.clone().process_block_broadcast(broadcast, src.clone())
            }
            Broadcast::TonNode_ExternalMessageBroadcast(_broadcast) => log::warn!(
                "ExternalMessageBroadcast from {src} is not allowed \
                    in fast sync overlay {} {}",
                self.shard,
                self.id
            ),
            Broadcast::TonNode_IhrMessageBroadcast(_broadcast) => log::warn!(
                "IhrMessageBroadcast from {src} is not allowed in fast sync overlay {} {}",
                self.shard,
                self.id
            ),
            Broadcast::TonNode_NewShardBlockBroadcast(broadcast) => {
                self.engine.clone().process_new_shard_block_broadcast(broadcast, src.clone())
            }
            Broadcast::TonNode_BlockBroadcastCompressed(broadcast) => {
                match decompress_block_broadcast(broadcast) {
                    Err(e) => log::error!("Error decompressing block broadcast: {e}"),
                    Ok(b) => self.engine.clone().process_block_broadcast(b, src.clone()),
                }
            }
            Broadcast::TonNode_BlockBroadcastCompressedV2(broadcast) => {
                match decompress_block_broadcast_v2(broadcast) {
                    Err(e) => log::error!("Error decompressing block broadcast V2: {e}"),
                    Ok(b) => self.engine.clone().process_block_broadcast_v2(b, src.clone()),
                }
            }
            Broadcast::TonNode_NewBlockCandidateBroadcast(broadcast) => {
                log::debug!(
                    "NewBlockCandidateBroadcast {} (overlay {}) received from {src}",
                    broadcast.id,
                    self.id
                );
                check_block_candidate_data(&broadcast)?;
                self.engine.cache_block_candidate(
                    &broadcast.id,
                    broadcast.catchain_seqno as u32,
                    broadcast.validator_set_hash as u32,
                    broadcast.collator_signature.try_into().unwrap_or_default(),
                    broadcast.data,
                )?;
            }
            Broadcast::TonNode_NewBlockCandidateBroadcastCompressed(broadcast) => {
                log::debug!(
                    "NewBlockCandidateBroadcastCompressed {} (overlay {}) received from {src}",
                    broadcast.id,
                    self.id
                );
                let data = decompress_and_check_candidate_data(&broadcast)?;
                self.engine.cache_block_candidate(
                    &broadcast.id,
                    broadcast.catchain_seqno as u32,
                    broadcast.validator_set_hash as u32,
                    broadcast.collator_signature.try_into().unwrap_or_default(),
                    data,
                )?;
            }
            Broadcast::TonNode_NewBlockCandidateBroadcastCompressedV2(_broadcast) => log::warn!(
                "NewBlockCandidateBroadcastCompressedV2 from {src} is not supported in fast sync overlay {} {}",
                self.shard,
                self.id
            ),
            Broadcast::TonNode_OutMsgQueueProofBroadcast(_) => {
                fail!("OutMsgQueueProofBroadcast processing is not implemented")
            }
        }
        Ok(())
    }

    pub async fn send_broadcast(
        &self,
        data: &TaggedByteSlice<'_>,
        flags: u32,
        method: AdnlSendMethod,
    ) -> Result<()> {
        match self
            .client
            .broadcast(data, None, flags | OverlayNode::FLAG_BCAST_ANY_SENDER, method)
            .await
        {
            Ok(info) => {
                #[cfg(feature = "telemetry")]
                log::debug!(
                    "sent broadcast {:08x} in {} to {} nodes",
                    data.tag,
                    self.id,
                    info.send_to
                );
                #[cfg(not(feature = "telemetry"))]
                log::debug!("sent broadcast in {} to {} nodes", self.id, info.send_to);

                Ok(())
            }
            Err(e) => {
                log::warn!("Error sending broadcast: {}", e);
                Err(e)
            }
        }
    }
}
