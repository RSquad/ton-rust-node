/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::*;
#[cfg(feature = "telemetry")]
use crate::collator_test_bundle::create_engine_telemetry;
use crate::{
    block::BlockStuff,
    collator_test_bundle::create_engine_allocated,
    config::{NodeConfigHandler, TestEmulatorConsensusConfig, TonNodeConfig, ValidatorKeysJson},
    engine::{run, EngineFlags, Stopper},
    engine_traits::{EngineOperations, PrivateOverlayOperations, ValidatorListOutcome},
    internal_db::{InternalDb, InternalDbConfig},
    network::node_network::NodeNetwork,
    shard_state::ShardStateStuff,
    validator::consensus::{ConsensusType, EmulatorConsensusOptions},
};
use adnl::{node::AdnlNodeConfig, PrivateOverlayShortId};
use catchain::{
    CatchainNode, CatchainOverlay, CatchainOverlayListenerPtr, CatchainOverlayLogReplayListenerPtr,
};
use consensus_common::{OverlayTransportType, PrivateKey};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        Arc, RwLock,
    },
    time::{Duration, Instant},
};
use storage::block_handle_db::BlockHandle;
use tempfile::TempDir;
use ton_block::{
    error, fail, signature::SigPubKey, validators::ValidatorDescr, Account, AccountId, BlkPrevInfo,
    Block, BlockCreateFees, BlockIdExt, BlockInfo, BlockLimits, Coins, ConfigParam12,
    ConfigParam14, ConfigParam18, ConfigParam2, ConfigParam3, ConfigParam31, ConfigParam34,
    ConfigParamEnum, ConfigParams, Ed25519KeyOption, ExtBlkRef, FutureSplitMerge, GasLimitsPrices,
    KeyOptionJson, MsgAddressInt, MsgForwardPrices, NewConsensusConfigAll, ParamLimits,
    Serializable, ShardAccount, ShardIdent, ShardStateUnsplit, StoragePrices, UInt256,
    ValidatorSet, WorkchainDescr, ZeroizingBytes, MASTERCHAIN_ID, SHARD_FULL,
};
use validator_session::SessionId;

const CONFIG_FILE_NAME: &str = "config.json";
const GLOBAL_CONFIG_FILE_NAME: &str = "ton-global.config.json";
const VALIDATOR_MANAGER_CONFIG_DIR_NAME: &str = "validator-manager-config";
const VALIDATOR_MANAGER_EMULATOR_CONFIG_FILE_NAME: &str = "emulator-consensus.json";
const ZEROSTATE_FILE_NAME: &str = "zerostate.boc";
const BASESTATE_FILE_NAME: &str = "basestate0.boc";
const VALIDATOR_WEIGHT: u64 = 1;
const ELECTION_ID_BASE: i32 = 1;
const VALIDATOR_EXPIRE_AT: i32 = i32::MAX;
const ZEROSTATE_GEN_TIME: u32 = 1 << 17;

#[derive(Clone)]
pub struct TestValidatorKeyPair {
    pub validator_key: PrivateKey,
    pub adnl_key: PrivateKey,
    validator_key_json: KeyOptionJson,
    adnl_key_json: KeyOptionJson,
    election_id: i32,
    pub validator_descr: ValidatorDescr,
}

impl TestValidatorKeyPair {
    fn generate(index: usize) -> Result<Self> {
        let (validator_key_json, validator_key) =
            Ed25519KeyOption::<ZeroizingBytes>::generate_with_json()?;
        let (adnl_key_json, adnl_key) = Ed25519KeyOption::<ZeroizingBytes>::generate_with_json()?;
        let validator_descr = ValidatorDescr::with_params(
            SigPubKey::from_bytes(validator_key.pub_key()?)?,
            VALIDATOR_WEIGHT,
            Some(UInt256::from_slice(adnl_key.id().data().as_slice())),
        );

        Ok(Self {
            validator_key,
            adnl_key,
            validator_key_json,
            adnl_key_json,
            election_id: ELECTION_ID_BASE + index as i32,
            validator_descr,
        })
    }

    fn validator_key_id(&self) -> String {
        base64_encode(self.validator_key.id().data())
    }

    fn adnl_key_id(&self) -> String {
        base64_encode(self.adnl_key.id().data())
    }

    fn validator_key_entry(&self) -> ValidatorKeysJson {
        ValidatorKeysJson {
            expire_at: VALIDATOR_EXPIRE_AT,
            election_id: self.election_id,
            validator_key_id: self.validator_key_id(),
            validator_adnl_key_id: Some(self.adnl_key_id()),
        }
    }
}

pub struct EmulatorNetworkBootstrap {
    temp_dir: TempDir,
    pub validators: Vec<TestValidatorKeyPair>,
    pub zerostate_id: BlockIdExt,
    pub base_state_id: Option<BlockIdExt>,
    pub config_path: PathBuf,
    config_params: ConfigParams,
    validator_set: ValidatorSet,
}

impl EmulatorNetworkBootstrap {
    pub fn build(validator_count: usize) -> Result<Self> {
        assert!(validator_count > 0, "emulator network must have at least one validator");

        let temp_dir = tempfile::tempdir()?;
        let validators =
            (0..validator_count).map(TestValidatorKeyPair::generate).collect::<Result<Vec<_>>>()?;
        let validator_set = make_validator_set(&validators)?;
        let (base_state_id, base_state_boc) = make_basechain_zerostate()?;
        let config_params = make_config_params(&validator_set, &base_state_id)?;
        let (zerostate_id, zerostate_boc) = make_masterchain_zerostate(
            config_params.clone(),
            &validator_set,
            &base_state_id,
            temp_dir.path().join(ZEROSTATE_FILE_NAME),
        )?;

        write_global_config(temp_dir.path(), &zerostate_id)?;
        let config_path = write_node_config(temp_dir.path(), &validators)?;

        // Keep the BOC write close to the ID calculation so commit 3 can boot Engine
        // directly from the generated bundle.
        fs::write(temp_dir.path().join(ZEROSTATE_FILE_NAME), zerostate_boc)?;
        fs::write(temp_dir.path().join(BASESTATE_FILE_NAME), base_state_boc)?;
        write_engine_static_zerostate_files(temp_dir.path(), &zerostate_id, &base_state_id)?;

        Ok(Self {
            temp_dir,
            validators,
            zerostate_id,
            base_state_id: Some(base_state_id),
            config_path,
            config_params,
            validator_set,
        })
    }

    pub fn config_dir(&self) -> &Path {
        self.temp_dir.path()
    }

    pub async fn load_node_config(&self) -> Result<TonNodeConfig> {
        TonNodeConfig::from_file(
            self.config_dir().to_str().expect("temporary path must be valid UTF-8"),
            CONFIG_FILE_NAME,
            None,
            CONFIG_FILE_NAME,
            None,
        )
        .await
    }

    pub fn validator_private_keys(&self) -> Vec<PrivateKey> {
        self.validators.iter().map(|v| v.validator_key.clone()).collect()
    }

    pub fn emulator_consensus_options(&self) -> ConsensusOptions {
        ConsensusOptions::Emulator(EmulatorConsensusOptions::fixed_local(
            self.validator_private_keys(),
        ))
    }

    pub fn validator_set(&self) -> &ValidatorSet {
        &self.validator_set
    }

    pub fn config_params(&self) -> &ConfigParams {
        &self.config_params
    }

    pub async fn wait_for_validation_status<F>(
        &self,
        mut read_status: F,
        expected: ValidationStatus,
        timeout: Duration,
    ) -> Result<()>
    where
        F: FnMut() -> ValidationStatus,
    {
        wait_until(timeout, || read_status() == expected).await
    }

    pub async fn wait_for_current_session<F>(
        &self,
        mut has_session: F,
        timeout: Duration,
    ) -> Result<()>
    where
        F: FnMut() -> bool,
    {
        wait_until(timeout, || has_session()).await
    }
}

fn make_validator_set(validators: &[TestValidatorKeyPair]) -> Result<ValidatorSet> {
    ValidatorSet::with_cc_seqno(
        0,
        u32::MAX,
        validators.len() as u16,
        0,
        validators.iter().map(|v| v.validator_descr.clone()).collect(),
    )
}

fn make_config_params(
    validator_set: &ValidatorSet,
    base_state_id: &BlockIdExt,
) -> Result<ConfigParams> {
    let mut config = ConfigParams::with_address_and_params(AccountId::from([0; 32]), None);
    let catchain_config = CatchainConfig {
        isolate_mc_validators: false,
        shuffle_mc_validators: false,
        mc_catchain_lifetime: u32::MAX,
        shard_catchain_lifetime: u32::MAX,
        shard_validators_lifetime: u32::MAX,
        shard_validators_num: validator_set.list().len() as u32,
    };
    let consensus_config = ConsensusConfig {
        new_catchain_ids: true,
        round_candidates: 1,
        next_candidate_delay_ms: 100,
        consensus_timeout_ms: 1_000,
        fast_attempts: 1,
        attempt_duration: 1_000,
        catchain_max_deps: 4,
        max_block_bytes: 2 * 1024 * 1024,
        max_collated_bytes: 2 * 1024 * 1024,
        proto_version: 0,
        catchain_max_blocks_coeff: 0,
    };
    let simplex_config = SimplexConfig::default();
    let block_limits = make_block_limits()?;
    let mut workchain_descr = WorkchainDescr::default();
    workchain_descr.active = true;
    workchain_descr.accept_msgs = true;
    workchain_descr.zerostate_root_hash = base_state_id.root_hash.clone();
    workchain_descr.zerostate_file_hash = base_state_id.file_hash.clone();
    let mut workchains = ConfigParam12::default();
    workchains.insert(0, &workchain_descr)?;

    config.set_config(ConfigParamEnum::ConfigParam12(workchains))?;
    config.set_config(ConfigParamEnum::ConfigParam2(ConfigParam2 {
        minter_addr: AccountId::from([0; 32]),
    }))?;
    config.set_config(ConfigParamEnum::ConfigParam3(ConfigParam3 {
        fee_collector_addr: AccountId::from([0; 32]),
    }))?;
    config.set_config(ConfigParamEnum::ConfigParam14(ConfigParam14 {
        block_create_fees: BlockCreateFees {
            masterchain_block_fee: Coins::default(),
            basechain_block_fee: Coins::default(),
        },
    }))?;
    config.set_config(ConfigParamEnum::ConfigParam18(make_storage_prices()?))?;
    config.set_config(ConfigParamEnum::ConfigParam20(make_gas_prices()))?;
    config.set_config(ConfigParamEnum::ConfigParam21(make_gas_prices()))?;
    config.set_config(ConfigParamEnum::ConfigParam22(block_limits.clone()))?;
    config.set_config(ConfigParamEnum::ConfigParam23(block_limits))?;
    config.set_config(ConfigParamEnum::ConfigParam24(make_msg_forward_prices()))?;
    config.set_config(ConfigParamEnum::ConfigParam25(make_msg_forward_prices()))?;
    config.set_config(ConfigParamEnum::ConfigParam28(catchain_config))?;
    config.set_config(ConfigParamEnum::ConfigParam29(consensus_config))?;
    config.set_config(ConfigParamEnum::ConfigParam30(NewConsensusConfigAll {
        mc: Some(simplex_config.clone()),
        shard: Some(simplex_config),
    }))?;
    config.set_config(ConfigParamEnum::ConfigParam31(ConfigParam31::default()))?;
    config.set_config(ConfigParamEnum::ConfigParam34(ConfigParam34 {
        cur_validators: validator_set.clone(),
    }))?;
    Ok(config)
}

fn make_storage_prices() -> Result<ConfigParam18> {
    let mut config = ConfigParam18::default();
    let prices = StoragePrices {
        utime_since: 0,
        bit_price_ps: 1,
        cell_price_ps: 1,
        mc_bit_price_ps: 1,
        mc_cell_price_ps: 1,
    };
    config.insert(&prices)?;
    Ok(config)
}

fn make_gas_prices() -> GasLimitsPrices {
    let mut prices = GasLimitsPrices::default();
    prices.gas_price = 1 << 16;
    prices.gas_limit = 10_000_000;
    prices.special_gas_limit = 10_000_000;
    prices.gas_credit = 10_000;
    prices.block_gas_limit = 10_000_000;
    prices.freeze_due_limit = 1_000_000_000;
    prices.delete_due_limit = 10_000_000_000;
    prices.flat_gas_limit = 100;
    prices.flat_gas_price = 100;
    prices.max_gas_threshold = prices.calc_max_gas_threshold(prices.gas_limit);
    prices
}

fn make_msg_forward_prices() -> MsgForwardPrices {
    MsgForwardPrices {
        lump_price: 1,
        bit_price: 1,
        cell_price: 1,
        ihr_price_factor: 0,
        first_frac: 21_845,
        next_frac: 21_845,
    }
}

fn make_block_limits() -> Result<BlockLimits> {
    Ok(BlockLimits::with_limits(
        ParamLimits::with_limits(1 << 20, 2 << 20, 4 << 20)?,
        ParamLimits::with_limits(1_000_000, 2_000_000, 4_000_000)?,
        ParamLimits::with_limits(1_000, 5_000, 10_000)?,
        None,
        None,
    ))
}

fn make_masterchain_zerostate(
    config_params: ConfigParams,
    validator_set: &ValidatorSet,
    base_state_id: &BlockIdExt,
    zerostate_path: PathBuf,
) -> Result<(BlockIdExt, Vec<u8>)> {
    let catchain_config = config_params.catchain_config()?;
    let (_subset, hash_short) =
        validator_set.calc_subset(&catchain_config, SHARD_FULL, MASTERCHAIN_ID, 0)?;

    let mut mc_extra = McStateExtra::default();
    mc_extra.config = config_params;
    mc_extra.validator_info.validator_list_hash_short = hash_short;
    mc_extra.validator_info.catchain_seqno = 0;
    mc_extra.validator_info.nx_cc_updated = true;
    mc_extra.after_key_block = true;
    let mut base_descr = ShardDescr::with_params(
        base_state_id.seq_no,
        0,
        0,
        base_state_id.root_hash.clone(),
        FutureSplitMerge::None,
    );
    base_descr.file_hash = base_state_id.file_hash.clone();
    base_descr.next_validator_shard = SHARD_FULL;
    base_descr.min_ref_mc_seqno = 0;
    mc_extra.add_workchain(0, &base_descr)?;

    let mut state = ShardStateUnsplit::with_ident(ShardIdent::masterchain());
    state.set_gen_time(ZEROSTATE_GEN_TIME);
    state.set_global_id(0);
    let config_account_id = AccountId::from([0; 32]);
    let config_account =
        Account::with_address(MsgAddressInt::standard(-1, config_account_id.clone()));
    state.insert_account(
        &config_account_id,
        &ShardAccount::with_params(&config_account, UInt256::default(), 0)?,
    )?;
    state.write_custom(Some(&mc_extra))?;

    let cell = state.serialize()?;
    let boc = ton_block::write_boc(&cell)?;
    let block_id = BlockIdExt::with_params(
        ShardIdent::masterchain(),
        0,
        cell.repr_hash().clone(),
        UInt256::calc_file_hash(&boc),
    );

    fs::write(zerostate_path, &boc)?;
    Ok((block_id, boc))
}

fn make_basechain_zerostate() -> Result<(BlockIdExt, Vec<u8>)> {
    let mut state = ShardStateUnsplit::with_ident(ShardIdent::with_workchain_id(0)?);
    state.set_gen_time(ZEROSTATE_GEN_TIME);
    state.set_global_id(0);

    let cell = state.serialize()?;
    let boc = ton_block::write_boc(&cell)?;
    let block_id = BlockIdExt::with_params(
        state.shard().clone(),
        0,
        cell.repr_hash().clone(),
        UInt256::calc_file_hash(&boc),
    );
    Ok((block_id, boc))
}

fn write_global_config(config_dir: &Path, zerostate_id: &BlockIdExt) -> Result<()> {
    let global_config = serde_json::json!({
        "@type": "config.global",
        "dht": {
            "@type": "dht.config.global",
            "k": 6,
            "a": 3,
            "static_nodes": {
                "@type": "dht.nodes",
                "nodes": []
            }
        },
        "validator": {
            "@type": "validator.config.global",
            "zero_state": block_id_json(zerostate_id),
            "hardforks": []
        }
    });
    write_json(config_dir.join(GLOBAL_CONFIG_FILE_NAME), &global_config)
}

fn write_node_config(config_dir: &Path, validators: &[TestValidatorKeyPair]) -> Result<PathBuf> {
    let (adnl_node, _) = AdnlNodeConfig::with_ip_address_and_private_key_tags(
        "127.0.0.1:0",
        vec![NodeNetwork::TAG_DHT_KEY, NodeNetwork::TAG_OVERLAY_KEY],
    )?;
    let validator_keys: Vec<_> =
        validators.iter().map(TestValidatorKeyPair::validator_key_entry).collect();
    let mut validator_key_ring = HashMap::new();
    for validator in validators {
        validator_key_ring
            .insert(validator.validator_key_id(), validator.validator_key_json.clone());
        validator_key_ring.insert(validator.adnl_key_id(), validator.adnl_key_json.clone());
    }
    write_validator_manager_config(config_dir, validators)?;

    let node_config = serde_json::json!({
        "ton_global_config_name": GLOBAL_CONFIG_FILE_NAME,
        "boot_from_zerostate": true,
        "internal_db_path": config_dir.join("node_db").to_string_lossy(),
        "validation_countdown_mode": "except-zerostate",
        "unsafe_catchain_patches_path": VALIDATOR_MANAGER_CONFIG_DIR_NAME,
        "sync_by_archives": false,
        "restore_db": false,
        "adnl_node": adnl_node,
        "validator_keys": validator_keys,
        "validator_key_ring": validator_key_ring,
        "json_rpc_server": null,
        "control_server": null,
        "lite_server": null,
        "test_bundles_config": {
            "collator": { "path": "" },
            "validator": { "path": "" }
        },
        "collator_config": {
            "cutoff_timeout_ms": 100,
            "stop_timeout_ms": 200,
            "max_collate_threads": 1,
            "retry_if_empty": false,
            "finalize_empty_after_ms": 50,
            "empty_collation_sleep_ms": 10
        }
    });

    let config_path = config_dir.join(CONFIG_FILE_NAME);
    write_json(&config_path, &node_config)?;
    Ok(config_path)
}

fn write_validator_manager_config(
    config_dir: &Path,
    validators: &[TestValidatorKeyPair],
) -> Result<()> {
    let validator_manager_config_dir = config_dir.join(VALIDATOR_MANAGER_CONFIG_DIR_NAME);
    fs::create_dir_all(&validator_manager_config_dir)?;
    let validator_keys =
        validators.iter().map(|v| v.validator_key_json.clone()).collect::<Vec<_>>();
    let config = serde_json::json!({
        "unsafe_resync_catchains": [],
        "unsafe_catchain_rotates": [],
        "test_emulator_consensus": {
            "validator_keys": validator_keys,
            "slot_interval_min_ms": 100,
            "slot_interval_max_ms": 100,
            "slot_interval_reliability": 0.0
        }
    });
    write_json(
        validator_manager_config_dir.join(VALIDATOR_MANAGER_EMULATOR_CONFIG_FILE_NAME),
        &config,
    )
}

fn write_engine_static_zerostate_files(
    config_dir: &Path,
    zerostate_id: &BlockIdExt,
    base_state_id: &BlockIdExt,
) -> Result<()> {
    fs::copy(
        config_dir.join(ZEROSTATE_FILE_NAME),
        config_dir.join(format!("{:x}.boc", zerostate_id.file_hash())),
    )?;
    fs::copy(
        config_dir.join(BASESTATE_FILE_NAME),
        config_dir.join(format!("{:x}.boc", base_state_id.file_hash())),
    )?;
    Ok(())
}

fn block_id_json(block_id: &BlockIdExt) -> serde_json::Value {
    serde_json::json!({
        "workchain": block_id.shard_id.workchain_id(),
        "shard": block_id.shard_id.shard_prefix_with_tag() as i64,
        "seqno": block_id.seq_no,
        "root_hash": base64_encode(block_id.root_hash.as_slice()),
        "file_hash": base64_encode(block_id.file_hash.as_slice()),
    })
}

fn write_json(path: impl AsRef<Path>, value: &serde_json::Value) -> Result<()> {
    fs::write(path, serde_json::to_string_pretty(value)?)?;
    Ok(())
}

async fn wait_until<F>(timeout: Duration, mut done: F) -> Result<()>
where
    F: FnMut() -> bool,
{
    let started = Instant::now();
    loop {
        if done() {
            return Ok(());
        }
        if started.elapsed() >= timeout {
            fail!("timed out waiting for emulator bootstrap condition");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

struct BootstrapValidatorNetwork {
    local_keys: Vec<PrivateKey>,
    contexts: RwLock<HashMap<UInt256, bool>>,
}

impl BootstrapValidatorNetwork {
    fn new(local_keys: Vec<PrivateKey>) -> Self {
        Self { local_keys, contexts: RwLock::new(HashMap::new()) }
    }

    fn select_local_keys(&self, validators: &[CatchainNode]) -> Vec<PrivateKey> {
        let mut keys = self
            .local_keys
            .iter()
            .filter(|key| {
                validators.iter().any(|node| node.public_key.id().data() == key.id().data())
            })
            .cloned()
            .map(|key| (*key.id().data(), key))
            .collect::<Vec<_>>();

        keys.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
        keys.dedup_by(|(left, _), (right, _)| left == right);
        keys.into_iter().map(|(_, key)| key).collect()
    }
}

#[async_trait::async_trait]
impl PrivateOverlayOperations for BootstrapValidatorNetwork {
    async fn set_validator_list(
        &self,
        validator_list_id: UInt256,
        validators: &[CatchainNode],
    ) -> Result<ValidatorListOutcome> {
        let matching_keys = self.select_local_keys(validators);
        if matching_keys.is_empty() {
            return Ok(ValidatorListOutcome::NotValidator);
        }

        self.contexts.write().unwrap().insert(validator_list_id, true);
        Ok(ValidatorListOutcome::Selected { key: matching_keys[0].clone(), matching_keys })
    }

    fn has_validator_list_context(&self, validator_list_id: &UInt256) -> bool {
        self.contexts.read().unwrap().contains_key(validator_list_id)
    }

    fn activate_validator_list(&self, validator_list_id: UInt256) -> Result<()> {
        self.contexts.write().unwrap().insert(validator_list_id, true);
        Ok(())
    }

    fn remove_validator_list(&self, validator_list_id: UInt256) -> Result<bool> {
        Ok(self.contexts.write().unwrap().remove(&validator_list_id).is_some())
    }

    fn create_catchain_client(
        &self,
        _validator_list_id: UInt256,
        _local_validator_key: &PrivateKey,
        _overlay_short_id: &Arc<PrivateOverlayShortId>,
        _nodes_public_keys: &[CatchainNode],
        _listener: CatchainOverlayListenerPtr,
        _log_replay_listener: CatchainOverlayLogReplayListenerPtr,
        _broadcast_hops: Option<u8>,
        _transport_type: OverlayTransportType,
        _block_sync_params: Option<consensus_common::BlockSyncOverlayParams>,
    ) -> Result<Arc<dyn CatchainOverlay + Send>> {
        fail!("bootstrap emulator test must not create catchain overlays")
    }

    fn stop_catchain_client(&self, _overlay_short_id: &Arc<PrivateOverlayShortId>) {}
}

struct BootstrapTestEngine {
    db: Arc<InternalDb>,
    validator_network: Arc<BootstrapValidatorNetwork>,
    states: RwLock<HashMap<BlockIdExt, Arc<ShardStateStuff>>>,
    blocks: RwLock<HashMap<BlockIdExt, BlockStuff>>,
    last_applied_mc_block_id: RwLock<Option<Arc<BlockIdExt>>>,
    last_rotation_block_id: RwLock<Option<Arc<BlockIdExt>>>,
    validation_status: RwLock<ValidationStatus>,
    will_validate: AtomicBool,
    stop: AtomicBool,
    flags: EngineFlags,
    hardforks: Vec<BlockIdExt>,
    init_mc_block_id: BlockIdExt,
    db_root: String,
    last_validation_time: lockfree::map::Map<ShardIdent, u64>,
    last_collation_time: lockfree::map::Map<ShardIdent, u64>,
    #[cfg(feature = "telemetry")]
    telemetry: Arc<crate::engine_traits::EngineTelemetry>,
    allocated: Arc<crate::engine_traits::EngineAlloc>,
}

impl BootstrapTestEngine {
    async fn new(bootstrap: &EmulatorNetworkBootstrap) -> Result<Self> {
        Self::new_with_local_keys(bootstrap, bootstrap.validator_private_keys()).await
    }

    async fn new_with_local_keys(
        bootstrap: &EmulatorNetworkBootstrap,
        local_keys: Vec<PrivateKey>,
    ) -> Result<Self> {
        let db_root = bootstrap.config_dir().join("manager_test_db");
        fs::create_dir_all(&db_root)?;
        #[cfg(feature = "telemetry")]
        let telemetry = create_engine_telemetry();
        let allocated = create_engine_allocated();
        let db_config = InternalDbConfig {
            db_directory: db_root.to_string_lossy().into_owned(),
            ..Default::default()
        };
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

        Ok(Self {
            db,
            validator_network: Arc::new(BootstrapValidatorNetwork::new(local_keys)),
            states: RwLock::new(HashMap::new()),
            blocks: RwLock::new(HashMap::new()),
            last_applied_mc_block_id: RwLock::new(None),
            last_rotation_block_id: RwLock::new(None),
            validation_status: RwLock::new(ValidationStatus::Disabled),
            will_validate: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            flags: EngineFlags {
                initial_sync_disabled: true,
                force_check_db: false,
                truncate_db: None,
            },
            hardforks: Vec::new(),
            init_mc_block_id: bootstrap.zerostate_id.clone(),
            db_root: db_root.to_string_lossy().into_owned(),
            last_validation_time: lockfree::map::Map::new(),
            last_collation_time: lockfree::map::Map::new(),
            #[cfg(feature = "telemetry")]
            telemetry,
            allocated,
        })
    }

    fn build_masterchain_state(
        &self,
        bootstrap: &EmulatorNetworkBootstrap,
        seqno: u32,
    ) -> Result<Arc<ShardStateStuff>> {
        let catchain_config = bootstrap.config_params().catchain_config()?;
        let (_subset, hash_short) = bootstrap.validator_set().calc_subset(
            &catchain_config,
            SHARD_FULL,
            MASTERCHAIN_ID,
            0,
        )?;

        let base_state_id = bootstrap
            .base_state_id
            .as_ref()
            .ok_or_else(|| error!("bootstrap must include basechain zerostate"))?;
        let shard_top = if seqno == 0 {
            base_state_id.clone()
        } else {
            synthetic_block_id(ShardIdent::with_workchain_id(0)?, seqno, 0x51)
        };

        let mut base_descr = ShardDescr::with_params(
            shard_top.seq_no,
            0,
            seqno as u64,
            shard_top.root_hash.clone(),
            FutureSplitMerge::None,
        );
        base_descr.file_hash = shard_top.file_hash.clone();
        base_descr.next_validator_shard = SHARD_FULL;
        base_descr.min_ref_mc_seqno = seqno;
        base_descr.gen_utime = seqno;

        let mut mc_extra = McStateExtra::default();
        mc_extra.config = bootstrap.config_params().clone();
        mc_extra.validator_info.validator_list_hash_short = hash_short;
        mc_extra.validator_info.catchain_seqno = 0;
        mc_extra.validator_info.nx_cc_updated = seqno == 0;
        mc_extra.after_key_block = seqno == 0;
        if seqno > 0 {
            mc_extra.last_key_block = Some(ext_blk_ref(&bootstrap.zerostate_id));
        }
        mc_extra.add_workchain(0, &base_descr)?;

        let mut state = ShardStateUnsplit::with_ident(ShardIdent::masterchain());
        if seqno > 0 {
            state.set_seq_no(seqno);
        }
        state.set_gen_time(seqno);
        state.set_global_id(0);
        state.write_custom(Some(&mc_extra))?;

        let cell = state.serialize()?;
        let boc = ton_block::write_boc(&cell)?;
        let block_id = BlockIdExt::with_params(
            ShardIdent::masterchain(),
            seqno,
            cell.repr_hash().clone(),
            UInt256::calc_file_hash(&boc),
        );

        ShardStateStuff::from_state(
            block_id,
            state,
            #[cfg(feature = "telemetry")]
            &self.telemetry,
            &self.allocated,
        )
    }

    fn install_masterchain_state(
        &self,
        state: Arc<ShardStateStuff>,
        prev_id: Option<BlockIdExt>,
    ) -> Result<Arc<BlockHandle>> {
        let block = make_masterchain_block(state.block_id().clone(), prev_id)?;
        let handle = if state.block_id().seq_no == 0 {
            self.db
                .create_or_load_block_handle(state.block_id(), None, Some(0), None)?
                .to_non_updated()
        } else {
            self.db
                .create_or_load_block_handle(state.block_id(), Some(block.block()?), None, None)?
                .to_non_updated()
        }
        .ok_or_else(|| error!("cannot create handle for {}", state.block_id()))?;

        self.states.write().unwrap().insert(state.block_id().clone(), state.clone());
        self.blocks.write().unwrap().insert(state.block_id().clone(), block);
        *self.last_applied_mc_block_id.write().unwrap() = Some(Arc::new(state.block_id().clone()));
        Ok(handle)
    }
}

#[async_trait::async_trait]
impl EngineOperations for BootstrapTestEngine {
    async fn check_sync(&self) -> Result<bool> {
        Ok(true)
    }

    fn set_will_validate(&self, will_validate: bool) {
        self.will_validate.store(will_validate, Ordering::Relaxed);
    }

    fn get_validator_status(&self) -> bool {
        true
    }

    fn validator_network(&self) -> Arc<dyn PrivateOverlayOperations> {
        self.validator_network.clone()
    }

    fn validation_status(&self) -> ValidationStatus {
        *self.validation_status.read().unwrap()
    }

    fn set_validation_status(&self, status: ValidationStatus) {
        *self.validation_status.write().unwrap() = status;
    }

    fn last_validation_time(&self) -> &lockfree::map::Map<ShardIdent, u64> {
        &self.last_validation_time
    }

    fn set_last_validation_time(&self, shard: ShardIdent, time: u64) {
        self.last_validation_time.insert(shard, time);
    }

    fn remove_last_validation_time(&self, shard: &ShardIdent) {
        self.last_validation_time.remove(shard);
    }

    fn last_collation_time(&self) -> &lockfree::map::Map<ShardIdent, u64> {
        &self.last_collation_time
    }

    fn set_last_collation_time(&self, shard: ShardIdent, time: u64) {
        self.last_collation_time.insert(shard, time);
    }

    fn remove_last_collation_time(&self, shard: &ShardIdent) {
        self.last_collation_time.remove(shard);
    }

    async fn set_validator_list(
        &self,
        validator_list_id: UInt256,
        validators: &[CatchainNode],
    ) -> Result<ValidatorListOutcome> {
        self.validator_network.set_validator_list(validator_list_id, validators).await
    }

    fn activate_validator_list(&self, validator_list_id: UInt256) -> Result<()> {
        self.validator_network.activate_validator_list(validator_list_id)
    }

    fn remove_validator_list(&self, validator_list_id: UInt256) -> Result<bool> {
        self.validator_network.remove_validator_list(validator_list_id)
    }

    fn load_block_handle(&self, id: &BlockIdExt) -> Result<Option<Arc<BlockHandle>>> {
        self.db.load_block_handle(id)
    }

    async fn load_block(&self, handle: &Arc<BlockHandle>) -> Result<BlockStuff> {
        self.blocks
            .read()
            .unwrap()
            .get(handle.id())
            .cloned()
            .ok_or_else(|| error!("no synthetic block for {}", handle.id()))
    }

    fn load_last_applied_mc_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
        Ok(self.last_applied_mc_block_id.read().unwrap().clone())
    }

    async fn load_last_applied_mc_state(&self) -> Result<Arc<ShardStateStuff>> {
        let id = self
            .load_last_applied_mc_block_id()?
            .ok_or_else(|| error!("no last applied masterchain block"))?;
        self.load_state(&id).await
    }

    fn load_last_rotation_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
        Ok(self.last_rotation_block_id.read().unwrap().clone())
    }

    fn save_last_rotation_block_id(&self, info: &BlockIdExt) -> Result<()> {
        *self.last_rotation_block_id.write().unwrap() = Some(Arc::new(info.clone()));
        Ok(())
    }

    fn clear_last_rotation_block_id(&self) -> Result<()> {
        *self.last_rotation_block_id.write().unwrap() = None;
        Ok(())
    }

    fn load_destroyed_session_ids(&self) -> Result<Vec<UInt256>> {
        Ok(Vec::new())
    }

    fn save_destroyed_session_ids(&self, _ids: &std::collections::HashSet<UInt256>) -> Result<()> {
        Ok(())
    }

    fn clear_destroyed_session_ids(&self) -> Result<()> {
        Ok(())
    }

    fn destroy_block_candidates(&self, _session_id: &SessionId) -> Result<bool> {
        Ok(false)
    }

    async fn load_state(&self, block_id: &BlockIdExt) -> Result<Arc<ShardStateStuff>> {
        self.states
            .read()
            .unwrap()
            .get(block_id)
            .cloned()
            .ok_or_else(|| error!("no synthetic state for {}", block_id))
    }

    async fn wait_state(
        self: Arc<Self>,
        id: &BlockIdExt,
        _timeout_ms: Option<u64>,
        _allow_block_downloading: bool,
    ) -> Result<Arc<ShardStateStuff>> {
        self.load_state(id).await
    }

    fn hardforks(&self) -> &[BlockIdExt] {
        &self.hardforks
    }

    fn flags(&self) -> &EngineFlags {
        &self.flags
    }

    fn init_mc_block_id(&self) -> &BlockIdExt {
        &self.init_mc_block_id
    }

    fn db_root_dir(&self) -> Result<&str> {
        Ok(&self.db_root)
    }

    fn produce_shard_hashes_enabled(&self) -> bool {
        true
    }

    fn engine_allocated(&self) -> &Arc<crate::engine_traits::EngineAlloc> {
        &self.allocated
    }

    fn acquire_stop(&self, _mask: u32) {}

    fn check_stop(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn release_stop(&self, _mask: u32) {}
}

fn ext_blk_ref(id: &BlockIdExt) -> ExtBlkRef {
    ExtBlkRef {
        end_lt: id.seq_no as u64,
        seq_no: id.seq_no,
        root_hash: id.root_hash.clone(),
        file_hash: id.file_hash.clone(),
    }
}

fn synthetic_block_id(shard: ShardIdent, seqno: u32, tag: u8) -> BlockIdExt {
    let mut root = [tag; 32];
    root[0..4].copy_from_slice(&seqno.to_be_bytes());
    let mut file = [tag.wrapping_add(1); 32];
    file[0..4].copy_from_slice(&seqno.to_be_bytes());
    BlockIdExt::with_params(shard, seqno, UInt256::from(root), UInt256::from(file))
}

fn make_masterchain_block(id: BlockIdExt, prev_id: Option<BlockIdExt>) -> Result<BlockStuff> {
    if id.seq_no == 0 {
        return BlockStuff::fake_block(id, None, true);
    }

    let mut info = BlockInfo::new();
    info.set_shard(ShardIdent::masterchain());
    info.set_seq_no(id.seq_no)?;
    info.set_gen_catchain_seqno(0);
    if let Some(prev_id) = prev_id {
        info.set_prev_stuff(false, &BlkPrevInfo::Block { prev: ext_blk_ref(&prev_id) })?;
    }

    let mut block = Block::default();
    block.write_info(&info)?;
    Ok(BlockStuff::fake_with_block(id, block))
}

async fn wait_for_manager_sessions(manager: &ValidatorManagerImpl) -> Result<()> {
    let started = Instant::now();
    loop {
        if manager.current_sessions.len() == 2 {
            let mut all_started = true;
            for group in manager.current_sessions.values() {
                if group.get_status().await < ValidatorGroupStatus::Sync {
                    all_started = false;
                    break;
                }
            }
            if all_started {
                return Ok(());
            }
        }

        if started.elapsed() >= Duration::from_secs(5) {
            fail!("timed out waiting for emulator manager sessions to start");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn wait_for_engine_mc_seqno(
    engine: &(dyn EngineOperations + Send + Sync),
    target_seqno: u32,
    timeout: Duration,
) -> Result<BlockIdExt> {
    let started = Instant::now();
    loop {
        if let Some(block_id) = engine.load_last_applied_mc_block_id()? {
            if block_id.seq_no >= target_seqno {
                return Ok(block_id.as_ref().clone());
            }
        }

        if started.elapsed() >= timeout {
            fail!("timed out waiting for real engine MC seqno {target_seqno}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[test]
fn test_bootstrap_generates_all_logical_validator_keys() -> Result<()> {
    let bootstrap = EmulatorNetworkBootstrap::build(4)?;

    assert_eq!(bootstrap.validators.len(), 4);
    assert_eq!(bootstrap.validator_private_keys().len(), 4);
    assert_eq!(bootstrap.validator_set().list().len(), 4);
    assert_eq!(bootstrap.zerostate_id.seq_no, 0);
    assert_eq!(bootstrap.base_state_id.as_ref().map(|id| id.seq_no), Some(0));
    assert!(bootstrap.config_path.exists());
    assert!(bootstrap.config_dir().join(ZEROSTATE_FILE_NAME).exists());
    assert!(bootstrap.config_dir().join(BASESTATE_FILE_NAME).exists());
    assert!(matches!(bootstrap.emulator_consensus_options(), ConsensusOptions::Emulator(_)));
    for validator in &bootstrap.validators {
        assert!(validator.validator_descr.adnl_addr.is_some());
    }

    Ok(())
}

#[test]
fn test_bootstrap_config_params_select_simplex() -> Result<()> {
    let bootstrap = EmulatorNetworkBootstrap::build(3)?;
    let config = bootstrap.config_params();

    assert_eq!(config.validator_set()?.list().len(), 3);
    assert!(config.get_mc_simplex_config()?.is_some());
    assert!(config.get_shard_simplex_config()?.is_some());
    assert_eq!(config.catchain_config()?.shard_validators_num, 3);

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_bootstrap_config_is_consumed_by_node_config_handler() -> Result<()> {
    let bootstrap = EmulatorNetworkBootstrap::build(3)?;
    let config = bootstrap.load_node_config().await?;
    let (handler, context) = NodeConfigHandler::create(config, tokio::runtime::Handle::current())?;

    handler.clone().start_sheduler(context, Vec::new())?;

    // Poll the actual handler state so these waits are real ordering
    // barriers (the original closures `|| ValidationStatus::Active` and
    // `|| true` always returned the expected values, which made the
    // waits no-ops and could mask races on slow CI). `start_sheduler`
    // calls `load_config` synchronously today so both checks normally
    // succeed on the first poll, but routing them through the handler
    // documents intent and makes the test robust against a future
    // async-ification of `load_config`. Bumped timeout from 1ms to 2s
    // to leave room on contended CI runners.
    let handler_status = handler.clone();
    bootstrap
        .wait_for_validation_status(
            move || {
                if handler_status.get_validator_status() {
                    ValidationStatus::Active
                } else {
                    ValidationStatus::Disabled
                }
            },
            ValidationStatus::Active,
            Duration::from_secs(2),
        )
        .await?;
    let handler_session = handler.clone();
    bootstrap
        .wait_for_current_session(
            move || {
                handler_session
                    .get_actual_validator_key_ids()
                    .map(|ids| !ids.is_empty())
                    .unwrap_or(false)
            },
            Duration::from_secs(2),
        )
        .await?;

    assert!(handler.get_validator_status());
    assert_eq!(handler.get_actual_validator_key_ids()?.len(), 3);
    assert_eq!(handler.get_actual_validator_adnl_key_ids()?.len(), 3);
    assert_eq!(handler.get_actual_validator_keys()?.len(), 3);

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_manager_creates_emulator_sessions_from_generated_mc_states() -> Result<()> {
    const TARGET_SEQNO: u32 = 50;

    let bootstrap = EmulatorNetworkBootstrap::build(4)?;
    assert!(bootstrap.config_path.exists());
    assert!(bootstrap.config_dir().join(GLOBAL_CONFIG_FILE_NAME).exists());
    assert!(bootstrap.config_dir().join(ZEROSTATE_FILE_NAME).exists());
    assert!(bootstrap.config_dir().join(BASESTATE_FILE_NAME).exists());

    let engine = Arc::new(BootstrapTestEngine::new(&bootstrap).await?);
    let mut config = ValidatorManagerConfig::default();
    config.update_interval = Duration::from_millis(10);
    config.no_countdown_for_zerostate = true;
    let mut emulator_config =
        TestEmulatorConsensusConfig::fixed_local(bootstrap.validator_private_keys());
    emulator_config.slot_interval_min_ms = 1_000;
    emulator_config.slot_interval_max_ms = 1_000;
    config.test_emulator_consensus = Some(emulator_config);

    let mut manager =
        ValidatorManagerImpl::create(engine.clone(), tokio::runtime::Handle::current(), config);

    let mut prev_id = None;
    for seqno in 0..=TARGET_SEQNO {
        let mc_state = engine.build_masterchain_state(&bootstrap, seqno)?;
        engine.install_masterchain_state(mc_state.clone(), prev_id)?;
        manager.update_shards(mc_state.clone()).await?;
        prev_id = Some(mc_state.block_id().clone());
    }

    wait_for_manager_sessions(&manager).await?;

    let last_applied = engine.load_last_applied_mc_block_id()?.expect("last applied MC block");
    assert_eq!(last_applied.seq_no, TARGET_SEQNO);
    assert_eq!(engine.validation_status(), ValidationStatus::Active);
    assert_eq!(manager.current_sessions.len(), 2);
    assert_eq!(manager.owned_current_shards, 2);

    let mut has_masterchain = false;
    let mut has_basechain = false;
    for group in manager.current_sessions.values() {
        let snapshot = group.snapshot().await;
        assert_eq!(snapshot.consensus_type, ConsensusType::Simplex);
        assert!(snapshot.has_engine);
        if snapshot.shard.is_masterchain() {
            has_masterchain = true;
        } else if snapshot.shard.workchain_id() == 0 {
            has_basechain = true;
        }
    }
    assert!(has_masterchain);
    assert!(has_basechain);

    let session_ids =
        manager.current_sessions.keys().cloned().collect::<std::collections::HashSet<_>>();
    manager.stop_and_remove_sessions(&session_ids, true).await;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_manager_selects_validator_key_by_validator_key_order() -> Result<()> {
    const TARGET_SEQNO: u32 = 50;

    let bootstrap = EmulatorNetworkBootstrap::build(4)?;

    let mut descending_local_keys = bootstrap.validator_private_keys();
    descending_local_keys.sort_unstable_by(|left, right| right.id().data().cmp(left.id().data()));
    let descending_order_ids: Vec<[u8; 32]> =
        descending_local_keys.iter().map(|key| *key.id().data()).collect();

    let mut expected_validator_order_ids = descending_order_ids.clone();
    expected_validator_order_ids.sort_unstable();
    expected_validator_order_ids.dedup();
    assert_ne!(
        descending_order_ids, expected_validator_order_ids,
        "test setup must use local key insertion order different from validator key order"
    );

    let engine = Arc::new(
        BootstrapTestEngine::new_with_local_keys(&bootstrap, descending_local_keys.clone()).await?,
    );
    let mut config = ValidatorManagerConfig::default();
    config.update_interval = Duration::from_millis(10);
    config.no_countdown_for_zerostate = true;
    let mut emulator_config =
        TestEmulatorConsensusConfig::fixed_local(bootstrap.validator_private_keys());
    emulator_config.slot_interval_min_ms = 1_000;
    emulator_config.slot_interval_max_ms = 1_000;
    config.test_emulator_consensus = Some(emulator_config);

    let mut manager =
        ValidatorManagerImpl::create(engine.clone(), tokio::runtime::Handle::current(), config);

    let mut prev_id = None;
    for seqno in 0..=TARGET_SEQNO {
        let mc_state = engine.build_masterchain_state(&bootstrap, seqno)?;
        engine.install_masterchain_state(mc_state.clone(), prev_id)?;
        manager.update_shards(mc_state.clone()).await?;
        prev_id = Some(mc_state.block_id().clone());
    }

    wait_for_manager_sessions(&manager).await?;

    let current_list_id = manager
        .validator_list_status
        .curr
        .clone()
        .ok_or_else(|| error!("expected current validator list id"))?;
    let local_keys_for_list = manager
        .validator_list_status
        .get_local_keys_for_list(&current_list_id)
        .ok_or_else(|| error!("expected local keys for current list"))?;
    let actual_list_order_ids: Vec<[u8; 32]> =
        local_keys_for_list.iter().map(|key| *key.id().data()).collect();
    assert_eq!(actual_list_order_ids, expected_validator_order_ids);

    let expected_first = expected_validator_order_ids
        .first()
        .copied()
        .ok_or_else(|| error!("expected at least one local validator key"))?;
    let expected_second = expected_validator_order_ids
        .iter()
        .copied()
        .find(|id| *id != expected_first)
        .ok_or_else(|| error!("expected at least two distinct local validator keys"))?;

    let all_validators = bootstrap.validator_set().list();
    let first_descr = all_validators
        .iter()
        .find(|val| val.compute_node_id_short().as_slice() == expected_first.as_slice())
        .ok_or_else(|| error!("missing descriptor for expected first key"))?
        .clone();
    let second_descr = all_validators
        .iter()
        .find(|val| val.compute_node_id_short().as_slice() == expected_second.as_slice())
        .ok_or_else(|| error!("missing descriptor for expected second key"))?
        .clone();

    // Deliberately place descriptors in a different order: key selection must still
    // follow local validator key order, not descriptor order.
    let selected = manager
        .find_us_for_list(&[second_descr, first_descr], &current_list_id)
        .ok_or_else(|| error!("expected selected local key for subset"))?;
    assert_eq!(*selected.id().data(), expected_first);

    let session_ids =
        manager.current_sessions.keys().cloned().collect::<std::collections::HashSet<_>>();
    manager.stop_and_remove_sessions(&session_ids, true).await;

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_real_engine_progresses_with_emulated_consensus() -> Result<()> {
    const TARGET_SEQNO: u32 = 50;

    let _ = env_logger::builder().is_test(true).try_init();
    let bootstrap = EmulatorNetworkBootstrap::build(4)?;

    let config = bootstrap.load_node_config().await?;
    let stopper = Arc::new(Stopper::new());
    let flags =
        EngineFlags { initial_sync_disabled: true, force_check_db: false, truncate_db: None };
    let static_zerostates_dir =
        bootstrap.config_dir().to_str().expect("temporary path must be valid UTF-8");

    let (engine, join_handle) = run(
        config,
        Some(static_zerostates_dir),
        tokio::runtime::Handle::current(),
        tokio::runtime::Handle::current(),
        flags,
        stopper.clone(),
        None,
    )
    .await?;

    let result = wait_for_engine_mc_seqno(
        engine.as_ref() as &(dyn EngineOperations + Send + Sync),
        TARGET_SEQNO,
        Duration::from_secs(60),
    )
    .await;

    stopper.set_stop();
    let _ = tokio::time::timeout(Duration::from_secs(10), join_handle).await;
    engine.wait_stop().await;

    let last_applied = result?;
    assert!(last_applied.seq_no >= TARGET_SEQNO);

    Ok(())
}
