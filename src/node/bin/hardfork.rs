/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use clap::{Arg, Command};
use node::{
    block::BlockStuff,
    block_proof::BlockProofStuff,
    collator_test_bundle::create_engine_allocated,
    config::CollatorConfig,
    engine_traits::{EngineAlloc, EngineOperations},
    ext_messages::MessagesPool,
    internal_db::{InternalDb, InternalDbConfig, LAST_APPLIED_MC_BLOCK, SHARD_CLIENT_MC_BLOCK},
    shard_state::ShardStateStuff,
    types::top_block_descr::TopBlockDescrStuff,
    validator::{
        collator::{CollateResult, Collator},
        validator_group::PipelineContext,
        validator_utils::{calc_subset_for_masterchain, PrevBlockHistory},
        CollatorSettings,
    },
};
#[cfg(feature = "telemetry")]
use node::{
    collator_test_bundle::create_engine_telemetry, engine_traits::EngineTelemetry,
    validator::telemetry::CollatorValidatorTelemetry,
};
use std::{
    str::FromStr,
    sync::{
        atomic::{AtomicU32, AtomicU8, Ordering},
        Arc,
    },
    time::SystemTime,
};
use storage::{block_handle_db::BlockHandle, db::rocksdb::AccessType};
use ton_block::{
    base64_encode, error, fail, AccountIdPrefixFull, BlockIdExt, ConfigParams, Message, Result,
    ShardIdent, UInt256,
};

// include!("../../common/src/log.rs");

/**
 * Mock Engine for crafting block
 * Opens database for readonly and produces masterchain block by seq no
 * It can also add external messages
 */
pub struct MockEngine {
    pub db: Arc<InternalDb>,
    pub now: Arc<AtomicU32>,
    pub ext_messages: Arc<MessagesPool>,
    #[cfg(feature = "telemetry")]
    engine_telemetry: Arc<EngineTelemetry>,
    #[cfg(feature = "telemetry")]
    collator_telemetry: CollatorValidatorTelemetry,
    engine_allocated: Arc<EngineAlloc>,
    collator_config: CollatorConfig,
    new_config: Option<ConfigParams>,
}

impl MockEngine {
    pub async fn new(db_dir: &str, new_config: Option<ConfigParams>) -> Result<Self> {
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
                &|| Ok(()),
                None,
                Arc::new(AtomicU8::new(0)),
                Some(AccessType::ReadOnly),
                #[cfg(feature = "telemetry")]
                telemetry.clone(),
                allocated.clone(),
            )
            .await?,
        );
        let now =
            SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs() as u32;
        Ok(Self {
            db,
            now: Arc::new(AtomicU32::new(now)),
            ext_messages: Arc::new(MessagesPool::new(now, None).0),
            #[cfg(feature = "telemetry")]
            engine_telemetry: telemetry,
            #[cfg(feature = "telemetry")]
            collator_telemetry: CollatorValidatorTelemetry::default(),
            engine_allocated: allocated,
            collator_config: CollatorConfig {
                cutoff_timeout_ms: 30_000,
                stop_timeout_ms: 60_000,
                ..Default::default()
            },
            new_config,
        })
    }
}

#[async_trait::async_trait]
impl EngineOperations for MockEngine {
    fn now(&self) -> u32 {
        self.now.load(Ordering::Relaxed)
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
        self.db.load_full_node_state(LAST_APPLIED_MC_BLOCK)
    }
    async fn load_last_applied_mc_state(&self) -> Result<Arc<ShardStateStuff>> {
        if let Some(id) = self.load_last_applied_mc_block_id()? {
            self.load_state(&id).await
        } else {
            fail!("No last applied MC block set")
        }
    }
    fn load_shard_client_mc_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
        self.db.load_full_node_state(SHARD_CLIENT_MC_BLOCK)
    }

    fn load_block_prev1(&self, id: &BlockIdExt) -> Result<BlockIdExt> {
        self.db.load_block_prev1(id)
    }

    fn load_block_prev2(&self, id: &BlockIdExt) -> Result<Option<BlockIdExt>> {
        self.db.load_block_prev2(id)
    }

    fn load_block_next1(&self, id: &BlockIdExt) -> Result<BlockIdExt> {
        self.db.load_block_next1(id)
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

    async fn lookup_block_by_seqno(
        &self,
        prefix: &AccountIdPrefixFull,
        seqno: u32,
    ) -> Result<Option<(BlockIdExt, Vec<u8>)>> {
        self.db.lookup_block_by_seqno(prefix, seqno).await
    }

    async fn download_and_apply_block(
        self: Arc<Self>,
        _id: &BlockIdExt,
        _mc_seq_no: u32,
        _pre_apply: bool,
    ) -> Result<()> {
        Ok(())
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
        self.ext_messages.new_message(id, message, self.now())?;
        Ok(())
    }
    fn get_external_messages_iterator(
        &self,
        shard: ShardIdent,
        finish_time_ms: u64,
    ) -> Box<dyn Iterator<Item = (Arc<Message>, UInt256)> + Send + Sync> {
        Box::new(self.ext_messages.clone().iter(shard, self.now(), finish_time_ms))
    }
    fn get_external_messages_len(&self) -> u32 {
        0
    }
    fn complete_external_messages(
        &self,
        to_delay: Vec<(UInt256, String)>,
        to_delete: Vec<(UInt256, i32)>,
    ) -> Result<()> {
        self.ext_messages.complete_messages(to_delay, to_delete, self.now())
    }

    // return empty shard blocks - collator will get it from previous state
    async fn get_shard_blocks(
        &self,
        _last_mc_state: &Arc<ShardStateStuff>,
        _actual_last_mc_seqno: Option<&mut u32>,
    ) -> Result<Vec<Arc<TopBlockDescrStuff>>> {
        Ok(Vec::new())
    }

    #[cfg(feature = "telemetry")]
    fn engine_telemetry(&self) -> &Arc<EngineTelemetry> {
        &self.engine_telemetry
    }

    fn engine_allocated(&self) -> &Arc<EngineAlloc> {
        &self.engine_allocated
    }

    #[cfg(feature = "telemetry")]
    fn collator_telemetry(&self) -> &CollatorValidatorTelemetry {
        &self.collator_telemetry
    }

    fn collator_config(&self) -> &CollatorConfig {
        &self.collator_config
    }

    fn collator_config_mc(&self) -> &CollatorConfig {
        &self.collator_config
    }

    fn get_config_for_hardfork(&self) -> Option<ConfigParams> {
        self.new_config.clone()
    }
}

async fn run(args: clap::ArgMatches) -> Result<()> {
    let db_dir =
        args.get_one::<String>("PATH").map(|s| s.as_str()).expect("path to database must be set");

    let new_config = if let Some(json) = args.get_one::<String>("STATE").map(|s| s.as_str()) {
        let json = std::fs::read_to_string(json)?;
        let map = serde_json::from_str(&json)?;
        let new_config = ton_block_json::parse_config(&map)?;
        if new_config == ConfigParams::default() {
            fail!("new config is empty");
        }
        Some(new_config)
    } else {
        None
    };

    let engine = Arc::new(
        MockEngine::new(db_dir, new_config)
            .await
            .map_err(|err| error!("cannot create engine: {}", err))?,
    );

    let mc_state = engine.load_last_applied_mc_state().await?;
    let mc_block_id = mc_state.block_id();
    if args.get_flag("LAST") {
        let id = serde_json::json!({
            "workchain": -1,
            "shard": -9223372036854775808i64,
            "seqno": mc_block_id.seq_no,
            "root_hash": base64_encode(mc_block_id.root_hash.as_slice()),
            "file_hash": base64_encode(mc_block_id.file_hash.as_slice())
        });
        println!("{:#}", id);
        let caps = mc_state.config_params().unwrap().get_global_version().unwrap();
        println!("Global version: {:?}", caps);
        /*let ss = ton_block_json::serialize_config_param(mc_state.config_params().unwrap(), 34).unwrap();
        println!("Validator set2: {}", ss);*/

        /*let block_id = mc_state.find_block_id(700).unwrap();
        println!("Prev state: {}", mc_state.has_prev_block(&block_id).unwrap());
        let state = engine.load_state(&block_id).await.unwrap();*/
    }
    if let Some(mc_seq_no) = args.get_one::<String>("SEQ NO").map(|s| s.as_str()) {
        let mc_seq_no = u32::from_str(mc_seq_no).expect("masterchain seq no must be u32");
        let prev_block_id = if mc_seq_no - 1 == mc_block_id.seq_no {
            mc_block_id.clone()
        } else {
            mc_state.find_block_id(mc_seq_no - 1)?
        };
        let mc_state = engine.load_state(&prev_block_id).await?;
        let cc_seqno = mc_state.shard_state_extra().unwrap().validator_info.catchain_seqno;

        let config = mc_state.config_params()?;
        let vset = config.validator_set()?;
        let validator_set = calc_subset_for_masterchain(&vset, config, cc_seqno)?;
        let mut v_set = validator_set.compute_validator_set(cc_seqno).unwrap();
        v_set.set_cc_seqno(cc_seqno);

        let shard = ShardIdent::masterchain();
        let seqno = prev_block_id.seq_no();
        let prev = PrevBlockHistory::with_prevs(&shard, vec![prev_block_id]);
        let collator = Collator::new(
            shard,
            seqno,
            &prev,
            PipelineContext::new(),
            v_set,
            UInt256::default(),
            engine,
            None,
            CollatorSettings::default(),
        )?;
        let (block, state, id, data) = match collator.collate().await {
            Err(e) => fail!("Cannot craft hardfork block: {}", e),
            Ok(CollateResult::Err { err, .. }) => fail!("Cannot craft hardfork block: {}", err),
            Ok(CollateResult::Ok { new_block, new_state, candidate, .. }) => {
                (new_block, new_state, candidate.block_id, candidate.data)
            }
        };
        // println!(
        //     "Crafted hardfork block \n{}",
        //     ton_block_json::debug_block_full(&block).unwrap()
        // );
        // ton_block_json::debug_state(new_state).unwrap();

        assert!(state.read_custom().unwrap().unwrap().after_key_block);
        assert!(block.read_info().unwrap().key_block());

        let file_name = format!("{:x}", id.root_hash);
        std::fs::write(&file_name, &data).unwrap();
        let id = serde_json::json!({
            "hardforks": [{
                "workchain": -1,
                "shard": -9223372036854775808i64,
                "seqno": id.seq_no,
                "root_hash": base64_encode(id.root_hash.as_slice()),
                "file_hash": base64_encode(id.file_hash.as_slice())
            }]
        });
        println!("{:#}", id);
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    // init_log("common/config/log_cfg_debug.yml");

    let args = Command::new(env!("CARGO_PKG_NAME"))
        .version(env!("CARGO_PKG_VERSION"))
        .arg(Arg::new("PATH").short('p').long("path").help("path to DB").num_args(1).required(true))
        .arg(
            Arg::new("SEQ NO")
                .short('q')
                .long("seqno")
                .num_args(1)
                .help("seq no of masterchain block"),
        )
        .arg(
            Arg::new("STATE").short('s').long("state").num_args(1).help("file with new state json"),
        )
        .arg(Arg::new("LAST").short('l').long("last").help("last seq no of masterchain block"))
        .get_matches();

    run(args).await.unwrap();
}
