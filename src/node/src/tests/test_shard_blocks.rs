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
#[cfg(feature = "telemetry")]
use crate::collator_test_bundle::create_engine_telemetry;
use crate::{
    collator_test_bundle::{create_block_handle_storage, create_engine_allocated},
    test_helper::{gen_master_state, GenMasterStateParams},
};
use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
};
use storage::{
    block_handle_db::{BlockHandle, BlockHandleStorage},
    db::rocksdb::{AccessType, RocksDb},
    types::BlockMeta,
};
use ton_block::{BlockIdExt, ShardIdent, UInt256};

struct TestEngine {
    pub last_applied_mc_block_seqno: AtomicU32,
    pub last_applied_mc_block_utime: AtomicU32,
    pub shard_blocks: std::sync::Mutex<HashSet<TopBlockDescrId>>,
    pub block_handle_storage: BlockHandleStorage,
    #[cfg(feature = "telemetry")]
    engine_telemetry: Arc<EngineTelemetry>,
    engine_allocated: Arc<EngineAlloc>,
}

#[async_trait::async_trait]
impl EngineOperations for TestEngine {
    fn load_block_handle(&self, id: &BlockIdExt) -> Result<Option<Arc<BlockHandle>>> {
        self.block_handle_storage.create_handle(
            id.clone(),
            BlockMeta::with_data(
                0,
                self.last_applied_mc_block_utime.load(Ordering::Relaxed),
                0,
                id.seq_no(),
                0,
            ),
            None,
        )
    }

    fn load_last_applied_mc_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
        let ret = BlockIdExt {
            shard_id: ShardIdent::masterchain(),
            seq_no: self.last_applied_mc_block_seqno.load(Ordering::Relaxed),
            root_hash: UInt256::default(),
            file_hash: UInt256::default(),
        };
        Ok(Some(Arc::new(ret)))
    }

    // Save tsb into persistent storage
    fn save_top_shard_block(&self, id: &TopBlockDescrId, _tsb: &TopBlockDescrStuff) -> Result<()> {
        self.shard_blocks.lock().unwrap().insert(id.clone());
        Ok(())
    }

    // Remove tsb from persistent storage
    fn remove_top_shard_block(&self, id: &TopBlockDescrId) -> Result<()> {
        self.shard_blocks.lock().unwrap().remove(id);
        Ok(())
    }

    #[cfg(feature = "telemetry")]
    fn engine_telemetry(&self) -> &Arc<EngineTelemetry> {
        &self.engine_telemetry
    }

    fn engine_allocated(&self) -> &Arc<EngineAlloc> {
        &self.engine_allocated
    }
}

fn build_id(shard: u64, seq_no: u32) -> BlockIdExt {
    BlockIdExt {
        shard_id: ShardIdent::with_tagged_prefix(0, shard).unwrap(),
        seq_no,
        root_hash: UInt256::default(),
        file_hash: UInt256::default(),
    }
}

#[tokio::test]
async fn test_shard_blocks_pool() {
    let db =
        RocksDb::new("../target", "test_shard_blocks_pool", None, AccessType::ReadWrite).unwrap();
    let engine = Arc::new(TestEngine {
        last_applied_mc_block_seqno: AtomicU32::new(0),
        last_applied_mc_block_utime: AtomicU32::new(0),
        block_handle_storage: create_block_handle_storage(Some(db)).unwrap(),
        shard_blocks: std::sync::Mutex::new(HashSet::default()),
        #[cfg(feature = "telemetry")]
        engine_telemetry: create_engine_telemetry(),
        engine_allocated: create_engine_allocated(),
    });
    let (pool, receiver) = ShardBlocksPool::new(
        HashMap::default(),
        1,
        true,
        #[cfg(feature = "telemetry")]
        &engine.engine_telemetry,
        &engine.engine_allocated,
    )
    .unwrap();
    save_top_shard_blocks_worker(engine.clone(), receiver);

    // TODO: prepare proper masterchain state
    let mc_ss = make_shard_state();

    if let ShardBlockProcessingResult::Duplication = pool
        .process_shard_block_raw(
            &build_id(0x8000_0000_0000_0000, 1),
            1,
            vec![],
            false,
            false,
            engine.as_ref(),
        )
        .await
        .unwrap()
    {
        panic!();
    }

    if let ShardBlockProcessingResult::Duplication = pool
        .process_shard_block_raw(
            &build_id(0x8000_0000_0000_0000, 2),
            1,
            vec![],
            false,
            false,
            engine.as_ref(),
        )
        .await
        .unwrap()
    {
        panic!();
    }

    let sb = pool.get_shard_blocks(&mc_ss, engine.as_ref(), false, None).await.unwrap();
    assert_eq!(sb.len(), 1);
    assert_eq!(sb[0].proof_for().seq_no(), 2);

    let result = pool
        .process_shard_block_raw(
            &build_id(0x8000_0000_0000_0000, 1),
            1,
            vec![],
            false,
            false,
            engine.deref(),
        )
        .await
        .unwrap();
    assert!(!matches!(result, ShardBlockProcessingResult::MightBeAdded(_)));

    // TODO: update state

    let sb = pool.get_shard_blocks(&mc_ss, engine.as_ref(), false, None).await.unwrap();
    assert_eq!(sb.len(), 1);
    assert_eq!(sb[0].proof_for().seq_no(), 2);

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(engine.shard_blocks.lock().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_shard_blocks_pool_threads() {
    let db =
        RocksDb::new("../target", "test_shard_blocks_pool_threads", None, AccessType::ReadWrite)
            .unwrap();
    let engine = Arc::new(TestEngine {
        last_applied_mc_block_seqno: AtomicU32::new(0),
        last_applied_mc_block_utime: AtomicU32::new(0),
        block_handle_storage: create_block_handle_storage(Some(db)).unwrap(),
        shard_blocks: std::sync::Mutex::new(HashSet::default()),
        #[cfg(feature = "telemetry")]
        engine_telemetry: create_engine_telemetry(),
        engine_allocated: create_engine_allocated(),
    });
    let (pool, receiver) = ShardBlocksPool::new(
        HashMap::default(),
        1,
        true,
        #[cfg(feature = "telemetry")]
        &engine.engine_telemetry,
        &engine.engine_allocated,
    )
    .unwrap();
    save_top_shard_blocks_worker(engine.clone(), receiver);
    let pool = Arc::new(pool);

    let threads = 500;
    let shards = 16;
    let catchains = 2;

    let mut tasks = vec![];
    for i in 0..threads {
        let pool = pool.clone();
        let engine = engine.clone() as Arc<dyn EngineOperations>;
        tasks.push(tokio::spawn(async move {
            for s in 0..shards {
                pool.process_shard_block_raw(
                    &build_id(s << 63_u64 | 1 << 62_u64, i + i % 8),
                    s as u32 * 100 + i / (threads / catchains),
                    vec![],
                    false,
                    false,
                    engine.deref(),
                )
                .await
                .unwrap();
            }
        }));
    }

    futures::future::join_all(tasks)
        .await
        .into_iter()
        .find(|r| r.is_err())
        .unwrap_or(Ok(()))
        .unwrap();

    // TODO: prepare proper masterchain state
    let mc_ss = make_shard_state();

    assert_eq!(
        pool.get_shard_blocks(&mc_ss, engine.as_ref(), false, None).await.unwrap().len() as u64,
        shards * catchains as u64
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(engine.shard_blocks.lock().unwrap().len(), (shards * catchains as u64) as usize);
}

fn make_shard_state() -> Arc<ShardStateStuff> {
    // TODO: prepare proper masterchain state
    let block_id = BlockIdExt {
        shard_id: ShardIdent::masterchain(),
        seq_no: 1,
        root_hash: Default::default(),
        file_hash: Default::default(),
    };
    let ss = gen_master_state(
        GenMasterStateParams { master_state_id: Some(block_id), ..Default::default() },
        #[cfg(feature = "telemetry")]
        None,
        None,
    );
    ss
}
