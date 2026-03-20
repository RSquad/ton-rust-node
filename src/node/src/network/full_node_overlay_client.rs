/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    block::BlockStuff,
    block_proof::BlockProofStuff,
    engine_traits::EngineOperations,
    network::{
        check_block_candidate_data, check_sync_for_listen_bcasts,
        decompress_and_check_candidate_data, decompress_block_broadcast,
        decompress_block_broadcast_v2, neighbours::Neighbour, overlay_client::OverlayClient,
        MAX_COMPRESSED_SIZE,
    },
    shard_state::ShardStateStuff,
};
use adnl::{
    common::{TaggedByteSlice, TaggedObject},
    node::AdnlSendMethod,
    OverlayNode,
};
use rand::seq::IteratorRandom;
use std::{sync::Arc, time::Duration};
use storage::types::PersistentStatePartId;
#[cfg(feature = "telemetry")]
use ton_api::Constructor;
use ton_api::{
    ton::{
        rpc::ton_node::{
            DownloadBlockFull, DownloadBlockProof, DownloadBlockProofLink, DownloadKeyBlockProof,
            DownloadKeyBlockProofLink, DownloadNextBlockFull, DownloadPersistentStateSliceV2,
            DownloadZeroState, GetArchiveInfo, GetArchiveSlice, GetNextKeyBlockIds,
            GetPersistentStateSizeV2, GetShardArchiveInfo, PrepareBlock, PrepareBlockProof,
            PrepareKeyBlockProof, PreparePersistentState, PrepareZeroState,
        },
        ton_node::{
            persistentstateidv2::PersistentStateIdV2, shardid::ShardId, ArchiveInfo, Broadcast,
            DataFull, KeyBlocks, PersistentStateSize, Prepared, PreparedProof, PreparedState,
        },
    },
    AnyBoxedSerialize, BoxedSerialize,
};
use ton_block::{
    error, fail, lz4_decompress, read_boc, write_boc, BlockIdExt, BocFlags, BocWriter, KeyId,
    Lz4DecompressMode, Result, ShardIdent,
};

/// Wrapper for OverlayClient that provides methods for performing fullnode network requests
pub struct FullNodeOverlayClient {
    active_peers: lockfree::set::Set<Arc<KeyId>>,
    bad_peers: lockfree::set::Set<Arc<KeyId>>,
    engine: Arc<dyn EngineOperations>,
    client: Arc<OverlayClient>,
    shard: ShardIdent,
}

impl FullNodeOverlayClient {
    const TIMEOUT_PREPARE: u64 = 6000; // Milliseconds
    const TIMEOUT_NO_NEIGHBOURS: u64 = 1000; // Milliseconds

    pub(crate) fn new(
        engine: Arc<dyn EngineOperations>,
        client: Arc<OverlayClient>,
        shard: ShardIdent,
    ) -> Arc<Self> {
        let fnc = Arc::new(Self {
            active_peers: lockfree::set::Set::new(),
            bad_peers: lockfree::set::Set::new(),
            engine,
            client,
            shard,
        });
        fnc.clone().listen_broadcasts();
        fnc
    }

    pub fn overlay_client(&self) -> &Arc<OverlayClient> {
        &self.client
    }

    pub fn shard(&self) -> &ShardIdent {
        &self.shard
    }

    fn listen_broadcasts(self: Arc<Self>) {
        tokio::spawn(async move {
            log::debug!("Started listening broadcasts for shard {}", self.shard());
            loop {
                if self.engine.check_stop() {
                    break;
                }
                if !self.overlay_client().is_active() {
                    continue;
                }
                if self.overlay_client().is_died() {
                    log::warn!("Overlay client {} is dead", self.shard());
                    return;
                }
                if !check_sync_for_listen_bcasts(self.engine.as_ref()) {
                    log::debug!(
                        "Node is not synced, pause processing broadcasts for shard {}",
                        self.shard()
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    continue;
                }
                match self.overlay_client().wait_broadcast().await {
                    Err(e) => {
                        log::error!("Error while wait_broadcast for shard {}: {e}", self.shard());
                        // Wait for the graceful loop exit
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    }
                    Ok(None) => {
                        log::warn!("wait_broadcast finished.");
                        break;
                    }
                    Ok(Some((broadcast, src))) => {
                        log::trace!(
                            "Received broadcast {:08x} in public overlay {} shard {} from {src}",
                            broadcast.bare_object().constructor(),
                            self.client.id(),
                            self.shard
                        );
                        if let Err(e) = self.process_broadcast(&src, broadcast).await {
                            log::warn!(
                                "Error while processing broadcast from {src} \
                                in public overlay {} for shard {}: {e}",
                                self.client.id(),
                                self.shard()
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
            Broadcast::TonNode_ExternalMessageBroadcast(broadcast) => {
                self.engine.process_ext_msg_broadcast(broadcast, src.clone()).await
            }
            Broadcast::TonNode_IhrMessageBroadcast(broadcast) => {
                log::debug!("Skipped TonNode_IhrMessageBroadcast from {src}: {broadcast:?}")
            }
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
                    self.client.id()
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
                    self.client.id()
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
                "NewBlockCandidateBroadcastCompressedV2 from {src} is not supported in public overlay {}",
                self.client.id()
            ),
            Broadcast::TonNode_OutMsgQueueProofBroadcast(_) => log::debug!(
                "OutMsgQueueProofBroadcast from {src} is not supported in public overlay {}",
                self.client.id()
            ),
        }
        Ok(())
    }

    // tonNode.prepareBlockProof block:tonNode.blockIdExt allow_partial:Bool = tonNode.PreparedProof;
    // tonNode.preparedProofEmpty = tonNode.PreparedProof;
    // tonNode.preparedProof = tonNode.PreparedProof;
    // tonNode.preparedProofLink = tonNode.PreparedProof;
    //
    // tonNode.downloadBlockProof block:tonNode.blockIdExt = tonNode.Data;
    // tonNode.downloadBlockProofLink block:tonNode.blockIdExt = tonNode.Data;
    pub async fn download_block_proof(
        &self,
        block_id: &BlockIdExt,
        is_link: bool,
        key_block: bool,
    ) -> Result<BlockProofStuff> {
        // Prepare
        let request = if key_block {
            PrepareKeyBlockProof { block: block_id.clone(), allow_partial: is_link.into() }
                .into_tl_object()
        } else {
            PrepareBlockProof { block: block_id.clone(), allow_partial: is_link.into() }
                .into_tl_object()
        }
        .into();
        let (prepare, good_peer): (PreparedProof, _) =
            self.client.send_adnl_query(&request, None, Some(Self::TIMEOUT_PREPARE), None).await?;
        // Download
        let (request, is_link) = match prepare {
            PreparedProof::TonNode_PreparedProofEmpty => {
                fail!("Got `TonNode_PreparedProofEmpty` from {}", good_peer.id())
            }
            PreparedProof::TonNode_PreparedProof => {
                let request = if key_block {
                    DownloadKeyBlockProof { block: block_id.clone() }.into_tl_object()
                } else {
                    DownloadBlockProof { block: block_id.clone() }.into_tl_object()
                };
                (request, false)
            }
            PreparedProof::TonNode_PreparedProofLink => {
                let request = if key_block {
                    DownloadKeyBlockProofLink { block: block_id.clone() }.into_tl_object()
                } else {
                    DownloadBlockProofLink { block: block_id.clone() }.into_tl_object()
                };
                (request, true)
            }
        };
        let proof = self.client.send_rldp_query_raw(&request.into(), &good_peer, 0, true).await?;
        BlockProofStuff::deserialize(block_id, proof, is_link)
    }

    // tonNode.prepareBlock block:tonNode.blockIdExt = tonNode.Prepared;
    // tonNode.downloadBlockFull block:tonNode.blockIdExt = tonNode.DataFull;
    // tonNode.dataFull id:tonNode.blockIdExt proof:bytes block:bytes is_link:Bool = tonNode.DataFull;
    // tonNode.dataFullEmpty = tonNode.DataFull;
    //
    // tonNode.downloadBlock block:tonNode.blockIdExt = tonNode.Data; DEPRECATED?
    pub async fn download_block_full(
        &self,
        id: &BlockIdExt,
    ) -> Result<(BlockStuff, BlockProofStuff)> {
        // Prepare
        let (prepare, peer): (Prepared, _) = self
            .client
            .send_adnl_query(
                &PrepareBlock { block: id.clone() }.into_tl_object().into(),
                Some(1), // 1 attempt
                None,    // default timeout
                Some(&self.active_peers),
            )
            .await?;
        log::trace!("USE PEER {}, PREPARE {} FINISHED", peer, id);
        // Download
        match prepare {
            Prepared::TonNode_NotFound => {
                self.active_peers.remove(peer.id());
                fail!("Got `TonNode_NotFound` from {}", peer.id())
            }
            Prepared::TonNode_Prepared => {
                let data_full: DataFull = self
                    .client
                    .send_rldp_query_typed(
                        &TaggedObject {
                            object: DownloadBlockFull { block: id.clone() },
                            #[cfg(feature = "telemetry")]
                            tag: DownloadBlockFull::constructor_const(),
                        },
                        &peer,
                        0,
                        true,
                    )
                    .await?;
                let (block_data, proof_data, is_link) = match data_full {
                    DataFull::TonNode_DataFull(data_full) => {
                        if id != &data_full.id {
                            self.active_peers.remove(peer.id());
                            fail!("Block with another id was received");
                        }
                        (data_full.block, data_full.proof, data_full.is_link.into())
                    }
                    DataFull::TonNode_DataFullCompressed(data_full) => {
                        if id != &data_full.id {
                            self.active_peers.remove(peer.id());
                            fail!("Block with another id was received");
                        }
                        let decompressed = lz4_decompress(
                            &data_full.compressed,
                            Lz4DecompressMode::WithMaxSize(MAX_COMPRESSED_SIZE as i32),
                        )?;
                        let mut roots = read_boc(&decompressed)?.roots;
                        if roots.len() != 2 {
                            self.active_peers.remove(peer.id());
                            fail!("DataFullCompressed contains {} roots instead of 2", roots.len());
                        }
                        let proof_data = write_boc(&roots.remove(0))?;
                        // Block's boc here must be serialized absolutely
                        // the same way as in collator, because of file hash concept.
                        let mut block_data = Vec::new();
                        BocWriter::with_flags(roots, BocFlags::all())?.write(&mut block_data)?;
                        (block_data, proof_data, data_full.is_link.into())
                    }
                    DataFull::TonNode_DataFullEmpty => {
                        self.active_peers.remove(peer.id());
                        fail!(
                            "PrepareBlock returned Prepared, but DownloadBlockFull \
                             returned DataFullEmpty from {}",
                            peer.id()
                        )
                    }
                };
                let proof = BlockProofStuff::deserialize(id, proof_data, is_link).map_err(|e| {
                    self.active_peers.remove(peer.id());
                    error!("Error deserializing block proof from {}: {}", peer.id(), e)
                })?;
                let block = BlockStuff::deserialize_block_checked(id.clone(), Arc::new(block_data))
                    .map_err(|e| {
                        self.active_peers.remove(peer.id());
                        error!("Error deserializing block from {}: {}", peer.id(), e)
                    })?;
                Ok((block, proof))
            }
        }
    }

    pub async fn check_persistent_state(
        &self,
        id: &PersistentStatePartId,
        masterchain_block_id: &BlockIdExt,
    ) -> Result<Option<(Arc<Neighbour>, usize)>> {
        if let PersistentStatePartId::WholeState(id) = &id {
            let request = PreparePersistentState {
                block: id.clone(),
                masterchain_block: masterchain_block_id.clone(),
            }
            .into_tl_object()
            .into();
            let (prepare, peer): (PreparedState, _) = self
                .client
                .send_adnl_query_to_all_peers(
                    &request,
                    Some(Self::TIMEOUT_PREPARE),
                    None,
                    &self.bad_peers,
                    |result| matches!(result, &PreparedState::TonNode_PreparedState),
                )
                .await?;
            match prepare {
                PreparedState::TonNode_NotFoundState => Ok(None),
                PreparedState::TonNode_PreparedState => Ok(Some((peer, 0))),
            }
        } else {
            let request = GetPersistentStateSizeV2 {
                state: PersistentStateIdV2 {
                    block: id.block_id().clone(),
                    masterchain_block: masterchain_block_id.clone(),
                    effective_shard: id.part_prefix() as i64,
                },
            }
            .into_tl_object()
            .into();
            let (size, peer): (PersistentStateSize, _) = self
                .client
                .send_adnl_query_to_all_peers(
                    &request,
                    Some(Self::TIMEOUT_PREPARE),
                    None,
                    &self.bad_peers,
                    |result| matches!(result, &PersistentStateSize::TonNode_PersistentStateSize(_)),
                )
                .await?;
            match size {
                PersistentStateSize::TonNode_PersistentStateSizeNotFound => Ok(None),
                PersistentStateSize::TonNode_PersistentStateSize(size) => {
                    Ok(Some((peer, size.size as usize)))
                }
            }
        }
    }

    pub async fn download_persistent_state_part(
        &self,
        block_id: &PersistentStatePartId,
        masterchain_block_id: &BlockIdExt,
        offset: usize,
        max_size: usize,
        peer: &Arc<Neighbour>,
        attempt: u32,
    ) -> Result<Vec<u8>> {
        let request = TaggedObject {
            object: DownloadPersistentStateSliceV2 {
                state: PersistentStateIdV2 {
                    block: block_id.block_id().clone(),
                    masterchain_block: masterchain_block_id.clone(),
                    effective_shard: block_id.part_prefix() as i64,
                },
                offset: offset as i64,
                max_size: max_size as i64,
            },
            #[cfg(feature = "telemetry")]
            tag: DownloadPersistentStateSliceV2::constructor_const(),
        };
        self.client.send_rldp_query_raw(&request, peer, attempt, true).await
    }

    // tonNode.prepareZeroState block:tonNode.blockIdExt = tonNode.PreparedState;
    // tonNode.downloadZeroState block:tonNode.blockIdExt = tonNode.Data;
    pub async fn download_zero_state(
        &self,
        id: &BlockIdExt,
    ) -> Result<(Arc<ShardStateStuff>, Vec<u8>)> {
        // Prepare
        let (prepare, good_peer): (PreparedState, _) = self
            .client
            .send_adnl_query(
                &PrepareZeroState { block: id.clone() }.into_tl_object().into(),
                None,
                Some(Self::TIMEOUT_PREPARE),
                None,
            )
            .await?;
        // Download
        match prepare {
            PreparedState::TonNode_NotFoundState => {
                fail!("Got `TonNode_NotFoundState` from {}", good_peer.id())
            }
            PreparedState::TonNode_PreparedState => {
                let state_bytes = self
                    .client
                    .send_rldp_query_raw(
                        &TaggedObject {
                            object: DownloadZeroState { block: id.clone() },
                            #[cfg(feature = "telemetry")]
                            tag: DownloadZeroState::constructor_const(),
                        },
                        &good_peer,
                        0,
                        true,
                    )
                    .await?;
                let state = ShardStateStuff::deserialize_zerostate(
                    id.clone(),
                    state_bytes.as_slice(),
                    #[cfg(feature = "telemetry")]
                    &self.client.network_context().engine_telemetry,
                    &self.client.network_context().engine_allocated,
                )?;
                Ok((state, state_bytes))
            }
        }
    }

    // tonNode.keyBlocks blocks:(vector tonNode.blockIdExt) incomplete:Bool error:Bool
    //     = tonNode.KeyBlocks;
    // tonNode.getNextKeyBlockIds block:tonNode.blockIdExt max_size:int
    //     = tonNode.KeyBlocks;
    pub async fn download_next_key_blocks_ids(
        &self,
        block_id: &BlockIdExt,
        max_size: i32,
    ) -> Result<Vec<BlockIdExt>> {
        let request =
            GetNextKeyBlockIds { block: block_id.clone(), max_size }.into_tl_object().into();
        let (ids, _): (KeyBlocks, _) =
            self.client.send_adnl_query(&request, None, None, None).await?;
        if !ids.blocks().is_empty() {
            return Ok(ids.only().blocks);
        }
        return Ok(Vec::new());
    }

    // tonNode.downloadNextBlockFull prev_block:tonNode.blockIdExt = tonNode.DataFull;
    pub async fn download_next_block_full(
        &self,
        prev_id: &BlockIdExt,
    ) -> Result<(BlockStuff, BlockProofStuff)> {
        let request = TaggedObject {
            object: DownloadNextBlockFull { prev_block: prev_id.clone() },
            #[cfg(feature = "telemetry")]
            tag: DownloadNextBlockFull::constructor_const(),
        };
        let peer = loop {
            if let Some(p) = self.active_peers.iter().choose(&mut rand::thread_rng()) {
                if let Some(n) = self.client.neighbours().peer(&p) {
                    break n;
                }
            }
            if let Some(n) = self.client.neighbours().choose_neighbour()? {
                break n;
            } else {
                tokio::time::sleep(Duration::from_millis(Self::TIMEOUT_NO_NEIGHBOURS)).await;
                fail!("neighbour is not found!")
            }
        };
        log::trace!("USE PEER {}, REQUEST {:?}", peer, request.object);
        let data_full: DataFull =
            self.client.send_rldp_query_typed(&request, &peer, 0, true).await?;
        match data_full {
            DataFull::TonNode_DataFull(data_full) => {
                let proof = BlockProofStuff::deserialize(
                    &data_full.id,
                    data_full.proof.to_vec(),
                    data_full.is_link.clone().into(),
                )
                .map_err(|e| {
                    self.active_peers.remove(peer.id());
                    error!("Failed to deserialize block proof from peer {}: {:?}", peer.id(), e)
                })?;
                let block = BlockStuff::deserialize_block_checked(
                    data_full.id.clone(),
                    Arc::new(data_full.block),
                )
                .map_err(|e| {
                    self.active_peers.remove(peer.id());
                    error!("Failed to deserialize block from peer {}: {:?}", peer.id(), e)
                })?;
                Ok((block, proof))
            }
            DataFull::TonNode_DataFullCompressed(_) => {
                fail!("Got DataFullCompressed from {}, not supported", peer.id())
            }
            DataFull::TonNode_DataFullEmpty => {
                // Do not delete peer from active_peers,
                // because next block may be not yet created.

                fail!("Got DataFullEmpty from {}", peer.id())
            }
        }
    }

    pub async fn download_archive(
        &self,
        mc_seq_no: u32,
        shard: &ShardIdent,
    ) -> Result<Option<Vec<u8>>> {
        const CHUNK_SIZE: i32 = 1 << 21;
        const MAX_ATTEMPTS: usize = 10;
        // tonNode.getArchiveInfo masterchain_seqno:int
        //     = tonNode.ArchiveInfo;
        // OR
        // tonNode.getShardArchiveInfo masterchain_seqno:int shard_prefix:tonNode.shardId
        //     = tonNode.ArchiveInfo;
        if !self.shard.intersect_with(shard) {
            fail!("Cannot download archive for shard {} in overlay {}", shard, self.shard);
        }
        let object = if self.shard.is_masterchain() {
            GetArchiveInfo { masterchain_seqno: mc_seq_no as i32 }.into_tl_object()
        } else {
            GetShardArchiveInfo {
                masterchain_seqno: mc_seq_no as i32,
                shard_prefix: ShardId {
                    workchain: shard.workchain_id(),
                    shard: shard.shard_prefix_with_tag() as i64,
                },
            }
            .into_tl_object()
        }
        .into();
        let (archive_info, peer) = self
            .client
            .send_adnl_query(&object, None, Some(Self::TIMEOUT_PREPARE), Some(&self.active_peers))
            .await?;
        let info = match archive_info {
            ArchiveInfo::TonNode_ArchiveNotFound => {
                self.active_peers.remove(peer.id());
                return Ok(None);
            }
            ArchiveInfo::TonNode_ArchiveInfo(info) => info,
        };
        let mut result = Vec::new();
        let mut slice = TaggedObject {
            object: GetArchiveSlice { archive_id: info.id, offset: 0, max_size: CHUNK_SIZE },
            #[cfg(feature = "telemetry")]
            tag: GetArchiveSlice::constructor_const(),
        };
        let mut part_attempt = 0;
        let mut peer_attempt = 0;
        loop {
            if self.engine.check_stop() {
                log::info!("Engine is stopping, quit download archive");
                break Ok(None);
            }
            match self.client.send_rldp_query_raw(&slice, &peer, peer_attempt, true).await {
                Ok(mut block_bytes) => {
                    let actual_size = block_bytes.len() as i32;
                    result.append(&mut block_bytes);
                    if actual_size < CHUNK_SIZE {
                        self.active_peers.remove(peer.id());
                        break Ok(Some(result));
                    }
                    slice.object.offset += actual_size as i64;
                    part_attempt = 0;
                }
                Err(e) => {
                    peer_attempt += 1;
                    part_attempt += 1;
                    log::error!(
                        "download_archive {}: {e}, offset: {}, attempt: {part_attempt}",
                        info.id,
                        slice.object.offset
                    );
                    if part_attempt > MAX_ATTEMPTS {
                        self.active_peers.remove(peer.id());
                        fail!("Error download_archive after {part_attempt} attempts : {e}")
                    }
                }
            }
        }
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
                    self.client.id(),
                    info.send_to
                );
                #[cfg(not(feature = "telemetry"))]
                log::debug!("sent broadcast in {} to {} nodes", self.client.id(), info.send_to);

                Ok(())
            }
            Err(e) => {
                log::warn!("Error sending broadcast: {}", e);
                Err(e)
            }
        }
    }
}
