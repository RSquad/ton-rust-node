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
#![allow(dead_code)]
use crate::{
    block::BlockStuff,
    block_proof::BlockProofStuff,
    collator_test_bundle::create_engine_allocated,
    config::{CollatorConfig, TonNodeConfig},
    engine::SplitQueues,
    engine_traits::{EngineAlloc, EngineOperations},
    ext_messages::MessagesPool,
    full_node::apply_block::apply_block,
    internal_db::{
        BlockResult, InternalDb, InternalDbConfig, LAST_APPLIED_MC_BLOCK, SHARD_CLIENT_MC_BLOCK,
    },
    network::node_network::NodeNetwork,
    shard_blocks::ShardBlocksPool,
    shard_state::ShardStateStuff,
    types::top_block_descr::TopBlockDescrStuff,
    validator::{
        accept_block::create_top_shard_block_description,
        collator::{CollateResult, Collator},
        state_resolver_cache::StateResolverCache,
        validate_query::ValidateQuery,
        validator_group::PipelineContext,
        validator_utils::{compute_validator_set_cc, PrevBlockHistory},
        BlockCandidate,
    },
};
#[cfg(feature = "telemetry")]
use crate::{collator_test_bundle::create_engine_telemetry, engine_traits::EngineTelemetry};
use std::{
    collections::HashSet,
    fs::copy,
    future::Future,
    path::Path,
    pin::Pin,
    sync::{
        atomic::{AtomicU32, AtomicU8, Ordering},
        Arc, Once, RwLock,
    },
    time::Duration,
};
use storage::block_handle_db::{BlockHandle, Callback, StoreJob};
use ton_block::{
    error, write_boc, Account, AccountId, AccountIdPrefixFull, AccountStorage, BlkMasterInfo,
    Block, BlockIdExt, BlockSignatures, BlockSignaturesVariant, Cell, ConfigParam0, ConfigParam34,
    ConfigParamEnum, ConfigParams, CurrencyCollection, Deserializable, HashmapAugType, HashmapType,
    InMsgDescr, InRefValue, Libraries, McStateExtra, Message, MsgAddressInt, OldMcBlocksInfo,
    OutMsgDescr, OutMsgQueue, Serializable, ShardAccount, ShardAccountBlocks, ShardIdent,
    ShardStateUnsplit, SliceData, StateInit, StorageInfo, TickTock, Transaction, UInt15, UInt256,
    ValidatorBaseInfo, ValidatorDescr, ValidatorSet,
};
use ton_block_json::*;

include!("../../../common/src/config.rs");
include!("../../../common/src/test.rs");
include!("../../../block/src/tests/test_utils.rs");

// replace assert_eq for compare not to get panic
macro_rules! assert_eq {
    ($left:expr , $right:expr,) => ({
        assert_eq!($left, $right)
    });
    ($left:expr , $right:expr) => ({
        match (&($left), &($right)) {
            (left_val, right_val) => {
                if !(*left_val == *right_val) {
                    fail!("{}, file {}:{}",
                        pretty_assertions::Comparison::new(left_val, right_val), file!(), line!())
                }
            }
        }
    });
    ($left:expr , $right:expr, $($arg:tt)*) => ({
        match (&($left), &($right)) {
            (left_val, right_val) => {
                if !(*left_val == *right_val) {
                    fail!("{}, {} file {}:{}",
                        pretty_assertions::Comparison::new(left_val, right_val),
                        format_args!($($arg)*), file!(), line!())
                }
            }
        }
    });
}

pub fn are_shard_states_equal(ss1: &ShardStateStuff, ss2: &ShardStateStuff) -> bool {
    (ss1.block_id() == ss2.block_id())
        && (ss1.state().unwrap() == ss2.state().unwrap())
        && (ss1.root_cell() == ss2.root_cell())
        && if let Ok(extra1) = ss1.shard_state_extra() {
            if let Ok(extra2) = ss2.shard_state_extra() {
                extra1 == extra2
            } else {
                false
            }
        } else {
            ss2.shard_state_extra().is_err()
        }
}

fn compare_block_accounts(acc1: &ShardAccountBlocks, acc2: &ShardAccountBlocks) -> Result<()> {
    acc1.scan_diff_with_aug(acc2, |key, acc_cur_1, acc_cur_2| {
        dbg!(&key);
        if let (Some((acc1, cur1)), Some((acc2, cur2))) = (&acc_cur_1, &acc_cur_2) {
            acc1.transactions().scan_diff_with_aug(
                acc2.transactions(),
                |lt, tr_cur1, tr_cur2| {
                    dbg!(&lt);
                    if let (Some((InRefValue(tr1), cur1)), Some((InRefValue(tr2), cur2))) =
                        (&tr_cur1, &tr_cur2)
                    {
                        compare_transactions(tr1, tr2, true)?;
                        assert_eq!(cur1, cur2);
                    } else {
                        if let Some((InRefValue(tr), _cur)) = &tr_cur1 {
                            log::info!("{}", debug_transaction(tr.clone())?);
                        }
                        assert_eq!(tr_cur1, tr_cur2);
                    }
                    Ok(true)
                },
            )?;
            // println!("{:#.3}", acc1.transactions().data().unwrap());
            // println!("{:#.3}", acc1.transactions().data().unwrap());
            assert_eq!(acc1.transactions(), acc2.transactions());
            assert_eq!(cur1, cur2);
            assert_eq!(acc1.read_state_update()?, acc2.read_state_update()?);
        } else {
            assert_eq!(acc_cur_1, acc_cur_2);
        }
        Ok(true)
    })?;
    // std::fs::write("../target/cmp/1.txt", &format!("{:#.3}", acc1.data().unwrap())).unwrap();
    // std::fs::write("../target/cmp/2.txt", &format!("{:#.3}", acc2.data().unwrap())).unwrap();
    assert_eq!(acc1, acc2);
    Ok(())
}

pub fn compare_blocks(block1: &Block, block2: &Block) -> Result<()> {
    // std::fs::write("src/tests/static/block_original.txt", debug_block_full(block1)?)?;
    // std::fs::write("src/tests/static/block_collated.txt", debug_block_full(block2)?)?;
    assert_eq!(block1.global_id(), block2.global_id(), "global_id");
    let mut info1 = block1.read_info()?;
    let info2 = block2.read_info()?;
    assert_eq!(info1.read_prev_ref()?, info2.read_prev_ref()?, "info");
    let extra1 = block1.read_extra()?;
    let extra2 = block2.read_extra()?;
    compare_in_msgs(&extra1.read_in_msg_descr()?, &extra2.read_in_msg_descr()?)?;
    compare_out_msgs(&extra1.read_out_msg_descr()?, &extra2.read_out_msg_descr()?)?;
    compare_block_accounts(&extra1.read_account_blocks()?, &extra2.read_account_blocks()?)?;

    let custom1 = extra1.read_custom()?;
    let custom2 = extra2.read_custom()?;
    if custom1.is_some() && custom2.is_some() {
        let custom1 = custom1.unwrap();
        let custom2 = custom2.unwrap();
        let msg1 = custom1.read_recover_create_msg()?;
        let msg2 = custom2.read_recover_create_msg()?;
        // assert_eq!(msg1.read_message()?, msg2.read_message()?, "recover_create_msg");
        assert_eq!(msg1, msg2, "recover_create_msg");
        let msg1 = custom1.read_mint_msg()?;
        let msg2 = custom2.read_mint_msg()?;
        // assert_eq!(msg1.read_message()?, msg2.read_message()?, "mint_msg");
        assert_eq!(msg1, msg2, "mint_msg");
        assert_eq!(custom1, custom2);
    } else {
        assert_eq!(custom1, custom2);
    }
    if let Some(mut version) = info1.gen_software().cloned() {
        version.version = info2.gen_software().unwrap().version;
        info1.set_gen_software(Some(version));
    }
    assert_eq!(info1, info2, "info");
    assert_eq!(extra1, extra2, "extra");
    let value_flow1 = block1.read_value_flow()?;
    let value_flow2 = block2.read_value_flow()?;
    assert_eq!(value_flow1, value_flow2, "value_flow");
    assert_eq!(
        block1.read_state_update()?.new_hash,
        block2.read_state_update()?.new_hash,
        "state_update new hash"
    );
    Ok(())
}

fn compare_in_msgs(msgs1: &InMsgDescr, msgs2: &InMsgDescr) -> Result<()> {
    msgs1.scan_diff_with_aug(msgs2, |_key, msg_aug_1, msg_aug_2| {
        dbg!(&_key);
        // dbg!(&msg_aug_1);
        // dbg!(&msg_aug_2);
        // let _tr = msg_aug_1.as_ref().unwrap().0.read_transaction()?.unwrap();
        // dbg!(debug_transaction(_tr)?);
        if let (Some((msg1, aug1)), Some((msg2, aug2))) = (&msg_aug_1, &msg_aug_2) {
            let check_trans;
            if let (Some(tr1), Some(tr2)) = (msg1.read_transaction()?, msg2.read_transaction()?) {
                compare_transactions(&tr1, &tr2, false)?;
                check_trans = false;
            } else {
                check_trans = true;
            }
            let (std_msg1, std_msg2) = (msg1.read_message()?, msg2.read_message()?);
            compare_messages(&std_msg1, &std_msg2, check_trans)?;
            assert_eq!(aug1, aug2);
        } else if let Some(msg_aug_2) = msg_aug_2 {
            println!("{}", debug_message(msg_aug_2.0.read_message()?.clone())?);
            assert_eq!(msg_aug_1, Some(msg_aug_2));
        } else {
            if let Some((msg1, _aug1)) = msg_aug_1.clone() {
                println!("only in msgs1 {:?}", msg1.read_message()?);
                println!("only in msgs1 {:?}", msg1.read_transaction()?);
            }
            if let Some((msg2, _aug2)) = msg_aug_2.clone() {
                println!("only in msgs2 {:?}", msg2.read_message()?);
                println!("only in msgs2 {:?}", msg2.read_transaction()?);
            }
            assert_eq!(msg_aug_1, msg_aug_2);
        }
        Ok(true)
    })?;
    assert_eq!(msgs1, msgs2);
    Ok(())
}

fn compare_messages(msg1: &Message, msg2: &Message, _check_transaction: bool) -> Result<()> {
    assert_eq!(msg1, msg2);
    Ok(())
}

fn compare_out_msgs(msgs1: &OutMsgDescr, msgs2: &OutMsgDescr) -> Result<()> {
    msgs1.scan_diff_with_aug(msgs2, |key, msg_aug_1, msg_aug_2| {
        dbg!(&key);
        dbg!(&msg_aug_1);
        dbg!(&msg_aug_2);
        assert_eq!(msg_aug_1, msg_aug_2);
        Ok(true)
    })?;
    assert_eq!(msgs1, msgs2);
    Ok(())
}

pub fn compare_transactions(
    tr1: &Transaction,
    tr2: &Transaction,
    check_messages: bool,
) -> Result<()> {
    dbg!(tr1.logical_time());
    assert_eq!(tr1.read_description()?, tr2.read_description()?);
    if check_messages {
        let (msg1, msg2) = (&tr1.in_msg, &tr2.in_msg);
        if !msg1.is_empty() && !msg2.is_empty() {
            compare_messages(&msg1.read_struct()?, &msg2.read_struct()?, false)?;
        } else {
            assert_eq!(msg1, msg2);
        }
    }
    tr1.out_msgs.scan_diff(&tr2.out_msgs, |key: UInt15, msg1, msg2| {
        dbg!(&key.0);
        if let (Some(InRefValue(msg1)), Some(InRefValue(msg2))) = (&msg1, &msg2) {
            compare_messages(msg1, msg2, false)?;
        } else {
            assert_eq!(tr1.in_msg, tr2.in_msg);
        }
        Ok(true)
    })?;
    assert_eq!(tr1.read_state_update()?, tr2.read_state_update()?);
    assert_eq!(tr1, tr2);
    Ok(())
}

pub async fn create_network(
    config: Option<TonNodeConfig>,
    config_tag: Option<&str>,
    ip: &str,
) -> Result<Arc<NodeNetwork>> {
    static INIT_CFG: Once = Once::new();
    INIT_CFG.call_once(|| {
        copy("./configs/ton-global.config-sample.json", "../target/ton-global.config-sample.json")
            .unwrap();
    });
    while !INIT_CFG.is_completed() {
        tokio::task::yield_now().await
    }
    let config = if let Some(config) = config {
        config
    } else {
        get_config(
            ip,
            Some("../target"),
            config_tag,
            "../node/configs/default_config_localhost.json",
        )
        .await?
    };
    NodeNetwork::new(
        config,
        tokio_util::sync::CancellationToken::new(),
        #[cfg(feature = "telemetry")]
        create_engine_telemetry(),
        create_engine_allocated(),
    )
    .await
}

pub fn full_trace_block(name: &str, block: &Block) -> Result<()> {
    let mut text = format!("Block: {}\n", debug_block(block.clone())?);
    let extra = block.read_extra()?;
    let in_msgs = extra.read_in_msg_descr()?;
    in_msgs.iterate_objects(|in_msg| {
        let msg = in_msg.read_message()?;
        text += &format!("InMsg: {}\n", debug_message(msg)?);
        Ok(true)
    })?;
    let out_msgs = extra.read_out_msg_descr()?;
    out_msgs.iterate_objects(|out_msg| {
        if let Some(msg) = out_msg.read_message()? {
            text += &format!("OutMsg: {}\n", debug_message(msg)?);
        }
        Ok(true)
    })?;
    let acc_blocks = extra.read_account_blocks()?;
    acc_blocks.iterate_objects(|block| {
        block.transactions().iterate_objects(|InRefValue(tr)| {
            text += &format!("Transaction: {}\n", debug_transaction(tr)?);
            Ok(true)
        })
    })?;
    std::fs::write(name, text)?;
    Ok(())
}

#[derive(Clone)]
pub struct GenMasterStateParams<'a> {
    pub config: ConfigParams,
    pub shard_state_id: Option<BlockIdExt>,
    pub master_state_id: Option<BlockIdExt>,
    pub accounts: &'a [&'a Account],
    pub libraries: Libraries,
    pub prev_blocks: Option<OldMcBlocksInfo>,
    pub after_key_block: bool,
}

impl Default for GenMasterStateParams<'_> {
    fn default() -> Self {
        Self {
            config: ConfigParams::default(),
            shard_state_id: None,
            master_state_id: None,
            accounts: &[],
            libraries: Libraries::default(),
            prev_blocks: None,
            after_key_block: false,
        }
    }
}

pub fn gen_master_state(
    params: GenMasterStateParams,
    #[cfg(feature = "telemetry")] telemetry: Option<Arc<EngineTelemetry>>,
    allocated: Option<Arc<EngineAlloc>>,
) -> Arc<ShardStateStuff> {
    let mut ss = ShardStateUnsplit::with_ident(ShardIdent::masterchain());
    for account in params.accounts {
        let account_id = account.get_id().unwrap();
        ss.insert_account(
            &account_id,
            &ShardAccount::with_params(account, UInt256::default(), 0).unwrap(),
        )
        .unwrap();
    }
    *ss.libraries_mut() = params.libraries;
    let mut ms = McStateExtra::default();
    if !params.config.config_params.is_empty() {
        ms.config = params.config;
    } else {
        let mut param = ConfigParam0::new();
        param.config_addr = AccountId::from([1; 32]);
        ms.config.set_config(ConfigParamEnum::ConfigParam0(param)).unwrap();
        let mut param = ConfigParam34::new();
        param.cur_validators =
            ValidatorSet::new(1600000000, 1610000000, 1, vec![ValidatorDescr::default()]).unwrap();
        ms.config.set_config(ConfigParamEnum::ConfigParam34(param)).unwrap();
    }

    if let Some(prev_blocks) = params.prev_blocks {
        ms.prev_blocks = prev_blocks;
    }
    ms.after_key_block = params.after_key_block;
    if let Some(shard_state_id) = params.shard_state_id {
        ms.shards
            .add_workchain(0, 0, shard_state_id.root_hash.clone(), shard_state_id.file_hash.clone())
            .unwrap();
    }
    if let Some(master_state_id) = &params.master_state_id {
        ss.set_seq_no(master_state_id.seq_no());
        ss.set_shard(master_state_id.shard().clone());
    }
    ss.write_custom(Some(&ms)).unwrap();
    let cell = ss.serialize().unwrap();
    let bytes = write_boc(&cell).unwrap();
    #[cfg(feature = "telemetry")]
    let telemetry = telemetry.unwrap_or_else(create_engine_telemetry);
    let allocated = allocated.unwrap_or_else(create_engine_allocated);
    let master_state_id = params.master_state_id.unwrap_or_else(|| {
        BlockIdExt::with_params(
            ShardIdent::masterchain(),
            0,
            cell.repr_hash().clone(),
            UInt256::calc_file_hash(&bytes),
        )
    });
    if master_state_id.seq_no() == 0 {
        ShardStateStuff::deserialize_zerostate(
            master_state_id,
            &bytes,
            #[cfg(feature = "telemetry")]
            &telemetry,
            &allocated,
        )
        .unwrap()
    } else {
        ShardStateStuff::deserialize_state(
            master_state_id,
            &bytes,
            #[cfg(feature = "telemetry")]
            &telemetry,
            &allocated,
        )
        .unwrap()
    }
}

pub fn gen_shard_state(
    shard_state_id: Option<BlockIdExt>,
    accounts: &[&Account],
    #[cfg(feature = "telemetry")] telemetry: Option<Arc<EngineTelemetry>>,
    allocated: Option<Arc<EngineAlloc>>,
    master_ref: Option<BlkMasterInfo>,
) -> (BlockIdExt, Arc<ShardStateStuff>) {
    let mut ss = ShardStateUnsplit::with_ident(ShardIdent::full(0));
    if let Some(shard_state_id) = &shard_state_id {
        ss.set_shard(shard_state_id.shard().clone());
        ss.set_seq_no(shard_state_id.seq_no());
    }
    ss.set_master_ref(master_ref);
    for account in accounts {
        let account_id = account.get_id().unwrap();
        ss.insert_account(
            &account_id,
            &ShardAccount::with_params(account, UInt256::default(), 0).unwrap(),
        )
        .unwrap();
    }
    let cell = ss.serialize().unwrap();
    let bytes = write_boc(&cell).unwrap();
    #[cfg(feature = "telemetry")]
    let telemetry = telemetry.unwrap_or_else(create_engine_telemetry);
    let allocated = allocated.unwrap_or_else(create_engine_allocated);
    if let Some(shard_state_id) = shard_state_id {
        let shard_state = ShardStateStuff::deserialize_state(
            shard_state_id.clone(),
            &bytes,
            #[cfg(feature = "telemetry")]
            &telemetry,
            &allocated,
        )
        .unwrap();
        (shard_state_id, shard_state)
    } else {
        let shard_state_id = BlockIdExt::with_params(
            ShardIdent::full(0),
            0,
            cell.repr_hash().clone(),
            UInt256::calc_file_hash(&bytes),
        );
        let shard_state = ShardStateStuff::deserialize_zerostate(
            shard_state_id.clone(),
            &bytes,
            #[cfg(feature = "telemetry")]
            &telemetry,
            &allocated,
        )
        .unwrap();
        (shard_state_id, shard_state)
    }
}

pub fn gen_test_account() -> Account {
    generate_test_account(true, AccountTestOptions::with_default_setup(true))
}

pub async fn get_config(
    ip: &str,
    config_dir: Option<&str>,
    config_tag: Option<&str>,
    default: &str,
) -> Result<TonNodeConfig> {
    let resolved_ip = resolve_ip(ip).await?;
    let config_tag = config_tag.unwrap_or("node");
    let config_path = get_test_config_path(config_tag, &resolved_ip)?;
    let config_dir = if let Some(config_dir) = config_dir {
        Path::new(config_dir)
    } else if let Some(config_dir) = config_path.parent() {
        config_dir
    } else {
        fail!("No parent in config path {}", config_path.display())
    };
    let Some(config_dir) = config_dir.to_str() else {
        fail!("Cannot use config dir {}", config_dir.display())
    };
    let Some(config_file) = config_path.file_name() else {
        fail!("No file name in config path {}", config_path.display())
    };
    let Some(config_file) = config_file.to_str() else {
        fail!("Cannot use config file {:?}", config_file)
    };
    let (adnl_config, _) = generate_adnl_configs(
        ip,
        vec![NodeNetwork::TAG_DHT_KEY, NodeNetwork::TAG_OVERLAY_KEY],
        Some(resolved_ip),
    )?;
    TonNodeConfig::from_file(config_dir, config_file, Some(adnl_config), default, None)
}

pub async fn test_async(test: impl Fn() -> Pinned<'static, ()>, done: impl Fn()) {
    let ret = test().await;
    done();
    ret.unwrap();
}

// Alias for pinned result
type Pinned<'a, X> = Pin<Box<dyn Future<Output = Result<X>> + 'a>>;

pub struct TestEngine {
    pub res_path: Option<String>,
    pub db: Arc<InternalDb>,
    pub now: AtomicU32,
    pub ext_messages: Arc<MessagesPool>,
    pub shard_states: lockfree::map::Map<ShardIdent, ShardStateStuff>,
    pub check_only_transactions: bool,
    pub check_only_masterchain: bool,
    pub check_only_msg_merger: bool,
    pub shard_blocks: ShardBlocksPool,
    last_applied_mc_block_id: RwLock<Option<Arc<BlockIdExt>>>,
    #[cfg(feature = "telemetry")]
    engine_telemetry: Arc<EngineTelemetry>,
    engine_allocated: Arc<EngineAlloc>,
}

impl TestEngine {
    pub async fn new_db_dir(db_dir: &str, res_path: Option<&str>) -> Result<Self> {
        if let Err(err) = std::fs::read_dir(db_dir) {
            fail!("Directory not found: {} {}", db_dir, err);
        }
        if let Some(res_path) = &res_path {
            std::fs::create_dir_all(res_path).ok();
        }
        let db_config = InternalDbConfig { db_directory: db_dir.to_string(), ..Default::default() };
        #[cfg(feature = "telemetry")]
        let telemetry = create_engine_telemetry();
        let allocated = create_engine_allocated();
        let db = Arc::new(
            InternalDb::with_update(
                db_config,
                false,
                false,
                false,
                None,
                &|| Ok(()),
                None,
                Arc::new(AtomicU8::new(0)),
                None,
                #[cfg(feature = "telemetry")]
                telemetry.clone(),
                allocated.clone(),
            )
            .await?,
        );
        let shard_blocks = db.load_all_top_shard_blocks().unwrap_or_default();
        let last_mc_seqno =
            db.load_full_node_state(LAST_APPLIED_MC_BLOCK)?.map(|id| id.seq_no).unwrap_or_default();
        let (shard_blocks, _) = ShardBlocksPool::new(
            shard_blocks,
            last_mc_seqno,
            true,
            #[cfg(feature = "telemetry")]
            &telemetry,
            &allocated,
        )?;
        Ok(Self {
            db,
            res_path: res_path.map(|s| s.to_string()),
            now: AtomicU32::new(0),
            ext_messages: Arc::new(MessagesPool::new(0, None).0),
            shard_states: Default::default(),
            check_only_transactions: false,
            check_only_masterchain: false,
            check_only_msg_merger: false,
            shard_blocks,
            last_applied_mc_block_id: RwLock::new(None),
            #[cfg(feature = "telemetry")]
            engine_telemetry: telemetry,
            engine_allocated: allocated,
        })
    }

    pub async fn change_mc_state(&self, mc_state_id: &BlockIdExt) -> Result<()> {
        log::debug!("Changing last masterchain state to {}", mc_state_id);
        if self.last_applied_mc_block_id.read().unwrap().as_deref() != Some(mc_state_id) {
            let mc_state = self.db.load_shard_state_dynamic(mc_state_id)?;
            self.now.store(mc_state.state()?.gen_time(), Ordering::Relaxed);
            self.save_last_applied_mc_block_id(mc_state_id)?;
            self.shard_blocks.update_shard_blocks(&mc_state).await?;
        }
        Ok(())
    }

    fn prev_mc_block_id(state: &ShardStateStuff, mc_seq_no: u32) -> Result<BlockIdExt> {
        if let Some(master) = state.shard_state_extra()?.prev_blocks.get(&mc_seq_no)? {
            let (_end_lt, id, _is_key) = master.master_block_id();
            Ok(id)
        } else {
            fail!("seq_no {mc_seq_no} not found in master {}", state.block_id().seq_no)
        }
    }

    pub async fn change_mc_state_by_seqno(&self, mc_seq_no: u32) -> Result<BlockIdExt> {
        let mc_state_id = self.db.load_full_node_state(LAST_APPLIED_MC_BLOCK)?.unwrap();
        if mc_seq_no < mc_state_id.seq_no {
            let mc_state = self.load_state(&mc_state_id).await?;
            let mc_state_id = Self::prev_mc_block_id(&mc_state, mc_seq_no)?;
            self.change_mc_state(&mc_state_id).await?;
            Ok(mc_state_id)
        } else {
            self.change_mc_state(&mc_state_id).await?;
            Ok((*mc_state_id).clone())
        }
    }

    pub async fn load_block_by_id(&self, id: &BlockIdExt) -> Result<BlockStuff> {
        let handle = self
            .load_block_handle(id)?
            .ok_or_else(|| error!("Cannot load handle for block {}", id))?;
        self.load_block(&handle).await
    }

    pub async fn prepare_for_block(&self, block_stuff: &BlockStuff) -> Result<()> {
        let info = block_stuff.block()?.read_info()?;
        let master_ref_seq_no = if let Some(master_ref) = info.read_master_ref()? {
            master_ref.master.seq_no
        } else {
            assert!(block_stuff.id().shard().is_masterchain());
            block_stuff.id().seq_no() - 1
        };
        if let Err(err) = self.change_mc_state_by_seqno(master_ref_seq_no).await {
            log::error!("DB do not have MC state for block: {} {}", block_stuff.id(), err);
            return Ok(());
        }
        let now = block_stuff.gen_utime()?;
        self.now.store(now, Ordering::Relaxed);
        self.ext_messages.set_now(now);
        let extra = block_stuff.block()?.read_extra()?;
        let in_msgs = extra.read_in_msg_descr()?;
        // self.ext_messages.clear();
        in_msgs.iterate_with_keys(|key, in_msg| {
            let msg = in_msg.read_message()?;
            if msg.is_inbound_external() {
                self.ext_messages.new_message(&key, Arc::new(msg), self.now())?;
            }
            Ok(true)
        })?;
        Ok(())
    }

    pub fn has_external_messages(&self) -> bool {
        self.ext_messages.has_messages()
    }

    pub async fn get_prev_mc_state(
        self: &Arc<Self>,
        block_id: &BlockIdExt,
    ) -> Result<(Arc<ShardStateStuff>, Vec<BlockIdExt>)> {
        if block_id.shard().is_masterchain() {
            let state = self.load_state(block_id).await?;
            let shard_blocks = state.top_blocks_all()?;
            let mc_seq_no = block_id.seq_no - 1;
            let mc_state_id = Self::prev_mc_block_id(&state, mc_seq_no)?;
            let mc_state = self.load_state(&mc_state_id).await?;
            Ok((mc_state, shard_blocks))
        } else {
            let block_stuff = self.load_block_by_id(block_id).await?;
            let block = block_stuff.block()?;
            let info = block.read_info()?;
            let (_, mc_state_id) = info.read_master_id()?.master_block_id();
            let mc_state = self.load_state(&mc_state_id).await?;
            Ok((mc_state, Vec::new()))
        }
    }

    pub async fn check_block(self: &Arc<Self>, block_id: &BlockIdExt) -> Result<()> {
        let block_stuff = self.load_block_by_id(block_id).await?;
        self.prepare_for_block(&block_stuff).await?;
        let info = block_stuff.block()?.read_info()?;
        if info.gen_software().unwrap().version < 10 {
            log::warn!("too old block version {}", info.gen_software().unwrap().version);
            return Ok(());
        }
        let prev_blocks_ids = info.read_prev_ids()?;
        let (mc_state, shard_blocks) = self.get_prev_mc_state(block_id).await?;

        if !block_id.shard().is_masterchain() && self.check_only_masterchain {
            return Ok(());
        }
        self.change_mc_state(mc_state.block_id()).await?;

        let mc_state_extra = mc_state.shard_state_extra()?;
        let cc_seqno_from_state = if block_id.shard().is_masterchain() {
            mc_state_extra.validator_info.catchain_seqno
        } else {
            mc_state_extra.shards.calc_shard_cc_seqno(block_id.shard())?
        };
        // let (validator_set, _) = mc_state.read_cur_validator_set_and_cc_conf()?;
        let mut cc_seqno_with_delta = 0;
        let nodes = compute_validator_set_cc(
            &mc_state,
            block_id.shard(),
            block_id.seq_no(),
            cc_seqno_from_state,
            &mut cc_seqno_with_delta,
        )?;
        let validator_set = ValidatorSet::with_cc_seqno(0, 0, 0, cc_seqno_with_delta, nodes)?;
        let engine = self.clone() as Arc<dyn EngineOperations>;

        // build TSBD for each shard block
        // let mut descr = TopBlockDescrSet::default();
        for block_id in &shard_blocks {
            let block_stuff = match self.load_block_by_id(block_id).await {
                Ok(block_stuff) => block_stuff,
                Err(err) => {
                    log::error!("{}", err);
                    continue;
                }
            };
            let info = block_stuff.block()?.read_info()?;
            let prev_blocks_ids = info.read_prev_ids()?; // TODO: this should be chain of ids not prev ids
            let base_info = ValidatorBaseInfo::with_params(
                info.gen_validator_list_hash_short(),
                info.gen_catchain_seqno(),
            );
            // sometimes some shards don't have states in not full database
            let signatures = BlockSignatures::with_params(base_info, Default::default());
            let tbd_opt = create_top_shard_block_description(
                &block_stuff,
                BlockSignaturesVariant::Ordinary(signatures),
                &mc_state,
                prev_blocks_ids.clone(),
                &*engine,
            )
            .await?;
            if let Some(tbd) = tbd_opt {
                assert_ne!(0, tbd.chain().len());
                let tbds = Arc::new(TopBlockDescrStuff::new(tbd, block_id, true, true)?);
                self.shard_blocks
                    .process_shard_block(
                        block_id,
                        info.gen_catchain_seqno(),
                        || Ok(tbds.clone()),
                        false,
                        &*engine,
                    )
                    .await?;
            }
        }
        self.try_collate_with_compare(validator_set, block_stuff, prev_blocks_ids).await
    }

    async fn try_validate_with_compare(
        self: &Arc<Self>,
        validator_set: ValidatorSet,
        block_stuff: BlockStuff,
        prev_blocks_ids: Vec<BlockIdExt>,
    ) -> Result<()> {
        let info = block_stuff.block()?.read_info()?;
        let extra = block_stuff.block()?.read_extra()?;
        let (_, block_id) = info.read_master_id()?.master_block_id();
        self.save_last_applied_mc_block_id(&block_id)?;

        let min_mc_seqno = info.min_ref_mc_seqno() - 1;

        let block_candidate = BlockCandidate {
            block_id: block_stuff.id().clone(),
            data: block_stuff.data().to_vec(),
            created_by: extra.created_by,
            ..Default::default()
        };

        log::info!(
            "TRY VALIDATE existing block {}, min_mc_seqno {}",
            block_stuff.id(),
            min_mc_seqno
        );

        let validator_query = ValidateQuery::new(
            block_stuff.id().shard().clone(),
            min_mc_seqno,
            prev_blocks_ids.clone(),
            Default::default(),
            None,
            block_candidate,
            validator_set.clone(),
            self.clone(),
            true,
            false,
            false,
        );
        validator_query.try_validate().await?;
        Ok(())
    }

    async fn try_collate_with_compare(
        self: &Arc<Self>,
        validator_set: ValidatorSet,
        block_stuff: BlockStuff,
        prev_blocks_ids: Vec<BlockIdExt>,
    ) -> Result<()> {
        let info = block_stuff.block()?.read_info()?;
        let extra = block_stuff.block()?.read_extra()?;
        let (_, block_id) = info.read_master_id()?.master_block_id();
        self.save_last_applied_mc_block_id(&block_id)?;

        let min_mc_seqno = info.min_ref_mc_seqno() - 1;

        let block_candidate = BlockCandidate {
            block_id: block_stuff.id().clone(),
            data: block_stuff.data().to_vec(),
            created_by: extra.created_by().clone(),
            ..Default::default()
        };

        log::info!(
            "TRY VALIDATE existing block {}, min_mc_seqno {}",
            block_stuff.id(),
            min_mc_seqno
        );

        let validator_query = ValidateQuery::new(
            block_stuff.id().shard().clone(),
            min_mc_seqno,
            prev_blocks_ids.clone(),
            Default::default(),
            None,
            block_candidate,
            validator_set.clone(),
            self.clone(),
            true,
            false,
            false,
        );
        validator_query.try_validate().await?;

        log::info!("TRY COLLATE block {}, min_mc_seqno {}", block_stuff.id(), min_mc_seqno);

        let prev = PrevBlockHistory::with_prevs(block_stuff.id().shard(), prev_blocks_ids.clone());
        let collator = Collator::new(
            block_stuff.id().shard().clone(),
            min_mc_seqno,
            &prev,
            PipelineContext::new(),
            Arc::new(tokio::sync::Mutex::new(StateResolverCache::new())),
            validator_set.clone(),
            extra.created_by().clone(),
            self.clone(),
            Some(extra.rand_seed().clone()),
            Default::default(),
        )?;

        let (block_candidate, new_state) = match collator.collate().await? {
            CollateResult::Ok { candidate, new_state, .. } => (candidate, new_state),
            CollateResult::Err { err, .. } => return Err(err),
        };

        if let Some(res_path) = &self.res_path {
            let new_block = Block::construct_from_bytes(&block_candidate.data)?;
            let su1 = block_stuff.block()?.read_state_update()?;
            let su2 = new_block.read_state_update()?;

            std::fs::write(
                format!("{}/update.txt", res_path),
                format!("old: {:#.1024}\nnew: {:#.1024}", su1.old, su1.new),
            )?;
            std::fs::write(
                format!("{}/update_candidate.txt", res_path),
                format!("old: {:#.1024}\nnew: {:#.1024}", su2.old, su2.new),
            )?;

            // let shard = block_stuff.id().shard().shard_key(false);
            // let shard = format!("{:x}-{}", shard, block_stuff.id().seq_no());

            block_stuff.block()?.write_to_file(format!("{}/block_real.boc", res_path))?;
            new_block.write_to_file(format!("{}/block_candidate.boc", res_path))?;
            std::fs::write(
                format!("{}/collated_data.bin", res_path),
                &block_candidate.collated_data,
            )?;

            full_trace_block(&format!("{}/block_real.txt", res_path), block_stuff.block()?)?;
            full_trace_block(&format!("{}/block_candidate.txt", res_path), &new_block)?;
            // full_trace_block(
            //     &format!("{}/{}-block_real.txt", res_path, shard),
            //     block_stuff.block()?
            // )?;
            // full_trace_block(
            //     &format!("{}/{}-block_candidate.txt", res_path, shard),
            //     &new_block
            // )?;

            let state_stuff = self.load_state(block_stuff.id()).await?;
            std::fs::write(
                format!("{}/state_real.txt", res_path),
                debug_state(state_stuff.state()?.clone())?,
            )?;
            std::fs::write(
                format!("{}/state_candidate.txt", res_path),
                debug_state(new_state.clone())?,
            )?;

            // std::fs::write(
            //     &format!("{}/{}-state_real.txt", res_path, shard),
            //     debug_state(state_stuff.state()?.clone())?
            //)?;
            // std::fs::write(
            //     &format!("{}/{}-state_candidate.txt", res_path, shard),
            //     debug_state(new_state.clone())?
            //)?;

            // let cell = ton_block::deserialize_tree_of_cells(
            //     &mut std::io::Cursor::new(&block_candidate.data)
            // )?;
            // std::fs::write(
            //     &format!("{}/boc_real.txt", res_path),
            //     format!("{:#.1024}", block_stuff.root_cell())
            // )?;
            // std::fs::write(
            //     &format!("{}/boc_candidate.txt", res_path),
            //     format!("{:#.1024}", &cell)
            // )?;

            // let cell = new_state.serialize()?;
            // std::fs::write(
            //     &format!("{}/toc_real.txt", res_path),
            //     format!("{:#.1024}", state_stuff.root_cell())
            // )?;
            // std::fs::write(
            //     &format!("{}/toc_candidate.txt", res_path),
            //     format!("{:#.1024}", &cell)
            // )?;
        }

        let validator_query = ValidateQuery::new(
            block_stuff.id().shard().clone(),
            min_mc_seqno,
            prev_blocks_ids,
            Default::default(),
            None,
            block_candidate.clone(),
            validator_set,
            self.clone(),
            true,
            false,
            false,
        );
        validator_query.try_validate().await?;

        // let mut error = String::new();
        // if let Err(err) = compare_states(state_stuff.state()?, &new_state) {
        //     writeln!(error, "{}", err)?;
        // }
        // if let Err(err) = compare_blocks(block_stuff.block()?, &mut new_block) {
        //     writeln!(error, "{}", err)?;
        // }
        // if !error.is_empty() {
        //     block_stuff.block()?.write_to_file(&format!("{}/block_real.boc", self.res_path))?;
        //     new_block.write_to_file(&format!("{}/block_candidate.boc", self.res_path))?;
        //     panic!("{}", error)
        // }

        Ok(())
    }
}

#[async_trait::async_trait]
impl EngineOperations for TestEngine {
    fn now(&self) -> u32 {
        self.now.load(Ordering::Relaxed)
    }

    fn now_ms(&self) -> u64 {
        self.now() as u64 * 1000
    }

    async fn lookup_block_by_seqno(
        &self,
        prefix: &AccountIdPrefixFull,
        seqno: u32,
    ) -> Result<Option<(BlockIdExt, Vec<u8>)>> {
        if !prefix.is_masterchain() {
            fail!("Only masterchain lookup is supported");
        }
        let mc_state = self.load_last_applied_mc_state().await?;
        let mc_state_id = if mc_state.block_id().seq_no() == seqno {
            mc_state.block_id().clone()
        } else {
            Self::prev_mc_block_id(&mc_state, seqno)?
        };
        Ok(Some((mc_state_id, vec![])))
    }

    async fn lookup_block_by_lt(
        &self,
        prefix: &AccountIdPrefixFull,
        lt: u64,
    ) -> Result<Option<(BlockIdExt, Vec<u8>)>> {
        self.db.lookup_block_by_lt(prefix, lt).await
    }

    fn load_block_handle(&self, id: &BlockIdExt) -> Result<Option<Arc<BlockHandle>>> {
        self.db.load_block_handle(id)
    }

    async fn load_state(&self, block_id: &BlockIdExt) -> Result<Arc<ShardStateStuff>> {
        self.db.load_shard_state_dynamic(block_id)
    }

    async fn load_block(&self, handle: &BlockHandle) -> Result<BlockStuff> {
        self.db.load_block_data(handle).await
    }

    async fn load_block_proof(
        &self,
        handle: &Arc<BlockHandle>,
        is_link: bool,
    ) -> Result<BlockProofStuff> {
        self.db.load_block_proof(handle, is_link).await
    }

    fn load_last_applied_mc_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
        match self.last_applied_mc_block_id.read().unwrap().to_owned() {
            Some(id) => Ok(Some(id)),
            None => self.db.load_full_node_state(LAST_APPLIED_MC_BLOCK),
        }
    }
    fn save_last_applied_mc_block_id(&self, last_mc_block: &BlockIdExt) -> Result<()> {
        *self.last_applied_mc_block_id.write().unwrap() = Some(Arc::new(last_mc_block.clone()));
        Ok(())
    }
    async fn load_last_applied_mc_state(&self) -> Result<Arc<ShardStateStuff>> {
        let id = if let Some(id) = self.load_last_applied_mc_block_id()? {
            id
        } else {
            fail!("No last applied MC block set")
        };
        self.load_state(&id).await
    }
    fn load_shard_client_mc_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
        self.db.load_full_node_state(SHARD_CLIENT_MC_BLOCK)
    }

    async fn send_block_broadcast(
        &self,
        _block: &BlockStuff,
        _proof: &BlockProofStuff,
        _signatures: &BlockSignaturesVariant,
    ) -> Result<()> {
        Ok(())
    }

    async fn send_top_shard_block_description(
        &self,
        _tbd: Arc<TopBlockDescrStuff>,
        _cc_seqno: u32,
        _resend: bool,
    ) -> Result<()> {
        Ok(())
    }

    async fn store_block_proof(
        &self,
        id: &BlockIdExt,
        handle: Option<Arc<BlockHandle>>,
        proof: &BlockProofStuff,
    ) -> Result<BlockResult> {
        self.db.store_block_proof(id, handle, proof, None).await
    }

    async fn store_block(&self, block: &BlockStuff) -> Result<BlockResult> {
        let mut tr_count = 0;
        block.block()?.read_extra()?.read_account_blocks()?.iterate_objects(|account| {
            tr_count += account.transactions().len()?;
            Ok(true)
        })?;
        log::trace!(
            target: "nodese",
            "block: {}:{}, transactions: {}",
            block.id().shard(), block.id().seq_no(), tr_count
        );
        self.db.store_block_data(block, None).await
    }

    fn store_block_prev1(&self, handle: &Arc<BlockHandle>, prev: &BlockIdExt) -> Result<()> {
        self.db.store_block_prev1(handle, prev, None)
    }

    fn store_block_prev2(&self, handle: &Arc<BlockHandle>, prev2: &BlockIdExt) -> Result<()> {
        self.db.store_block_prev2(handle, prev2, None)
    }

    async fn store_zerostate(
        &self,
        mut state: Arc<ShardStateStuff>,
        state_bytes: &[u8],
    ) -> Result<(Arc<ShardStateStuff>, Arc<BlockHandle>)> {
        let handle = self
            .db
            .create_or_load_block_handle(
                state.block_id(),
                None,
                Some(state.state()?.gen_time()),
                None,
            )?
            .to_non_updated()
            .ok_or_else(|| error!("Bad result in create or load block handle"))?;
        state = self.store_state(&handle, state).await?;
        self.db.store_shard_state_persistent_raw(&handle, state_bytes, None).await?;
        Ok((state, handle))
    }

    async fn set_applied(&self, handle: &Arc<BlockHandle>, mc_seq_no: u32) -> Result<bool> {
        if handle.is_applied() {
            return Ok(false);
        }
        log::trace!(target: "nodese", "set_applied: {}:{}", handle.id().shard(), handle.id().seq_no());
        self.db.assign_mc_ref_seq_no(handle, mc_seq_no, None)?;
        self.db.archive_block(handle.id(), None).await?;
        self.db.store_block_applied(handle, None)
    }

    fn load_block_prev1(&self, id: &BlockIdExt) -> Result<BlockIdExt> {
        self.db.load_block_prev1(id)
    }

    fn load_block_prev2(&self, id: &BlockIdExt) -> Result<Option<BlockIdExt>> {
        self.db.load_block_prev2(id)
    }

    fn store_block_next1(&self, handle: &Arc<BlockHandle>, next: &BlockIdExt) -> Result<()> {
        self.db.store_block_next1(handle, next, None)
    }

    fn load_block_next1(&self, id: &BlockIdExt) -> Result<BlockIdExt> {
        self.db.load_block_next1(id)
    }

    fn store_block_next2(&self, handle: &Arc<BlockHandle>, next2: &BlockIdExt) -> Result<()> {
        self.db.store_block_next2(handle, next2, None)
    }

    fn load_block_next2(&self, id: &BlockIdExt) -> Result<Option<BlockIdExt>> {
        self.db.load_block_next2(id)
    }

    async fn wait_state(
        self: Arc<Self>,
        id: &BlockIdExt,
        _timeout_ms: Option<u64>,
        _allow_block_downloading: bool,
    ) -> Result<Arc<ShardStateStuff>> {
        self.load_state(id).await
    }

    async fn store_state(
        &self,
        handle: &Arc<BlockHandle>,
        state: Arc<ShardStateStuff>,
    ) -> Result<Arc<ShardStateStuff>> {
        let (state, _) =
            self.db.store_shard_state_dynamic(handle, &state, None, None, false).await?;
        Ok(state)
    }

    async fn apply_block_internal(
        self: Arc<Self>,
        handle: &Arc<BlockHandle>,
        block: &BlockStuff,
        mc_seq_no: u32,
        pre_apply: bool,
        recursion_depth: u32,
    ) -> Result<()> {
        debug_assert!(!pre_apply);
        log::debug!(target: "nodese", "apply_block {}", handle.id());
        apply_block(
            handle,
            block,
            mc_seq_no,
            &(self.clone() as Arc<dyn EngineOperations>),
            pre_apply,
            recursion_depth,
        )
        .await?;
        self.set_applied(handle, mc_seq_no).await?;
        Ok(())
    }

    async fn download_and_apply_block(
        self: Arc<Self>,
        _id: &BlockIdExt,
        _mc_seq_no: u32,
        _pre_apply: bool,
    ) -> Result<()> {
        Ok(())
    }

    async fn wait_applied_block(
        &self,
        id: &BlockIdExt,
        _timeout_ms: Option<u64>,
    ) -> Result<Arc<BlockHandle>> {
        self.load_block_handle(id)?.ok_or_else(|| error!("Cannot load handle for block {}", id))
    }

    async fn download_block(
        &self,
        id: &BlockIdExt,
        _limit: Option<u32>,
    ) -> Result<(BlockStuff, BlockProofStuff)> {
        let handle = self
            .load_block_handle(id)?
            .ok_or_else(|| error!("Cannot load handle for block {}", id))?;
        Ok((
            self.db.load_block_data(&handle).await?,
            self.db.load_block_proof(&handle, !id.shard().is_masterchain()).await?,
        ))
    }
    fn new_external_message(&self, id: &UInt256, message: Arc<Message>) -> Result<()> {
        self.ext_messages.new_message(id, message, self.now())
    }
    fn get_external_messages_iterator(
        &self,
        shard: ShardIdent,
        finish_time_ms: u64,
    ) -> Box<dyn Iterator<Item = (Arc<Message>, UInt256)> + Send + Sync> {
        Box::new(self.ext_messages.clone().iter(shard, self.now(), finish_time_ms))
    }
    fn complete_external_messages(
        &self,
        to_delay: &[UInt256],
        to_delete: &[UInt256],
    ) -> Result<()> {
        self.ext_messages.complete_messages(to_delay, to_delete, self.now())
    }
    async fn get_shard_blocks(
        &self,
        last_mc_state: &Arc<ShardStateStuff>,
        actual_last_mc_seqno: Option<&mut u32>,
    ) -> Result<Vec<Arc<TopBlockDescrStuff>>> {
        self.shard_blocks.get_shard_blocks(last_mc_state, self, true, actual_last_mc_seqno).await
    }

    #[cfg(feature = "telemetry")]
    fn engine_telemetry(&self) -> &Arc<EngineTelemetry> {
        &self.engine_telemetry
    }

    fn engine_allocated(&self) -> &Arc<EngineAlloc> {
        &self.engine_allocated
    }

    fn collator_config(&self) -> &CollatorConfig {
        &COLLATOR_CONFIG
    }
    fn collator_config_mc(&self) -> &CollatorConfig {
        &COLLATOR_CONFIG
    }
    fn set_split_queues(
        &self,
        _before_split_block: &BlockIdExt,
        _queue0: OutMsgQueue,
        _queue1: OutMsgQueue,
        _visited_cells: HashSet<UInt256>,
    ) {
    }

    fn get_split_queues(&self, _before_split_block: &BlockIdExt) -> SplitQueues {
        None
    }
}

lazy_static::lazy_static! {
    static ref COLLATOR_CONFIG: CollatorConfig = CollatorConfig {
        cutoff_timeout_ms: 100_000,
        stop_timeout_ms: 100_000,
        max_collate_threads: 6,
        ..Default::default()
    };
}

pub struct WaitForHandle {
    count: AtomicU32,
    delay: u32,
    max_count: u32,
    ping: tokio::sync::Barrier,
}

impl WaitForHandle {
    pub fn with_max_count(max_count: u32) -> Arc<Self> {
        Self::with_max_count_and_delay(max_count, 0)
    }
    pub fn with_max_count_and_delay(max_count: u32, delay: u32) -> Arc<Self> {
        let ret =
            Self { count: AtomicU32::new(0), delay, max_count, ping: tokio::sync::Barrier::new(2) };
        Arc::new(ret)
    }
    pub async fn wait(&self) {
        self.ping.wait().await;
        if self.delay > 0 {
            tokio::time::sleep(Duration::from_millis(self.delay as u64)).await
        }
    }
}

#[async_trait::async_trait]
impl Callback for WaitForHandle {
    async fn invoke(&self, _job: StoreJob, ok: bool) {
        if ok {
            let count = self.count.fetch_add(1, Ordering::Relaxed);
            if count + 1 == self.max_count {
                self.ping.wait().await;
            }
        }
    }
}
