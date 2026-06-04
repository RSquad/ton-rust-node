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
use super::*;
use crate::{
    boot::cold_boot,
    collator_test_bundle::{create_block_handle_storage, create_engine_allocated},
    config::TonNodeGlobalConfig,
    engine::EngineFlags,
    engine_traits::EngineAlloc,
    internal_db::{BlockResult, DataStatus},
    network::{
        full_node_overlay_client::FullNodeOverlayClient,
        full_node_overlays::FullNodeOverlaysRouter, node_network::NodeNetwork,
        telemetry::FullNodeNetworkTelemetry,
    },
    shard_state::ShardStateStuff,
    test_helper::{
        gen_master_state, gen_shard_state, get_config, init_test_log, GenMasterStateParams,
    },
};
#[cfg(feature = "telemetry")]
use crate::{collator_test_bundle::create_engine_telemetry, engine_traits::EngineTelemetry};
use adnl::DhtSearchPolicy;
use std::{
    borrow::Borrow,
    fs::{read_to_string, remove_dir_all, write},
    path::PathBuf,
    str::FromStr,
    sync::{
        atomic::{AtomicU32, AtomicU8, Ordering},
        Mutex, OnceLock,
    },
};
use storage::{
    archives::{
        archive_manager::ArchiveManager,
        db_provider::{ArchiveDbProvider, SingleDbProvider},
    },
    block_handle_db::BlockHandleStorage,
    db::rocksdb::{AccessType, RocksDb},
    types::{BlockMeta, PersistentStatePartId},
};
use ton_api::ton::ton_node::broadcast::{
    BlockBroadcast, ExternalMessageBroadcast, NewShardBlockBroadcast,
};
use ton_block::{AccountIdPrefixFull, ConfigParams, KeyId, UInt256};

#[tokio::test]
async fn test_read_package() -> Result<()> {
    let data = tokio::fs::read("src/tests/static/archive.1459995.pack").await?;
    let maps = read_package(&data).await?;

    assert_eq!(maps.mc_blocks_ids.len(), 5);
    assert_eq!(maps.blocks.len(), 155);

    for (id, entry) in maps.blocks.iter() {
        if id.is_masterchain() {
            assert!(maps.mc_blocks_ids.contains_key(&id.seq_no()));
        }
        assert!(entry.block.is_some());
        assert!(entry.proof.is_some());
    }
    Ok(())
}

#[ignore]
#[tokio::test(flavor = "multi_thread")]
async fn test_sync() -> Result<()> {
    const DEFAULT_DST_CONFIG: &str = "../target/default_config_sync.json";
    const DEFAULT_SRC_CONFIG: &str = "./configs/default_config_localhost.json";
    const IP_ADDR: &str = "0.0.0.0:30303";

    struct Checker;
    #[async_trait::async_trait]
    impl StopSyncChecker for Checker {
        async fn check(&self, engine: &Arc<dyn EngineOperations>) -> bool {
            engine.check_sync().await.unwrap_or(false)
        }
    }

    struct TestEngine {
        allocated: Arc<EngineAlloc>,
        applied_blocks: AtomicU32,
        archive_manager: Arc<ArchiveManager>,
        block_handle_storage: BlockHandleStorage,
        cancellation_token: tokio_util::sync::CancellationToken,
        init_mc_block_id: BlockIdExt,
        last_mc_block_id: Mutex<BlockIdExt>,
        blocks: lockfree::map::Map<BlockIdExt, BlockStuff>,
        mc_block_ids: lockfree::map::Map<u32, BlockIdExt>,
        monitor_min_split: Arc<AtomicU8>,
        overlays_router: OnceLock<Arc<FullNodeOverlaysRouter>>,
        proofs: lockfree::map::Map<BlockIdExt, BlockProofStuff>,
        shard_client_mc_block_id: Mutex<BlockIdExt>,
        states: lockfree::map::Map<ShardIdent, Arc<ShardStateStuff>>,
        #[cfg(feature = "telemetry")]
        telemetry: Arc<EngineTelemetry>,
        #[cfg(feature = "telemetry")]
        telemetry_service: FullNodeNetworkTelemetry,
        zerostate_id: BlockIdExt,
    }

    impl TestEngine {
        async fn new(init_id: Option<BlockIdExt>) -> Result<Self> {
            const DB_PATH: &str = "../target/test_sync";
            remove_dir_all(DB_PATH).ok();
            let config = TonNodeGlobalConfig::from_json_file("./src/tests/config/mainnet.json")?;
            let zerostate_id = config.zero_state()?;
            let init_mc_block_id = config.init_block()?;
            let init_mc_block_id =
                init_id.unwrap_or_else(|| init_mc_block_id.unwrap_or(zerostate_id.clone()));
            let monitor_min_split = Arc::new(AtomicU8::new(0));
            let db = RocksDb::new(DB_PATH, "db", None, AccessType::ReadWrite)?;
            let allocated = create_engine_allocated();
            #[cfg(feature = "telemetry")]
            let telemetry = create_engine_telemetry();
            let db_root_path = Arc::new(PathBuf::from(DB_PATH));
            let db_provider: Arc<dyn ArchiveDbProvider> =
                Arc::new(SingleDbProvider::new(db.clone(), db_root_path.clone()));
            let archive_manager = ArchiveManager::with_data(
                db.clone(),
                db_root_path,
                db_provider,
                init_mc_block_id.seq_no(),
                monitor_min_split.clone(),
                #[cfg(feature = "telemetry")]
                telemetry.storage.clone(),
                allocated.storage.clone(),
            )
            .await?;
            let ret = Self {
                allocated,
                applied_blocks: AtomicU32::new(0),
                archive_manager: Arc::new(archive_manager),
                block_handle_storage: create_block_handle_storage(Some(db))?,
                cancellation_token: tokio_util::sync::CancellationToken::new(),
                last_mc_block_id: Mutex::new(init_mc_block_id.clone()),
                shard_client_mc_block_id: Mutex::new(init_mc_block_id.clone()),
                init_mc_block_id,
                blocks: lockfree::map::Map::new(),
                mc_block_ids: lockfree::map::Map::new(),
                monitor_min_split,
                overlays_router: OnceLock::new(),
                proofs: lockfree::map::Map::new(),
                states: lockfree::map::Map::new(),
                #[cfg(feature = "telemetry")]
                telemetry,
                #[cfg(feature = "telemetry")]
                telemetry_service: FullNodeNetworkTelemetry::new_service(),
                zerostate_id,
            };
            Ok(ret)
        }

        async fn get_client(&self, shard: &ShardIdent) -> Result<Arc<FullNodeOverlayClient>> {
            self.overlays_router()?.overlay_client(shard).await
        }

        async fn on_store<B: Borrow<BlockIdExt>>(
            &self,
            handle: &Arc<BlockHandle>,
            entry_id: PackageEntryId<B>,
            data: &[u8],
            checker: impl Fn(&Arc<BlockHandle>) -> bool,
            updater: impl Fn(&Arc<BlockHandle>) -> bool,
        ) -> Result<()> {
            if !checker(handle) || !self.archive_manager.check_file(&handle, &entry_id) {
                let _lock = handle.block_file_lock().write().await;
                if !checker(handle) || !self.archive_manager.check_file(&handle, &entry_id) {
                    self.archive_manager.add_file(&entry_id, data).await?;
                    if updater(handle) {
                        self.block_handle_storage.save_handle(handle, None)?
                    }
                }
            }
            Ok(())
        }

        fn overlays_router(&self) -> Result<&Arc<FullNodeOverlaysRouter>> {
            self.overlays_router.get().ok_or_else(|| error!("Iverlays router is not initialized"))
        }

        async fn update_monitor_min_split(
            &self,
            key_block_id: &BlockIdExt,
            config: &ConfigParams,
        ) -> Result<()> {
            let new = config.base_workchain()?.monitor_min_split();
            self.monitor_min_split.swap(new, Ordering::Relaxed);
            self.overlays_router()?.update_public_overlays(key_block_id, new).await
        }
    }

    #[async_trait::async_trait]
    impl EngineOperations for TestEngine {
        async fn apply_block_internal(
            self: Arc<Self>,
            handle: &Arc<BlockHandle>,
            _block: &BlockStuff,
            mc_seq_no: u32,
            pre_apply: bool,
            _recursion_depth: u32,
        ) -> Result<()> {
            if !pre_apply {
                self.set_applied(handle, mc_seq_no).await?;
            }
            Ok(())
        }

        async fn apply_persistent_state(
            &self,
            handle: &Arc<BlockHandle>,
            _root_hash: &UInt256,
            data: Arc<Vec<u8>>,
            cells_index: Vec<(UInt256, u16)>,
        ) -> Result<(Arc<ShardStateStuff>, Vec<(UInt256, u16)>)> {
            if let Some(state) = self.states.get(handle.id().shard()) {
                Ok((state.val().clone(), cells_index))
            } else {
                let state = ShardStateStuff::deserialize_state(
                    handle.id().clone(),
                    &data,
                    #[cfg(feature = "telemetry")]
                    &self.telemetry,
                    &self.allocated,
                )?;
                self.states.insert(handle.id().shard().clone(), state.clone());
                Ok((state, cells_index))
            }
        }

        fn check_stop(&self) -> bool {
            self.cancellation_token.is_cancelled()
        }

        async fn check_sync(&self) -> Result<bool> {
            Ok(self.applied_blocks.load(Ordering::Relaxed) > 500)
        }

        async fn cleanup_persistent_states(&self) -> Result<()> {
            Ok(())
        }

        async fn download_and_apply_block_internal(
            self: Arc<Self>,
            _id: &BlockIdExt,
            _mc_seq_no: u32,
            _pre_apply: bool,
            _recursion_depth: u32,
        ) -> Result<()> {
            fail!("No per-block download allowed")
        }

        async fn download_archive(
            &self,
            shard: Option<ShardIdent>,
            masterchain_seqno: u32,
        ) -> Result<Option<Vec<u8>>> {
            let shard = shard.unwrap_or_else(|| ShardIdent::masterchain());
            self.get_client(&shard).await?.download_archive(masterchain_seqno, &shard).await
        }

        async fn download_block(
            &self,
            id: &BlockIdExt,
            limit: Option<u32>,
        ) -> Result<(BlockStuff, BlockProofStuff)> {
            let client = self.get_client(&id.shard_id).await?;
            let mut attempts = 1;
            loop {
                match client.download_block_full(id, false).await {
                    Ok(ret) => break Ok(ret),
                    Err(e) => println!("Error downloading block {}: {}", id, e),
                }
                if let Some(limit) = limit {
                    if attempts >= limit {
                        fail!("Out of attempts ({}) when downloading block {}", attempts, id)
                    }
                }
                attempts += 1;
            }
        }

        async fn download_block_proof(
            &self,
            id: &BlockIdExt,
            is_link: bool,
            key_block: bool,
        ) -> Result<BlockProofStuff> {
            self.get_client(&id.shard_id).await?.download_block_proof(id, is_link, key_block).await
        }

        async fn download_next_key_blocks_ids(&self, id: &BlockIdExt) -> Result<Vec<BlockIdExt>> {
            self.get_client(&id.shard_id).await?.download_next_key_blocks_ids(id, 100).await
        }

        async fn download_persistent_state(
            &self,
            _id: PersistentStatePartId,
            _master_id: &BlockIdExt,
            _attempts: Option<usize>,
        ) -> Result<(usize, usize)> {
            // (bytes, cells_count)
            Ok((0, 0))
        }

        fn engine_allocated(&self) -> &Arc<EngineAlloc> {
            &self.allocated
        }

        #[cfg(feature = "telemetry")]
        fn engine_telemetry(&self) -> &Arc<EngineTelemetry> {
            &self.telemetry
        }

        async fn lookup_block_by_seqno(
            &self,
            prefix: &AccountIdPrefixFull,
            seqno: u32,
        ) -> Result<Option<(BlockIdExt, Vec<u8>)>> {
            if !prefix.is_masterchain() {
                fail!("Only masterchain lookup is supported");
            }
            if let Some(id) = self.mc_block_ids.get(&seqno) {
                return Ok(Some((id.val().clone(), vec![])));
            }
            fail!("No MC block {} is available", seqno)
        }

        fn flags(&self) -> &EngineFlags {
            const FLAGS: EngineFlags = EngineFlags {
                initial_sync_disabled: false,
                force_check_db: false,
                truncate_db: None,
            };
            &FLAGS
        }

        #[cfg(feature = "telemetry")]
        fn full_node_service_telemetry(&self) -> &FullNodeNetworkTelemetry {
            &self.telemetry_service
        }

        fn get_monitor_min_split(&self) -> u8 {
            self.monitor_min_split.load(Ordering::Relaxed)
        }

        fn init_mc_block_id(&self) -> &BlockIdExt {
            &self.init_mc_block_id
        }

        fn hardforks(&self) -> &[BlockIdExt] {
            &[]
        }

        async fn load_block(&self, handle: &BlockHandle) -> Result<BlockStuff> {
            let Some(block) = self.blocks.get(handle.id()) else {
                fail!("Cannot load block {}", handle.id())
            };
            Ok(block.val().clone())
        }

        fn load_block_handle(&self, id: &BlockIdExt) -> Result<Option<Arc<BlockHandle>>> {
            self.block_handle_storage.load_handle_by_id(id)
        }

        async fn load_block_proof(
            &self,
            handle: &Arc<BlockHandle>,
            _is_link: bool,
        ) -> Result<BlockProofStuff> {
            let Some(proof) = self.proofs.get(handle.id()) else {
                fail!("Cannot load proof for block {}", handle.id())
            };
            Ok(proof.val().clone())
        }

        fn load_last_applied_mc_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
            match self.last_mc_block_id.lock() {
                Ok(load) => Ok(Some(Arc::new(load.clone()))),
                Err(_) => fail!("Cannot load last applied MC block"),
            }
        }

        async fn load_last_applied_mc_state(&self) -> Result<Arc<ShardStateStuff>> {
            let Some(ret) = self.states.get(&ShardIdent::masterchain()) else {
                fail!("No last applied MC state")
            };
            Ok(ret.val().clone())
        }

        async fn load_persistent_state_to(
            &self,
            id: &PersistentStatePartId,
            buffer: &mut Vec<u8>,
        ) -> Result<()> {
            if !matches!(id, PersistentStatePartId::WholeState(_)) {
                fail!("Only whole state is supported in this test")
            }
            let id = id.block_id();
            let state = if id.shard().is_masterchain() {
                let (block, _) = self.download_block(id, None).await?;
                let config = block.read_config_params()?;
                self.update_monitor_min_split(id, &config).await?;
                let state = gen_master_state(
                    GenMasterStateParams {
                        config,
                        master_state_id: Some(id.clone()),
                        shard_state_id: Some(id.clone()),
                        ..Default::default()
                    },
                    #[cfg(feature = "telemetry")]
                    Some(self.telemetry.clone()),
                    Some(self.allocated.clone()),
                );
                state
            } else {
                let (_, state) = gen_shard_state(
                    Some(id.clone()),
                    &[],
                    #[cfg(feature = "telemetry")]
                    Some(self.telemetry.clone()),
                    Some(self.allocated.clone()),
                    None,
                );
                state
            };
            self.states.insert(id.shard().clone(), state.clone());
            state.write_to(buffer)
        }

        fn load_shard_client_mc_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
            match self.shard_client_mc_block_id.lock() {
                Ok(load) => Ok(Some(Arc::new(load.clone()))),
                Err(_) => fail!("Cannot load shard client MC block"),
            }
        }

        fn process_block_broadcast(self: Arc<Self>, _broadcast: BlockBroadcast, _src: Arc<KeyId>) {}

        async fn process_ext_msg_broadcast(
            &self,
            _broadcast: ExternalMessageBroadcast,
            _src: Arc<KeyId>,
        ) {
        }

        fn process_new_shard_block_broadcast(
            self: Arc<Self>,
            _broadcast: NewShardBlockBroadcast,
            _src: Arc<KeyId>,
        ) {
        }

        fn save_last_applied_mc_block_id(&self, last_mc_block: &BlockIdExt) -> Result<()> {
            match self.last_mc_block_id.lock() {
                Ok(mut save) => *save = last_mc_block.clone(),
                Err(_) => fail!("Cannot save last applied MC block"),
            }
            Ok(())
        }

        fn save_shard_client_mc_block_id(&self, id: &BlockIdExt) -> Result<()> {
            match self.shard_client_mc_block_id.lock() {
                Ok(mut save) => *save = id.clone(),
                Err(_) => fail!("Cannot save shard client MC block"),
            }
            Ok(())
        }

        async fn set_applied(&self, handle: &Arc<BlockHandle>, mc_seq_no: u32) -> Result<bool> {
            let ret = handle.set_block_applied();
            if ret {
                if handle.id().shard().is_masterchain() {
                    let id = handle.id().clone();
                    self.mc_block_ids.insert(id.seq_no(), id);
                    self.applied_blocks.fetch_add(1, Ordering::Relaxed);
                    if handle.is_key_block()? {
                        let config = self.load_block(handle).await?.read_config_params()?;
                        self.update_monitor_min_split(handle.id(), &config).await?;
                    }
                } else {
                    handle.set_masterchain_ref_seq_no(mc_seq_no)?;
                }
                self.block_handle_storage.save_handle(handle, None)?;
                self.archive_manager.move_to_archive(handle.as_ref(), || Ok(())).await?;
            }
            Ok(ret)
        }

        fn set_sync_status(&self, _status: u32) {}

        async fn store_block(&self, block: &BlockStuff) -> Result<BlockResult> {
            let handle =
                if let Some(handle) = self.block_handle_storage.load_handle_by_id(block.id())? {
                    handle
                } else if let Some(handle) = self.block_handle_storage.create_handle(
                    block.id().clone(),
                    BlockMeta::from_block(block.block()?)?,
                    None,
                )? {
                    handle
                } else {
                    fail!("Cannot create or load block handle for {}", block.id())
                };
            self.blocks.insert(block.id().clone(), block.clone());
            self.on_store(
                &handle,
                PackageEntryId::Block(block.id()),
                block.data(),
                |handle| handle.has_data(),
                |handle| handle.set_data(),
            )
            .await?;
            Ok(BlockResult::with_status(handle, DataStatus::Updated))
        }

        async fn store_block_proof(
            &self,
            id: &BlockIdExt,
            handle: Option<Arc<BlockHandle>>,
            proof: &BlockProofStuff,
        ) -> Result<BlockResult> {
            let handle = if let Some(handle) = handle {
                handle
            } else {
                let (virt_block, _) = proof.virtualize_block()?;
                self.block_handle_storage
                    .create_handle(id.clone(), BlockMeta::from_block(&virt_block)?, None)?
                    .ok_or_else(|| error!("Cannot create block handle for {}", id))?
            };
            self.proofs.insert(id.clone(), proof.clone());
            let is_link = proof.is_link();
            self.on_store(
                &handle,
                if is_link { PackageEntryId::ProofLink(id) } else { PackageEntryId::Proof(id) },
                proof.data(),
                |handle| if is_link { handle.has_proof_link() } else { handle.has_proof() },
                |handle| if is_link { handle.set_proof_link() } else { handle.set_proof() },
            )
            .await?;
            Ok(BlockResult::with_status(handle, DataStatus::Updated))
        }

        fn zerostate_id(&self) -> Result<&BlockIdExt> {
            Ok(&self.zerostate_id)
        }
    }

    init_test_log();

    let init_id = BlockIdExt {
        shard_id: ShardIdent::masterchain(),
        seq_no: 48295457,
        root_hash: UInt256::from_str(
            "ced9e07153ff4033261bec1217946953aa61f81cd1a979282bbaf855dee56aa9",
        )?,
        file_hash: UInt256::from_str(
            "b7156ca9d6bce20ad50f8e89fd6b3b44b231cacc0c1f15f7c43c762889fd5c8b",
        )?,
    };
    let engine = Arc::new(TestEngine::new(Some(init_id)).await?);

    let mut json: serde_json::Value = serde_json::from_str(&read_to_string(DEFAULT_SRC_CONFIG)?)?;
    let Some(global_config) = json.get_mut("ton_global_config_name") else {
        fail!("No global config name in default config")
    };
    *global_config = serde_json::Value::String("../node/src/tests/config/mainnet.json".to_string());
    write(DEFAULT_DST_CONFIG, serde_json::to_string_pretty(&json)?)?;
    let config = get_config(IP_ADDR, Some("../target"), None, DEFAULT_DST_CONFIG).await?;
    let network = NodeNetwork::new(
        config,
        engine.cancellation_token.clone(),
        #[cfg(feature = "telemetry")]
        engine.telemetry.clone(),
        engine.allocated.clone(),
    )
    .await?;
    network.start().await?;

    let overlays_router =
        FullNodeOverlaysRouter::new(engine.clone(), network, DhtSearchPolicy::FastSearch(5))
            .await?;
    engine
        .overlays_router
        .set(overlays_router)
        .map_err(|_| error!("Overlays router was already set"))?;

    let mc_handle = cold_boot(engine.clone(), 0).await?;
    engine.save_shard_client_mc_block_id(mc_handle.id())?;
    start_sync(Arc::clone(&engine) as Arc<dyn EngineOperations>, Some(&Checker), Some(5)).await
}
