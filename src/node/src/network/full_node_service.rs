/*
 * Copyright (C) 2019-2024 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    engine_traits::EngineOperations,
    network::neighbours::{PROTOCOL_VERSION_MAJOR, PROTOCOL_VERSION_MINOR},
};
use adnl::common::{AdnlPeers, Answer, QueryAnswer, QueryResult, Subscriber, TaggedByteVec};
use std::{cmp::min, fmt::Debug, sync::Arc};
use storage::types::PersistentStatePartId;
#[cfg(feature = "telemetry")]
use ton_api::BoxedSerialize;
use ton_api::{
    serialize_boxed,
    ton::{
        self,
        rpc::ton_node::{
            DownloadBlock, DownloadBlockFull, DownloadBlockProof, DownloadBlockProofLink,
            DownloadKeyBlockProof, DownloadKeyBlockProofLink, DownloadNextBlockFull,
            DownloadPersistentState, DownloadPersistentStateSlice, DownloadPersistentStateSliceV2,
            DownloadZeroState, GetArchiveInfo, GetArchiveSlice, GetCapabilities,
            GetNextBlockDescription, GetNextKeyBlockIds, GetPersistentStateSize,
            GetPersistentStateSizeV2, GetShardArchiveInfo, PrepareBlock, PrepareBlockProof,
            PrepareKeyBlockProof, PreparePersistentState, PrepareZeroState,
        },
        ton_node::{
            self,
            archiveinfo::ArchiveInfo,
            capabilities::Capabilities,
            datafull::{DataFull, DataFullCompressed},
            persistentstateidv2::PersistentStateIdV2,
            ArchiveInfo as ArchiveInfoBoxed, BlockDescription, DataFull as DataFullBoxed,
            KeyBlocks, Prepared, PreparedProof, PreparedState,
        },
    },
    AnyBoxedSerialize, IntoBoxed, TLObject,
};
use ton_block::{
    fail, lz4_compress, read_single_root_boc, BlockIdExt, BocWriter, Result, ShardIdent,
};

// max part size for partially transmitted data like archives and states
const PART_MAX_SIZE: usize = 1 << 21;

pub struct FullNodeOverlayService {
    engine: Arc<dyn EngineOperations>,
    compression: bool,
}

impl FullNodeOverlayService {
    pub fn new(engine: Arc<dyn EngineOperations>, compression: bool) -> Self {
        Self { engine, compression }
    }

    // tonNode.getNextBlockDescription prev_block:tonNode.blockIdExt = tonNode.BlockDescription;
    async fn get_next_block_description(&self, query: GetNextBlockDescription) -> Result<TLObject> {
        let answer = match self.engine.load_block_next1(&query.prev_block) {
            Ok(id) => ton_node::blockdescription::BlockDescription { id }.into_boxed(),
            Err(_) => BlockDescription::TonNode_BlockDescriptionEmpty,
        };
        Ok(answer.into_tl_object())
    }

    // tonNode.getNextBlocksDescription prev_block:tonNode.blockIdExt limit:int
    //     = tonNode.BlocksDescription;
    // Not supported in t-node

    // tonNode.getPrevBlocksDescription next_block:tonNode.blockIdExt limit:int cutoff_seqno:int
    //     = tonNode.BlocksDescription;
    // Not supported in t-node

    fn prepare_block_proof_internal(
        &self,
        block_id: BlockIdExt,
        allow_partial: bool,
        key_block: bool,
    ) -> Result<TLObject> {
        let answer = if let Some(handle) = self.engine.load_block_handle(&block_id)? {
            if key_block && !handle.is_key_block()? {
                fail!("prepare_key_block_proof: given block is not key");
            }
            if !handle.has_proof() && (!allow_partial || !handle.has_proof_link()) {
                PreparedProof::TonNode_PreparedProofEmpty
            } else if handle.has_proof() && handle.id().shard().is_masterchain() {
                PreparedProof::TonNode_PreparedProof
            } else {
                PreparedProof::TonNode_PreparedProofLink
            }
        } else {
            PreparedProof::TonNode_PreparedProofEmpty
        };
        Ok(answer.into_tl_object())
    }

    // tonNode.prepareBlockProof block:tonNode.blockIdExt allow_partial:Bool
    //     = tonNode.PreparedProof;
    async fn prepare_block_proof(&self, query: PrepareBlockProof) -> Result<TLObject> {
        self.prepare_block_proof_internal(query.block, query.allow_partial.into(), false)
    }

    // tonNode.prepareKeyBlockProof block:tonNode.blockIdExt allow_partial:Bool
    //     = tonNode.PreparedProof;
    async fn prepare_key_block_proof(&self, query: PrepareKeyBlockProof) -> Result<TLObject> {
        self.prepare_block_proof_internal(query.block, query.allow_partial.into(), true)
    }

    // tonNode.prepareBlockProofs blocks:(vector tonNode.blockIdExt) allow_partial:Bool
    //     = tonNode.PreparedProof;
    // Not supported in t-node

    // tonNode.prepareKeyBlockProofs blocks:(vector tonNode.blockIdExt) allow_partial:Bool
    //     = tonNode.PreparedProof;
    // Not supported in t-node

    // tonNode.prepareBlock block:tonNode.blockIdExt = tonNode.Prepared;
    async fn prepare_block(&self, query: PrepareBlock) -> Result<TLObject> {
        let answer = if self
            .engine
            .load_block_handle(&query.block)?
            .map(|h| h.has_data())
            .unwrap_or(false)
        {
            Prepared::TonNode_Prepared
        } else {
            Prepared::TonNode_NotFound
        };
        Ok(answer.into_tl_object())
    }

    // tonNode.prepareBlocks blocks:(vector tonNode.blockIdExt) = tonNode.Prepared;
    // Not supported in t-node

    fn prepare_state_internal(&self, block_id: &BlockIdExt) -> Result<TLObject> {
        let answer = if self
            .engine
            .load_block_handle(block_id)?
            .map(|h| h.has_persistent_state())
            .unwrap_or(false)
        {
            PreparedState::TonNode_PreparedState
        } else {
            PreparedState::TonNode_NotFoundState
        };
        Ok(answer.into_tl_object())
    }

    // tonNode.preparePersistentState block:tonNode.blockIdExt masterchain_block:tonNode.blockIdExt
    //     = tonNode.PreparedState;
    async fn prepare_persistent_state(&self, query: PreparePersistentState) -> Result<TLObject> {
        self.prepare_state_internal(&query.block)
    }

    // tonNode.prepareZeroState block:tonNode.blockIdExt = tonNode.PreparedState;
    async fn prepare_zero_state(&self, query: PrepareZeroState) -> Result<TLObject> {
        self.prepare_state_internal(&query.block)
    }

    const NEXT_KEY_BLOCKS_LIMIT: usize = 8;

    fn build_next_key_blocks_answer(
        blocks: Vec<BlockIdExt>,
        incomplete: bool,
        error: bool,
    ) -> KeyBlocks {
        ton_node::keyblocks::KeyBlocks {
            blocks,
            incomplete: incomplete.into(),
            error: error.into(),
        }
        .into_boxed()
    }

    async fn get_next_key_block_ids_(
        &self,
        start_block_id: &BlockIdExt,
        limit: usize,
    ) -> Result<KeyBlocks> {
        if !start_block_id.shard().is_masterchain() {
            fail!("Given block {} doesn't belong master chain", start_block_id);
        }

        let last_mc_state = match self.engine.load_last_applied_mc_block_id()? {
            Some(block_id) if start_block_id.seq_no() < block_id.seq_no() => {
                self.engine.load_state(&block_id).await?
            }
            _ => {
                return Ok(ton_node::keyblocks::KeyBlocks {
                    blocks: Vec::new(),
                    incomplete: false.into(),
                    error: true.into(),
                }
                .into_boxed())
            }
        };
        let prev_blocks = &last_mc_state.shard_state_extra()?.prev_blocks;

        if start_block_id.seq_no != 0 {
            // check if start block is key-block
            prev_blocks.check_key_block(start_block_id, Some(true))?;
        }

        let mut ids = vec![];
        let mut seq_no = start_block_id.seq_no();
        while let Some(id) = prev_blocks.get_next_key_block(seq_no + 1)? {
            seq_no = id.seq_no;
            let ext_id = id.master_block_id().1;
            ids.push(ext_id);
            if ids.len() == limit {
                break;
            }
        }
        let incomplete = ids.len() < limit;
        Ok(Self::build_next_key_blocks_answer(ids, incomplete, false))
    }

    // tonNode.getNextKeyBlockIds block:tonNode.blockIdExt max_size:int = tonNode.KeyBlocks;
    async fn get_next_key_block_ids(&self, query: GetNextKeyBlockIds) -> Result<TLObject> {
        let limit = min(Self::NEXT_KEY_BLOCKS_LIMIT, query.max_size as usize);
        let answer = match self.get_next_key_block_ids_(&query.block, limit).await {
            Err(e) => {
                log::warn!("tonNode.getNextKeyBlockIds: {:?}", e);
                Self::build_next_key_blocks_answer(vec![], false, true)
            }
            Ok(r) => r,
        };
        Ok(answer.into_tl_object())
    }

    // tonNode.downloadNextBlockFull prev_block:tonNode.blockIdExt = tonNode.DataFull;
    async fn download_next_block_full(&self, query: DownloadNextBlockFull) -> Result<TLObject> {
        let mut answer = DataFullBoxed::TonNode_DataFullEmpty;
        if let Some(prev_handle) = self.engine.load_block_handle(&query.prev_block)? {
            if prev_handle.has_next1() {
                let next_id = self.engine.load_block_next1(&query.prev_block)?;
                if let Some(next_handle) = self.engine.load_block_handle(&next_id)? {
                    let has_proof_link = next_handle.has_proof_link();
                    let has_proof = next_handle.has_proof();
                    if next_handle.has_data() && (has_proof || has_proof_link) {
                        let block = self.engine.load_block_raw(&next_handle).await?;
                        let proof =
                            self.engine.load_block_proof_raw(&next_handle, has_proof_link).await?;
                        answer = DataFull {
                            id: next_id,
                            proof,
                            block,
                            is_link: if has_proof_link {
                                ton::Bool::BoolTrue
                            } else {
                                ton::Bool::BoolFalse
                            },
                        }
                        .into_boxed();
                    }
                }
            }
        }
        Ok(answer.into_tl_object())
    }

    // tonNode.downloadBlockFull block:tonNode.blockIdExt = tonNode.DataFull;
    async fn download_block_full(&self, query: DownloadBlockFull) -> Result<TLObject> {
        let mut answer = DataFullBoxed::TonNode_DataFullEmpty;
        if let Some(handle) = self.engine.load_block_handle(&query.block)? {
            let has_proof_link = handle.has_proof_link();
            let has_proof = handle.has_proof();
            if handle.has_data() && (has_proof || has_proof_link) {
                let block = self.engine.load_block_raw(&handle).await?;
                let proof = self.engine.load_block_proof_raw(&handle, has_proof_link).await?;
                answer = if self.compression {
                    let mut data = vec![];
                    let proof_root = read_single_root_boc(&proof)?;
                    let block_root = read_single_root_boc(&block)?;
                    BocWriter::with_roots([proof_root, block_root])?.write(&mut data)?;
                    let compressed = lz4_compress(&data, false)?;
                    DataFullCompressed {
                        id: query.block,
                        compressed,
                        is_link: if has_proof_link {
                            ton::Bool::BoolTrue
                        } else {
                            ton::Bool::BoolFalse
                        },
                    }
                    .into_boxed()
                } else {
                    DataFull {
                        id: query.block,
                        proof,
                        block,
                        is_link: if has_proof_link {
                            ton::Bool::BoolTrue
                        } else {
                            ton::Bool::BoolFalse
                        },
                    }
                    .into_boxed()
                };
            }
        }
        Ok(answer.into_tl_object())
    }

    // tonNode.downloadBlock block:tonNode.blockIdExt = tonNode.Data;
    async fn download_block(&self, query: DownloadBlock) -> Result<TaggedByteVec> {
        if let Some(handle) = self.engine.load_block_handle(&query.block)? {
            if handle.has_data() {
                let answer = TaggedByteVec {
                    object: self.engine.load_block_raw(&handle).await?,
                    #[cfg(feature = "telemetry")]
                    tag: 0x8000000A, // Raw reply do download block
                };
                return Ok(answer);
            }
        }
        fail!("Block's data isn't initialized");
    }

    // tonNode.downloadBlocks blocks:(vector tonNode.blockIdExt) = tonNode.DataList;
    // Not supported in t-node

    // tonNode.downloadPersistentState block:tonNode.blockIdExt masterchain_block:tonNode.blockIdExt = tonNode.Data;
    async fn download_persistent_state(
        &self,
        query: DownloadPersistentState,
    ) -> Result<TaggedByteVec> {
        // This request is never called in t-node, because new downloadPersistentStateSlice exists.
        // Because of state is pretty big it is bad idea to send it by one request.
        fail!(
            "`tonNode.downloadPersistentState` request is not supported (block: {}, mc block: {})",
            query.block,
            query.masterchain_block
        );
    }

    // tonNode.downloadPersistentStateSlice block:tonNode.blockIdExt masterchain_block:tonNode.blockIdExt offset:long max_size:long = tonNode.Data;
    async fn download_persistent_state_slice(
        &self,
        query: DownloadPersistentStateSlice,
    ) -> Result<TaggedByteVec> {
        if query.max_size as usize > PART_MAX_SIZE {
            fail!("Part size {} is too big, max is {}", query.max_size, PART_MAX_SIZE);
        }
        if let Some(handle) = self.engine.load_block_handle(&query.block)? {
            if handle.has_persistent_state() {
                let data = self
                    .engine
                    .load_persistent_state_slice(
                        &PersistentStatePartId::WholeState(handle.id().clone()),
                        query.offset as u64,
                        query.max_size as u64,
                    )
                    .await?;
                let answer = TaggedByteVec {
                    object: data,
                    #[cfg(feature = "telemetry")]
                    tag: 0x8000000B, // Raw reply to download state slice
                };
                return Ok(answer);
            }
        }
        fail!("Persistent state for block {} is not exist", query.block)
    }

    async fn download_persistent_state_slice_v2(
        &self,
        query: DownloadPersistentStateSliceV2,
    ) -> Result<TaggedByteVec> {
        if query.max_size as usize > PART_MAX_SIZE {
            fail!("Part size {} is too big, max is {}", query.max_size, PART_MAX_SIZE);
        }
        if let Some(handle) = self.engine.load_block_handle(&query.state.block)? {
            if handle.has_persistent_state() {
                let data = self
                    .engine
                    .load_persistent_state_slice(
                        &Self::convert_pss_id(&query.state),
                        query.offset as u64,
                        query.max_size as u64,
                    )
                    .await?;
                let answer = TaggedByteVec {
                    object: data,
                    #[cfg(feature = "telemetry")]
                    tag: 0x8000000B, // Raw reply to download state slice
                };
                return Ok(answer);
            }
        }
        fail!(
            "Persistent state for block {}, {:016x} is not exist",
            query.state.block,
            query.state.effective_shard
        )
    }

    // tonNode.downloadZeroState block:tonNode.blockIdExt = tonNode.Data;
    async fn download_zero_state(&self, query: DownloadZeroState) -> Result<TaggedByteVec> {
        if let Some(handle) = self.engine.load_block_handle(&query.block)? {
            if handle.has_persistent_state() {
                let id = PersistentStatePartId::WholeState(handle.id().clone());
                let size = self.engine.load_persistent_state_size(&id).await?;
                let data = self.engine.load_persistent_state_slice(&id, 0, size).await?;
                let answer = TaggedByteVec {
                    object: data,
                    #[cfg(feature = "telemetry")]
                    tag: 0x8000000C, // Raw reply to download zero state
                };
                return Ok(answer);
            }
        }
        fail!("Zero state {} is not inited", query.block)
    }

    async fn get_persistent_state_size(&self, query: GetPersistentStateSize) -> Result<TLObject> {
        self.get_persistent_state_size_internal(PersistentStatePartId::WholeState(query.block))
            .await
    }

    fn convert_pss_id(id: &PersistentStateIdV2) -> PersistentStatePartId {
        let shard_prefix = id.block.shard().shard_prefix_with_tag();
        let part_prefix = id.effective_shard as u64;
        match part_prefix {
            0 => PersistentStatePartId::WholeState(id.block.clone()),
            _ if part_prefix == shard_prefix => PersistentStatePartId::Head(id.block.clone()),
            _ => PersistentStatePartId::Part(id.block.clone(), part_prefix),
        }
    }

    async fn get_persistent_state_size_v2(
        &self,
        query: GetPersistentStateSizeV2,
    ) -> Result<TLObject> {
        self.get_persistent_state_size_internal(Self::convert_pss_id(&query.state)).await
    }

    async fn get_persistent_state_size_internal(
        &self,
        id: PersistentStatePartId,
    ) -> Result<TLObject> {
        if let Ok(size) = self.engine.load_persistent_state_size(&id).await {
            let answer = ton_node::persistentstatesize::PersistentStateSize { size: size as i64 }
                .into_boxed();
            Ok(answer.into_tl_object())
        } else {
            Ok(ton_node::PersistentStateSize::TonNode_PersistentStateSizeNotFound.into_tl_object())
        }
    }

    async fn download_block_proof_internal(
        &self,
        block_id: BlockIdExt,
        is_link: bool,
        _key_block: bool,
    ) -> Result<TaggedByteVec> {
        if let Some(handle) = self.engine.load_block_handle(&block_id)? {
            if (is_link && handle.has_proof_link()) || (!is_link && handle.has_proof()) {
                let answer = TaggedByteVec {
                    object: self.engine.load_block_proof_raw(&handle, is_link).await?,
                    #[cfg(feature = "telemetry")]
                    tag: 0x8000000D, // Raw reply to download proof
                };
                return Ok(answer);
            }
        }
        if is_link {
            fail!("Block's proof link isn't initialized")
        } else {
            fail!("Block's proof isn't initialized")
        }
    }

    // tonNode.downloadBlockProof block:tonNode.blockIdExt = tonNode.Data;
    async fn download_block_proof(&self, query: DownloadBlockProof) -> Result<TaggedByteVec> {
        self.download_block_proof_internal(query.block, false, false).await
    }

    // tonNode.downloadKeyBlockProof block:tonNode.blockIdExt = tonNode.Data;
    async fn download_key_block_proof(
        &self,
        query: DownloadKeyBlockProof,
    ) -> Result<TaggedByteVec> {
        self.download_block_proof_internal(query.block, false, true).await
    }

    // tonNode.downloadBlockProofs blocks:(vector tonNode.blockIdExt) = tonNode.DataList;
    // Not supported in t-node

    // tonNode.downloadKeyBlockProofs blocks:(vector tonNode.blockIdExt) = tonNode.DataList;
    // Not supported in t-node

    // tonNode.downloadBlockProofLink block:tonNode.blockIdExt = tonNode.Data;
    async fn download_block_proof_link(
        &self,
        query: DownloadBlockProofLink,
    ) -> Result<TaggedByteVec> {
        self.download_block_proof_internal(query.block, true, false).await
    }

    // tonNode.downloadKeyBlockProofLink block:tonNode.blockIdExt = tonNode.Data;
    async fn download_key_block_proof_link(
        &self,
        query: DownloadKeyBlockProofLink,
    ) -> Result<TaggedByteVec> {
        self.download_block_proof_internal(query.block, true, true).await
    }

    // tonNode.downloadBlockProofLinks blocks:(vector tonNode.blockIdExt) = tonNode.DataList;
    // Not supported in t-node

    // tonNode.downloadKeyBlockProofLinks blocks:(vector tonNode.blockIdExt) = tonNode.DataList;
    // Not supported in t-node

    async fn get_any_archive_info(&self, mc_seq_no: u32, shard: ShardIdent) -> Result<TLObject> {
        let mut answer = ArchiveInfoBoxed::TonNode_ArchiveNotFound;
        if let Some(id) = self.engine.load_last_applied_mc_block_id()? {
            if mc_seq_no <= id.seq_no() {
                if let Some(id) = self.engine.load_shard_client_mc_block_id()? {
                    if mc_seq_no <= id.seq_no() {
                        if let Some(id) = self.engine.get_archive_id(mc_seq_no, &shard).await {
                            answer = ArchiveInfo { id: id as ton::long }.into_boxed()
                        }
                    }
                }
            }
        }
        Ok(answer.into_tl_object())
    }

    // tonNode.getArchiveInfo masterchain_seqno:int = tonNode.ArchiveInfo;
    async fn get_archive_info(&self, query: GetArchiveInfo) -> Result<TLObject> {
        self.get_any_archive_info(query.masterchain_seqno as u32, ShardIdent::masterchain()).await
    }

    // tonNode.getShardArchiveInfo masterchain_seqno:int shard_prefix:tonNode.shardId
    //   = tonNode.ArchiveInfo;
    async fn get_shard_archive_info(&self, query: GetShardArchiveInfo) -> Result<TLObject> {
        let shard = ShardIdent::with_tagged_prefix(
            query.shard_prefix.workchain,
            query.shard_prefix.shard as u64,
        )?;
        self.get_any_archive_info(query.masterchain_seqno as u32, shard).await
    }

    // tonNode.getArchiveSlice archive_id:long offset:long max_size:int = tonNode.Data;
    async fn get_archive_slice(&self, query: GetArchiveSlice) -> Result<TaggedByteVec> {
        if query.max_size as usize > PART_MAX_SIZE {
            fail!("Part size {} is too big, max is {}", query.max_size, PART_MAX_SIZE);
        }
        let answer = TaggedByteVec {
            object: self
                .engine
                .get_archive_slice(
                    query.archive_id as u64,
                    query.offset as u64,
                    query.max_size as u32,
                )
                .await?,
            #[cfg(feature = "telemetry")]
            tag: 0x8000000E, // Raw reply to download archive slice
        };
        Ok(answer)
    }

    // tonNode.getCapabilities = tonNode.Capabilities;
    async fn get_capabilities(&self, _query: GetCapabilities) -> Result<TLObject> {
        let answer = Capabilities {
            version_major: PROTOCOL_VERSION_MAJOR,
            version_minor: PROTOCOL_VERSION_MINOR,
        }
        .into_boxed();
        Ok(answer.into_tl_object())
    }

    async fn consume_query<'a, Q, F>(
        &'a self,
        query: TLObject,
        consumer: &'a (dyn Fn(&'a Self, Q) -> F + Send + Sync),
    ) -> Result<std::result::Result<QueryResult, TLObject>>
    where
        Q: AnyBoxedSerialize + Debug,
        F: futures::Future<Output = Result<TLObject>>,
    {
        Ok(match query.downcast::<Q>() {
            Ok(query) => {
                let query_str =
                    if log::log_enabled!(log::Level::Trace) || cfg!(feature = "telemetry") {
                        std::any::type_name::<Q>().to_string()
                    } else {
                        String::default()
                    };
                log::trace!("consume_query: before consume query {}", query_str);
                #[cfg(feature = "telemetry")]
                let now = std::time::Instant::now();
                let answer = match consumer(self, query).await {
                    Ok(answer) => {
                        let answer = TaggedByteVec {
                            object: serialize_boxed(&answer)?,
                            #[cfg(feature = "telemetry")]
                            tag: answer.bare_object().constructor(),
                        };
                        log::trace!("consume_query: consumed {}", query_str);
                        #[cfg(feature = "telemetry")]
                        self.engine.full_node_service_telemetry().consumed_query(
                            query_str,
                            true,
                            now.elapsed(),
                            answer.object.len(),
                        );
                        answer
                    }
                    Err(e) => {
                        log::warn!("consume_query: consumed {}, error {:?}", query_str, e);
                        #[cfg(feature = "telemetry")]
                        self.engine.full_node_service_telemetry().consumed_query(
                            query_str,
                            false,
                            now.elapsed(),
                            0,
                        );
                        return Err(e);
                    }
                };
                Ok(QueryResult::Consumed(QueryAnswer::Ready(Some(Answer::Raw(answer)))))
            }
            Err(query) => Err(query),
        })
    }

    async fn consume_query_raw<'a, Q, F>(
        &'a self,
        query: TLObject,
        consumer: &'a (dyn Fn(&'a Self, Q) -> F + Send + Sync),
    ) -> Result<std::result::Result<QueryResult, TLObject>>
    where
        Q: AnyBoxedSerialize + Debug,
        F: futures::Future<Output = Result<TaggedByteVec>>,
    {
        Ok(match query.downcast::<Q>() {
            Ok(query) => {
                let query_str =
                    if log::log_enabled!(log::Level::Trace) || cfg!(feature = "telemetry") {
                        std::any::type_name::<Q>().to_string()
                    } else {
                        String::default()
                    };
                log::trace!("consume_query_raw: before consume query {}", query_str);
                #[cfg(feature = "telemetry")]
                let now = std::time::Instant::now();
                let answer = match consumer(self, query).await {
                    Ok(answer) => {
                        #[cfg(feature = "telemetry")]
                        log::trace!("consume_query_raw: consumed {}", query_str);
                        #[cfg(feature = "telemetry")]
                        self.engine.full_node_service_telemetry().consumed_query(
                            query_str,
                            true,
                            now.elapsed(),
                            answer.object.len(),
                        );
                        answer
                    }
                    Err(e) => {
                        #[cfg(feature = "telemetry")]
                        log::trace!("consume_query_raw: consumed {}, error {:?}", query_str, e);
                        #[cfg(feature = "telemetry")]
                        self.engine.full_node_service_telemetry().consumed_query(
                            query_str,
                            false,
                            now.elapsed(),
                            0,
                        );
                        return Err(e);
                    }
                };
                Ok(QueryResult::Consumed(QueryAnswer::Ready(Some(Answer::Raw(answer)))))
            }
            Err(query) => Err(query),
        })
    }
}

#[async_trait::async_trait]
impl Subscriber for FullNodeOverlayService {
    #[allow(dead_code)]
    async fn try_consume_query(
        &self,
        query: TLObject,
        adnl_peers: &AdnlPeers,
    ) -> Result<QueryResult> {
        log::debug!("try_consume_query {:?} from {}", query, adnl_peers.other());

        let query = match self
            .consume_query::<GetNextBlockDescription, _>(query, &Self::get_next_block_description)
            .await?
        {
            Ok(answer) => return Ok(answer),
            Err(query) => query,
        };

        let query = match self
            .consume_query::<PrepareBlockProof, _>(query, &Self::prepare_block_proof)
            .await?
        {
            Ok(answer) => return Ok(answer),
            Err(query) => query,
        };

        let query = match self
            .consume_query::<PrepareKeyBlockProof, _>(query, &Self::prepare_key_block_proof)
            .await?
        {
            Ok(answer) => return Ok(answer),
            Err(query) => query,
        };

        let query = match self.consume_query::<PrepareBlock, _>(query, &Self::prepare_block).await?
        {
            Ok(answer) => return Ok(answer),
            Err(query) => query,
        };

        let query = match self
            .consume_query::<PreparePersistentState, _>(query, &Self::prepare_persistent_state)
            .await?
        {
            Ok(answer) => return Ok(answer),
            Err(query) => query,
        };

        let query = match self
            .consume_query::<PrepareZeroState, _>(query, &Self::prepare_zero_state)
            .await?
        {
            Ok(answer) => return Ok(answer),
            Err(query) => query,
        };

        let query = match self
            .consume_query::<GetNextKeyBlockIds, _>(query, &Self::get_next_key_block_ids)
            .await?
        {
            Ok(answer) => return Ok(answer),
            Err(query) => query,
        };

        let query = match self
            .consume_query::<DownloadNextBlockFull, _>(query, &Self::download_next_block_full)
            .await?
        {
            Ok(answer) => return Ok(answer),
            Err(query) => query,
        };

        let query = match self
            .consume_query::<DownloadBlockFull, _>(query, &Self::download_block_full)
            .await?
        {
            Ok(answer) => return Ok(answer),
            Err(query) => query,
        };

        let query =
            match self.consume_query_raw::<DownloadBlock, _>(query, &Self::download_block).await? {
                Ok(answer) => return Ok(answer),
                Err(query) => query,
            };

        let query = match self
            .consume_query_raw::<DownloadPersistentState, _>(
                query,
                &Self::download_persistent_state,
            )
            .await?
        {
            Ok(answer) => return Ok(answer),
            Err(query) => query,
        };

        let query = match self
            .consume_query_raw::<DownloadPersistentStateSlice, _>(
                query,
                &Self::download_persistent_state_slice,
            )
            .await?
        {
            Ok(answer) => return Ok(answer),
            Err(query) => query,
        };

        let query = match self
            .consume_query_raw::<DownloadZeroState, _>(query, &Self::download_zero_state)
            .await?
        {
            Ok(answer) => return Ok(answer),
            Err(query) => query,
        };

        let query = match self
            .consume_query_raw::<DownloadBlockProof, _>(query, &Self::download_block_proof)
            .await?
        {
            Ok(answer) => return Ok(answer),
            Err(query) => query,
        };

        let query = match self
            .consume_query_raw::<DownloadKeyBlockProof, _>(query, &Self::download_key_block_proof)
            .await?
        {
            Ok(answer) => return Ok(answer),
            Err(query) => query,
        };

        let query = match self
            .consume_query_raw::<DownloadBlockProofLink, _>(query, &Self::download_block_proof_link)
            .await?
        {
            Ok(answer) => return Ok(answer),
            Err(query) => query,
        };

        let query = match self
            .consume_query_raw::<DownloadKeyBlockProofLink, _>(
                query,
                &Self::download_key_block_proof_link,
            )
            .await?
        {
            Ok(answer) => return Ok(answer),
            Err(query) => query,
        };

        let query =
            match self.consume_query::<GetArchiveInfo, _>(query, &Self::get_archive_info).await? {
                Ok(answer) => return Ok(answer),
                Err(query) => query,
            };

        let query = match self
            .consume_query_raw::<GetArchiveSlice, _>(query, &Self::get_archive_slice)
            .await?
        {
            Ok(answer) => return Ok(answer),
            Err(query) => query,
        };

        let query =
            match self.consume_query::<GetCapabilities, _>(query, &Self::get_capabilities).await? {
                Ok(answer) => return Ok(answer),
                Err(query) => query,
            };

        let query = match self
            .consume_query::<GetPersistentStateSize, _>(query, &Self::get_persistent_state_size)
            .await?
        {
            Ok(answer) => return Ok(answer),
            Err(query) => query,
        };

        let query = match self
            .consume_query_raw::<DownloadPersistentStateSliceV2, _>(
                query,
                &Self::download_persistent_state_slice_v2,
            )
            .await?
        {
            Ok(answer) => return Ok(answer),
            Err(query) => query,
        };

        let query = match self
            .consume_query::<GetPersistentStateSizeV2, _>(
                query,
                &Self::get_persistent_state_size_v2,
            )
            .await?
        {
            Ok(answer) => return Ok(answer),
            Err(query) => query,
        };

        let query = match self
            .consume_query::<GetShardArchiveInfo, _>(query, &Self::get_shard_archive_info)
            .await?
        {
            Ok(answer) => return Ok(answer),
            Err(query) => query,
        };

        log::warn!("Unsupported full node query {:?}", query);
        fail!("Unsupported full node query {:?}", query);
    }
}
