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
    block::BlockStuff,
    collator_test_bundle::{
        create_block_handle_storage, create_engine_allocated, CollatorTestBundle,
    },
    config::CollatorConfig,
    engine_traits::{EngineAlloc, EngineOperations},
    shard_state::ShardStateStuff,
    test_helper::{compare_blocks, init_test_log, test_async, TestEngine},
    types::messages::{count_matching_bits, MsgEnvelopeStuff},
};
#[cfg(feature = "telemetry")]
use crate::{collator_test_bundle::create_engine_telemetry, engine_traits::EngineTelemetry};
use pretty_assertions::assert_eq;
use std::{
    fs::{create_dir_all, remove_dir_all},
    sync::{Arc, Mutex},
};
use storage::{
    block_handle_db::{BlockHandle, BlockHandleStorage},
    types::BlockMeta,
};
use ton_block::{
    AccountIdPrefixFull, AccountStorageDictProof, BinTreeType, BocReader, InRefValue, MerkleProof,
    Result,
};

#[test]
fn test_cycle_vec() {
    let mut vec: CycleVec<'_, i32> = CycleVec::from_slice(&[]);
    assert_eq!(vec.move_next(), None);

    let mut vec = CycleVec::from_slice(&[1, 2]);
    assert_eq!(vec.move_next(), Some(&1));
    assert_eq!(vec.move_next(), Some(&2));
    vec.remove_current();
    assert_eq!(vec.move_next(), Some(&1));
    vec.remove_current();
    assert_eq!(vec.move_next(), None);
}

struct TestPipelineCollatorEngine {
    states: lockfree::map::Map<BlockIdExt, Arc<ShardStateStuff>>,
    blocks: lockfree::map::Map<BlockIdExt, BlockStuff>,
    last_mc_state: Mutex<BlockIdExt>,
    block_handle_storage: BlockHandleStorage,
    collator_config: CollatorConfig,
    #[cfg(feature = "telemetry")]
    telemetry: Arc<EngineTelemetry>,
    allocated: Arc<EngineAlloc>,
}

impl TestPipelineCollatorEngine {
    pub fn new() -> Self {
        Self {
            states: lockfree::map::Map::new(),
            blocks: lockfree::map::Map::new(),
            last_mc_state: Mutex::new(BlockIdExt::default()),
            block_handle_storage: create_block_handle_storage(None).unwrap(),
            collator_config: CollatorConfig::default(),
            #[cfg(feature = "telemetry")]
            telemetry: create_engine_telemetry(),
            allocated: create_engine_allocated(),
        }
    }

    pub fn add_state(&self, state: Arc<ShardStateStuff>) {
        self.states.insert(state.block_id().clone(), state);
    }

    pub fn add_block(&self, block: BlockStuff) {
        self.blocks.insert(block.id().clone(), block);
    }

    pub fn set_last_mc(&self, id: BlockIdExt) {
        self.last_mc_state.lock().unwrap().clone_from(&id);
    }
}

#[async_trait::async_trait]
impl EngineOperations for TestPipelineCollatorEngine {
    fn load_block_handle(&self, id: &BlockIdExt) -> Result<Option<Arc<BlockHandle>>> {
        let handle =
            self.block_handle_storage.create_handle(id.clone(), BlockMeta::default(), None)?;
        Ok(handle)
    }

    async fn load_block(&self, handle: &BlockHandle) -> Result<BlockStuff> {
        if let Some(s) = self.blocks.get(handle.id()) {
            return Ok(s.val().clone());
        }
        fail!("There is not block {}", handle.id())
    }

    async fn load_state(&self, block_id: &BlockIdExt) -> Result<Arc<ShardStateStuff>> {
        if let Some(s) = self.states.get(block_id) {
            return Ok(s.val().clone());
        }
        fail!("There is not state for block {}", block_id)
    }

    async fn load_last_applied_mc_state(&self) -> Result<Arc<ShardStateStuff>> {
        if let Some(s) = self.states.get(&self.last_mc_state.lock().unwrap()) {
            Ok(s.val().clone())
        } else {
            fail!("Can't load last applied masterchain state by id");
        }
    }

    async fn wait_state(
        self: Arc<Self>,
        id: &BlockIdExt,
        _timeout_ms: Option<u64>,
        _allow_block_downloading: bool,
    ) -> Result<Arc<ShardStateStuff>> {
        self.load_state(id).await
    }

    fn get_external_messages_iterator(
        &self,
        _shard: ShardIdent,
        _finish_time_ms: u64,
    ) -> Box<dyn Iterator<Item = (Arc<Message>, UInt256)> + Send + Sync> {
        Box::new(vec![].into_iter())
    }

    async fn get_shard_blocks(
        &self,
        _: &Arc<ShardStateStuff>,
        _: Option<&mut u32>,
    ) -> Result<Vec<Arc<TopBlockDescrStuff>>> {
        Ok(vec![])
    }

    fn complete_external_messages(
        &self,
        _to_delay: Vec<(UInt256, String)>,
        _to_delete: Vec<(UInt256, i32)>,
    ) -> Result<()> {
        Ok(())
    }

    #[cfg(feature = "telemetry")]
    fn engine_telemetry(&self) -> &Arc<EngineTelemetry> {
        &self.telemetry
    }

    fn engine_allocated(&self) -> &Arc<EngineAlloc> {
        &self.allocated
    }

    fn collator_config(&self) -> &CollatorConfig {
        &self.collator_config
    }

    fn collator_config_mc(&self) -> &CollatorConfig {
        &self.collator_config
    }

    async fn wait_applied_block(
        &self,
        id: &BlockIdExt,
        _timeout_ms: Option<u64>,
    ) -> Result<Arc<BlockHandle>> {
        self.load_block_handle(id)?.ok_or_else(|| error!("Cannot load handle for block {}", id))
    }
}

#[test]
fn test_calc_utime_allow_same_timestamp_does_not_drift_when_prev_ahead() {
    // Simplex / allow_same_timestamp=true: monotonic, but no forced +1 drift.
    assert_eq!(Collator::calc_utime(1015, 1000, true), 1015);
}

#[test]
fn test_calc_utime_strict_timestamp_forces_increment_when_prev_ahead() {
    // Catchain / allow_same_timestamp=false: C++-compatible strict +1.
    assert_eq!(Collator::calc_utime(1015, 1000, false), 1016);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_pipeline_collator() {
    async fn test() -> Result<()> {
        create_dir_all(RES_PATH).ok();
        init_test_log();
        let bundle = Arc::new(
            CollatorTestBundle::build_with_zero_state(
                "src/tests/static/zerostate.boc",
                &["src/tests/static/basestate0.boc", "src/tests/static/basestate0.boc"],
            )
            .await?,
        );
        let engine = bundle.clone() as Arc<dyn EngineOperations>;
        let result = crate::collator_test_bundle::try_collate(
            engine,
            bundle.block_id().shard().clone(),
            bundle.prev_blocks_ids().clone(),
            PipelineContext::new(),
            None,
            None,
            true,
            true,
            true,
        )
        .await?;
        let (mc_id, mc_state) = match result {
            CollateResult::Ok { candidate, new_state, .. } => (candidate.block_id, new_state),
            CollateResult::Err { err, .. } => fail!("Failed to collate mc block {err:?}"),
        };
        let extra = mc_state.read_custom()?.unwrap();

        let mut prev_blocks_ids = Vec::new();
        if let Some(InRefValue(bintree)) = extra.shards.get(&0)? {
            bintree.iterate(|prefix, shard_descr| {
                let shard_ident = ShardIdent::with_prefix_slice(0, prefix)?;
                let block_id = BlockIdExt::with_params(
                    shard_ident,
                    shard_descr.seq_no,
                    shard_descr.root_hash,
                    shard_descr.file_hash,
                );
                prev_blocks_ids.push(block_id);
                Ok(true)
            })?;
        }

        let engine = Arc::new(TestPipelineCollatorEngine::new());
        engine.add_state(bundle.load_state(&prev_blocks_ids[0]).await?);
        engine.add_state(ShardStateStuff::from_state(
            mc_id.clone(),
            mc_state,
            engine.engine_telemetry(),
            engine.engine_allocated(),
        )?);
        engine.set_last_mc(mc_id);

        // let mut prev_states = vec![bundle.load_state(&prev_blocks_ids[0]).await?];
        let mut context = PipelineContext::new();
        for _ in 0..6 {
            let CollateResult::Ok { candidate, new_state, new_block, .. } =
                crate::collator_test_bundle::try_collate(
                    engine.clone(),
                    ShardIdent::with_workchain_id(0)?,
                    prev_blocks_ids.clone(),
                    context.clone(),
                    None,
                    None,
                    false,
                    false,
                    false,
                )
                .await?
            else {
                fail!("Failed to collate block");
            };
            prev_blocks_ids = vec![candidate.block_id.clone()];
            let state = ShardStateStuff::from_state(
                candidate.block_id.clone(),
                new_state,
                engine.engine_telemetry(),
                engine.engine_allocated(),
            )?;
            context.add(state, new_block, 10);
            engine.add_block(BlockStuff::deserialize_block(
                candidate.block_id,
                Arc::new(candidate.data),
            )?);
        }
        Ok(())
    }
    test_async(
        || Box::pin(test()),
        || {
            remove_dir_all(RES_PATH).ok();
        },
    )
    .await;
}

// prepare for testing purposes
fn prepare_test_env_message(
    src_prefix: u64,
    dst_prefix: u64,
    bits: u8,
    at: u32,
    lt: u64,
    use_hypercube: bool,
) -> Result<MsgEnvelopeStuff> {
    let shard = ShardIdent::with_prefix_len(bits, 0, src_prefix)?;
    let src = UInt256::from_le_bytes(&src_prefix.to_be_bytes());
    let dst = UInt256::from_le_bytes(&dst_prefix.to_be_bytes());
    let src = MsgAddressInt::with_standart(None, 0, src.into())?;
    let dst = MsgAddressInt::with_standart(None, 0, dst.into())?;

    // let src_prefix = AccountIdPrefixFull::prefix(&src).unwrap();
    // let dst_prefix = AccountIdPrefixFull::prefix(&dst).unwrap();
    // let ia = IntermediateAddress::full_src();
    // let route_info = src_prefix.perform_hypercube_routing(&dst_prefix, &shard, ia)?.unwrap();
    // let cur_prefix  = src_prefix.interpolate_addr_intermediate(&dst_prefix, &route_info.0)?;
    // let next_prefix = src_prefix.interpolate_addr_intermediate(&dst_prefix, &route_info.1)?;

    let hdr = InternalMessageHeader::with_addresses(
        src,
        dst,
        CurrencyCollection::with_coins(1_000_000_000),
    );
    let mut msg = Message::with_int_header(hdr);
    msg.set_at_and_lt(at, lt);
    let msg_cell = msg.serialize()?;
    MsgEnvelopeStuff::new(msg, msg_cell, &shard, Coins::from(1_000_000), use_hypercube)
}

#[test]
fn test_hypercube_routing_off() {
    let pfx_len = 12;
    let src = 0xd78b3fd904191a09;
    let dst = 0xd4d300cee029b9c7;
    let hop = 0xd48b3fd904191a09;
    let env = prepare_test_env_message(src, dst, pfx_len, 0, 0, false).unwrap();
    let src_shard_id = ShardIdent::with_prefix_len(pfx_len, 0, src).unwrap();
    let dst_shard_id = ShardIdent::with_prefix_len(pfx_len, 0, dst).unwrap();
    let hop_shard_id = ShardIdent::with_prefix_len(pfx_len, 0, hop).unwrap();
    let src_prefix = env.src_prefix();
    let dst_prefix = env.dst_prefix();
    assert!(src_shard_id.contains_full_prefix(src_prefix));
    assert!(dst_shard_id.contains_full_prefix(dst_prefix));

    assert_eq!(src_prefix, &AccountIdPrefixFull::workchain(0, src));
    assert_eq!(dst_prefix, &AccountIdPrefixFull::workchain(0, dst));

    let cur_prefix = env.cur_prefix();
    let next_prefix = env.next_prefix();

    assert_eq!(src_prefix, cur_prefix);
    assert_eq!(dst_prefix, next_prefix);
    assert!(src_shard_id.contains_full_prefix(cur_prefix));
    println!("shard: {}, prefix: {:x}", hop_shard_id, next_prefix.prefix);
    assert!(!hop_shard_id.contains_full_prefix(next_prefix));
    assert!(dst_shard_id.contains_full_prefix(next_prefix));

    assert_eq!(cur_prefix, &AccountIdPrefixFull::workchain(0, src));
    assert_eq!(next_prefix, &AccountIdPrefixFull::workchain(0, dst));

    assert!(
        count_matching_bits(dst_prefix, next_prefix) >= count_matching_bits(dst_prefix, cur_prefix)
    );
}

static NODE_DB: &str = "../../node_db";
static RES_PATH: &str = "../target/cmp";

async fn try_collate_by_bundle(
    bundle: Arc<CollatorTestBundle>,
    compare_ethalon: bool,
) -> Result<BlockCandidate> {
    let block_stuff_opt = bundle.ethalon_block()?;
    let engine = bundle.clone() as Arc<dyn EngineOperations>;
    let collate_result = crate::collator_test_bundle::try_collate(
        engine,
        bundle.block_id().shard().clone(),
        bundle.prev_blocks_ids().clone(),
        PipelineContext::new(),
        Some(bundle.created_by().clone()),
        match &block_stuff_opt {
            Some(block) => Some(block.block()?.read_extra().unwrap().rand_seed().clone()),
            None => bundle.rand_seed().cloned(),
        },
        true,
        true,
        compare_ethalon,
    )
    .await?;

    let candidate = match &collate_result {
        CollateResult::Ok { candidate, .. } => candidate.clone(),
        CollateResult::Err { err, .. } => panic!("No block candidate produced {err:?}"),
    };
    if !compare_ethalon {
        return Ok(candidate);
    }

    if let Some(ethalon_block) = &block_stuff_opt {
        // let old_state = bundle.load_state(&ethalon_block.block()?.read_info()?.read_prev_ids()?[0]).await?;
        // let old_state = old_state.state()?.serialize()?;
        // let new_state = ethalon_block.block()?.read_state_update()?.apply_for(&old_state)?;
        // println!("Original old state root {:#.3}", old_state);
        // println!("Original new state root {:#.3}", new_state);
        if let Err(result) =
            compare_blocks(ethalon_block.block()?, &Block::construct_from_bytes(&candidate.data)?)
        {
            panic!("Blocks are not equal: {}", result);
        }
    }
    Ok(candidate)
}

async fn try_validate_by_bundle(
    bundle: Arc<CollatorTestBundle>,
) -> Result<Option<Arc<ShardStateStuff>>> {
    let candidate = bundle.candidate().ok_or_else(|| error!("No candidate in bundle"))?;
    let engine = bundle.clone() as Arc<dyn EngineOperations>;
    crate::collator_test_bundle::try_validate(engine, candidate.clone()).await
}

#[tokio::test(flavor = "multi_thread")]
async fn test_collate_first_block() {
    init_test_log();
    async fn test() -> Result<()> {
        create_dir_all(RES_PATH).ok();
        //init_test_log();
        match CollatorTestBundle::build_with_zero_state(
            "src/tests/static/zerostate.boc",
            &["src/tests/static/basestate0.boc", "src/tests/static/basestate0.boc"],
        )
        .await
        {
            Ok(bundle) => try_collate_by_bundle(Arc::new(bundle), true).await.map(|_| ()),
            Err(e) => Err(e),
        }
    }
    test_async(
        || Box::pin(test()),
        || {
            remove_dir_all(RES_PATH).ok();
        },
    )
    .await;
}

#[ignore]
#[tokio::test]
async fn prepare_ethalon_bundle() {
    init_test_log();
    let block_id = "(0:e000000000000000, 53211575, rh 984d826d3ac90a586849090b370705eee7b20f2b0c6a8b64d3ae912d9977f135, fh c7dd3f738afb8f8677096e5e5958d681093201a8c435943b4e1feee5b83c9797)";
    let block_id: BlockIdExt = block_id.parse().unwrap();
    let engine = Arc::new(TestEngine::new_db_dir(NODE_DB, Some(RES_PATH)).await.unwrap());
    let block = engine.load_block_by_id(&block_id).await.unwrap();
    let engine = engine.clone() as Arc<dyn EngineOperations>;
    let bundle = CollatorTestBundle::build_with_ethalon(&engine, block).await.unwrap();
    bundle.save("../target/bundles").unwrap();
}

#[ignore]
#[tokio::test(flavor = "multi_thread")]
async fn check_ethalon_bundle() {
    init_test_log();
    let path = "../target/bundles/0.e000000000000000_53211575_984d826d_collator_test_bundle";
    let bundle = CollatorTestBundle::load(path).unwrap();
    let bundle = Arc::new(bundle);
    try_collate_by_bundle(bundle.clone(), true).await.unwrap();
    try_validate_by_bundle(bundle.clone()).await.unwrap();
}

#[ignore]
#[tokio::test]
async fn test_validate_history_data() {
    init_test_log();
    let engine = TestEngine::new_db_dir(NODE_DB, None).await.unwrap();
    // engine.check_only_masterchain = true;
    let engine = Arc::new(engine);
    let mut mc_state_id = (*engine.load_last_applied_mc_block_id().unwrap().unwrap()).clone();
    log::info!("last masterchain block_id: {}", mc_state_id);
    let skip_blocks = [
        "(0:e000000000000000, 53211575, rh 984d826d3ac90a586849090b370705eee7b20f2b0c6a8b64d3ae912d9977f135, fh c7dd3f738afb8f8677096e5e5958d681093201a8c435943b4e1feee5b83c9797)",
    ];
    let skip_blocks =
        skip_blocks.iter().map(|block_id| block_id.parse().unwrap()).collect::<Vec<BlockIdExt>>();

    for _ in 0..2000 {
        // engine.check_block(&mc_state_id).await.unwrap();
        let (mc_state, shard_blocks) = engine.get_prev_mc_state(&mc_state_id).await.unwrap();
        mc_state_id = mc_state.block_id().clone();
        engine.change_mc_state(&mc_state_id).await.unwrap();
        for block_id in &shard_blocks {
            if !skip_blocks.iter().any(|id| id == block_id) {
                engine.check_block(block_id).await.unwrap();
            }
        }
    }
}

#[ignore]
#[tokio::test]
async fn test_collate_bad_block() {
    init_test_log();
    let engine = Arc::new(TestEngine::new_db_dir(NODE_DB, Some(RES_PATH)).await.unwrap());
    let ids = [
        "(0:e000000000000000, 53211575, rh 984d826d3ac90a586849090b370705eee7b20f2b0c6a8b64d3ae912d9977f135, fh c7dd3f738afb8f8677096e5e5958d681093201a8c435943b4e1feee5b83c9797)",
    ];
    for block_id in ids {
        let block_id: BlockIdExt = block_id.parse().unwrap();
        println!("{block_id}");
        // let _block_stuff = engine.load_block_by_id(&block_id).await.unwrap();
        engine.check_block(&block_id).await.unwrap();
    }
}

#[cfg(not(feature = "xp25"))]
#[tokio::test(flavor = "multi_thread")]
async fn test_collated_data1() {
    init_test_log();
    let mut bundle = CollatorTestBundle::load("src/tests/static/test_collated_data").unwrap();
    let config =
        bundle.load_last_applied_mc_state().await.unwrap().config_params().unwrap().clone();
    let capabilities = config.capabilities();
    bundle.set_capabilities(capabilities | GlobalCapabilities::CapFullCollatedData as u64);

    let candidate = try_collate_by_bundle(Arc::new(bundle), true).await.unwrap();

    let mut collated_data_roots =
        BocReader::new().read_inmem(Arc::new(candidate.collated_data)).unwrap().roots;
    assert_eq!(collated_data_roots.len(), 10);
    let state_proof = MerkleProof::construct_from_cell(collated_data_roots.pop().unwrap()).unwrap();
    let state: ShardStateUnsplit = state_proof.virtualize().unwrap();
    let account = state
        .read_accounts()
        .unwrap()
        .account(
            &SliceData::from_string(
                "44d417b8b57d494d2fc37966f1b0659319328a89ed31f3a4c0d39bd8e135e637",
            )
            .unwrap(),
        )
        .unwrap()
        .unwrap();
    account
        .read_account()
        .unwrap()
        .init_storage_stat(config.size_limits_config().unwrap().acc_state_cells_for_storage_dict)
        .unwrap();
}

fn read_dict_proof(root: Cell) -> Result<MerkleProof> {
    MerkleProof::construct_from_cell(AccountStorageDictProof::construct_from_cell(root)?.proof)
}

async fn check_bundle(bundle: &str, collated_roots: usize) {
    let mut bundle = CollatorTestBundle::load(bundle).unwrap();
    let config =
        bundle.load_last_applied_mc_state().await.unwrap().config_params().unwrap().clone();
    let capabilities = config.capabilities();
    bundle.set_capabilities(capabilities | GlobalCapabilities::CapFullCollatedData as u64);

    let candidate = try_collate_by_bundle(Arc::new(bundle), true).await.unwrap();

    let mut collated_data_roots =
        BocReader::new().read_inmem(Arc::new(candidate.collated_data)).unwrap().roots;
    assert_eq!(collated_data_roots.len(), collated_roots);
    let state_proof = MerkleProof::construct_from_cell(collated_data_roots.pop().unwrap()).unwrap();
    let state: ShardStateUnsplit = state_proof.virtualize().unwrap();
    let accounts = state.read_accounts().unwrap();

    let mut other_proofs = HashMap::new();
    let dict_proofs = collated_data_roots
        .iter()
        .filter_map(|root| {
            if let Ok(dict_proof) = read_dict_proof(root.clone()) {
                Some(dict_proof)
            } else {
                let proof = MerkleProof::construct_from_cell(root.clone()).unwrap();
                other_proofs.insert(proof.hash.clone(), proof);
                None
            }
        })
        .collect::<Vec<_>>();

    let block = Block::construct_from_bytes(&candidate.data).unwrap();
    let extra = block.read_extra().unwrap();
    let account_blocks = extra.read_account_blocks().unwrap();
    account_blocks
        .iterate_with_keys(|acc_id, _| {
            let shard_acc = accounts.account(&acc_id).unwrap().unwrap();
            let mut account = shard_acc.read_account().unwrap();
            if let Some(dict_hash) = account.dict_hash() {
                // check that we either have dict proof or account state is not pruned
                // so we are able to init dict
                if !dict_proofs.iter().any(|p| &p.proof.hash(0) == dict_hash) {
                    account
                        .init_storage_stat(
                            config.size_limits_config().unwrap().acc_state_cells_for_storage_dict,
                        )
                        .unwrap();
                }
            }
            Ok(true)
        })
        .unwrap();

    // check block state proofs and corresponding msg queue proofs
    let mut total_proofs = 0;
    for proof in other_proofs.values() {
        if let Ok(block) = proof.virtualize::<Block>() {
            total_proofs += 1;
            let state_update = block.read_state_update().unwrap();
            if state_update.new_hash == state_proof.hash {
                continue;
            }
            let state_proof = other_proofs.get(&state_update.new_hash).unwrap();
            let state: ShardStateUnsplit = state_proof.virtualize().unwrap();
            state.read_out_msg_queue_info().unwrap();
            total_proofs += 1;
        }
    }
    assert_eq!(total_proofs + dict_proofs.len() + 1, collated_roots);
}

#[cfg(not(feature = "xp25"))]
#[tokio::test(flavor = "multi_thread")]
async fn test_collated_data2() {
    init_test_log();
    check_bundle("src/tests/static/test_collated_data", 10).await;
    check_bundle("src/tests/static/0.8000000000000000_56703839_5b25cdef_collator_test_bundle", 11)
        .await;
    check_bundle("src/tests/static/0.8000000000000000_57120589_921aa09f_collator_test_bundle", 4)
        .await;

    let bundle = Arc::new(
        CollatorTestBundle::load(
            "src/tests/static/0.6000000000000000_439_d27493e2_collator_test_bundle",
        )
        .unwrap(),
    );
    try_validate_by_bundle(bundle).await.unwrap();
}

#[ignore]
#[tokio::test(flavor = "multi_thread")]
async fn test_collated_data_xp25() {
    init_test_log();
    check_bundle("src/tests/static/0.a000000000000000_610_7ff07250_collator_test_bundle", 4).await;

    let bundle = Arc::new(
        CollatorTestBundle::load(
            "src/tests/static/0.a000000000000000_610_7ff07250_collator_test_bundle",
        )
        .unwrap(),
    );
    try_validate_by_bundle(bundle).await.unwrap();
}
