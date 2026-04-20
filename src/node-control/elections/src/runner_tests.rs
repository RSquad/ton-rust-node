/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::*;
use common::{
    app_config::{ElectionsConfig, NodeBinding, StakePolicy},
    snapshot::SnapshotStore,
    task_cancellation::{CancellationCtx, CancellationReason},
    time_format,
};
use contracts::{
    ElectionsInfo, ElectorWrapper, NominatorWrapper, Participant, PoolKind, TonCoreNominatorRouter,
    TonWallet,
    elector::{FrozenParticipant, PastElections},
    nominator::{NominatorRoles, PoolData, SNP_STORAGE_RESERVE, TONCORE_STORAGE_RESERVE, opcodes},
};
use mockall::mock;
use std::{collections::HashMap, sync::Arc, time::Duration};
use ton_block::{
    BuilderData, Cell, Coins, ConfigParam15, Deserializable, MsgAddressInt, Number16, SigPubKey,
    SliceData, UInt256, ValidatorDescr, ValidatorSet,
    config_params::{ConfigParam16, ConfigParam17},
    read_single_root_boc,
};

// ---- Address helpers ----

const POOL_ADDR: [u8; 32] = [0xBBu8; 32];
const POOL_ADDR_1: [u8; 32] = [0xCCu8; 32];
const WALLET_ADDR: [u8; 32] = [0xAAu8; 32];

fn wallet_address() -> MsgAddressInt {
    MsgAddressInt::standard(-1, WALLET_ADDR)
}

fn pool_address() -> MsgAddressInt {
    MsgAddressInt::standard(-1, POOL_ADDR)
}

fn pool_address_1() -> MsgAddressInt {
    MsgAddressInt::standard(-1, POOL_ADDR_1)
}

fn elector_address() -> MsgAddressInt {
    MsgAddressInt::standard(-1, [0x33u8; 32])
}

fn addr_bytes(addr: &MsgAddressInt) -> Vec<u8> {
    addr.address().clone().storage().to_vec()
}

fn default_cfg15() -> ConfigParam15 {
    ConfigParam15 {
        validators_elected_for: 3600,
        elections_start_before: 1800,
        elections_end_before: 600,
        stake_held_for: 7200,
    }
}

fn default_cfg16() -> ConfigParam16 {
    ConfigParam16 {
        max_validators: Number16::from(400u16),
        max_main_validators: Number16::from(100u16),
        min_validators: Number16::from(13u16),
    }
}

fn default_cfg17() -> ConfigParam17 {
    ConfigParam17 {
        min_stake: Coins::from(10_000_000_000_000u64), // 10,000 TON
        max_stake: Coins::from(10_000_000_000_000_000u64), // 10,000,000 TON
        min_total_stake: Coins::from(100_000_000_000_000u64), // 100,000 TON
        max_stake_factor: 3 * 65536,                   // 3x
    }
}

fn dummy_cell() -> Cell {
    BuilderData::new().into_cell().unwrap()
}

const ELECTION_ID: u64 = 1_700_000_000;
const KEY_ID: [u8; 32] = [0x01u8; 32];
const PUB_KEY: [u8; 32] = [0x02u8; 32];
const ADNL_ADDR: [u8; 32] = [0x03u8; 32];
const SIGNATURE: [u8; 64] = [0x04u8; 64];

const MIN_STAKE: u64 = 10_000_000_000_000; // 10 000 TON
const WALLET_BALANCE: u64 = 50_000_000_000_000; // 50 000 TON
const POOL_BALANCE: u64 = 100_000_000_000_000; // 100 000 TON

// ---- Mock: ElectionsProvider ----

mock! {
    ElectionsProviderImpl {}

    #[async_trait::async_trait]
    impl ElectionsProvider for ElectionsProviderImpl {
        async fn setup(&self) -> anyhow::Result<()>;
        async fn shutdown(&mut self) -> anyhow::Result<()>;
        async fn new_validator_key(
            &mut self,
            since: u64,
            until: u64,
        ) -> anyhow::Result<(Vec<u8>, Vec<u8>)>;
        async fn new_adnl_addr(&mut self, perm_key_id: Vec<u8>, until: u64) -> anyhow::Result<Vec<u8>>;
        async fn validator_config(&mut self) -> anyhow::Result<ValidatorConfig>;
        async fn election_parameters(&mut self) -> anyhow::Result<ConfigParam15>;
        async fn send_boc(&mut self, msg_boc: &[u8]) -> anyhow::Result<()>;
        async fn sign(&mut self, key_hash: Vec<u8>, data: Vec<u8>) -> anyhow::Result<Vec<u8>>;
        async fn account(&mut self, address: &str) -> anyhow::Result<crate::providers::Account>;
        async fn export_public_key(&mut self, key_id: &[u8]) -> anyhow::Result<Vec<u8>>;
        async fn get_current_vset(&mut self) -> anyhow::Result<ValidatorSet>;
        async fn config_param_16(&mut self) -> anyhow::Result<ton_block::config_params::ConfigParam16>;
        async fn config_param_17(&mut self) -> anyhow::Result<ton_block::config_params::ConfigParam17>;
        async fn get_next_vset(&mut self) -> anyhow::Result<Option<ValidatorSet>>;
        async fn generate_adnl_addr(&mut self) -> anyhow::Result<Vec<u8>>;
        async fn register_adnl_addr(
            &mut self,
            adnl_key_id: Vec<u8>,
            perm_key_id: Vec<u8>,
            until: u64,
        ) -> anyhow::Result<()>;
    }
}

// ---- Mock: ElectorWrapper ----

mock! {
    ElectorWrapperImpl {}

    #[async_trait::async_trait]
    impl contracts::SmartContract for ElectorWrapperImpl {
        async fn balance(&self) -> anyhow::Result<u64>;
        async fn address(&self) -> anyhow::Result<MsgAddressInt>;
    }

    #[async_trait::async_trait]
    impl ElectorWrapper for ElectorWrapperImpl {
        async fn get_active_election_id(&self) -> anyhow::Result<u64>;
        async fn participates_in(&self, pubkey: &[u8]) -> anyhow::Result<Option<Participant>>;
        async fn compute_returned_stake(&self, address: &[u8]) -> anyhow::Result<u64>;
        async fn elections_info(&self) -> anyhow::Result<ElectionsInfo>;
        async fn past_elections(&self) -> anyhow::Result<Vec<PastElections>>;
    }
}

// ---- Mock: TonWallet ----

mock! {
    TonWalletImpl {}

    #[async_trait::async_trait]
    impl contracts::SmartContract for TonWalletImpl {
        async fn balance(&self) -> anyhow::Result<u64>;
        async fn address(&self) -> anyhow::Result<MsgAddressInt>;
    }

    #[async_trait::async_trait]
    impl TonWallet for TonWalletImpl {
        async fn message(
            &self,
            dest: MsgAddressInt,
            value: u64,
            payload: Cell,
        ) -> anyhow::Result<Cell>;

        async fn deploy_message(
            &self,
            value: u64,
            payload: Cell,
        ) -> anyhow::Result<Cell>;

        async fn build_message(
            &self,
            dest: MsgAddressInt,
            value: u64,
            payload: Cell,
            bounce: bool,
            seqno: Option<u32>,
            state_init_external: Option<ton_block::StateInit>,
            state_init_internal: Option<ton_block::StateInit>,
        ) -> anyhow::Result<Cell>;
    }
}

// ---- Mock: NominatorWrapper ----

mock! {
    SingleNominatorWrapper {}

    #[async_trait::async_trait]
    impl contracts::SmartContract for SingleNominatorWrapper {
        async fn balance(&self) -> anyhow::Result<u64>;
        async fn address(&self) -> anyhow::Result<MsgAddressInt>;
    }

    #[async_trait::async_trait]
    impl NominatorWrapper for SingleNominatorWrapper {
        async fn get_roles(&self) -> anyhow::Result<NominatorRoles>;
        async fn get_pool_data(&self) -> anyhow::Result<PoolData>;
        fn state_init(&self) -> Option<ton_block::StateInit>;
        fn inner_pools(&self) -> Vec<std::sync::Arc<dyn NominatorWrapper>>;
        fn storage_reserve(&self) -> u64;
        fn pool_kind(&self) -> PoolKind;
    }
}

// ---- Fake Account using control_client ----

fn fake_account(balance: u64) -> crate::providers::Account {
    crate::providers::Account::new(control_client::client_api::Account::ShardAccountState(
        control_client::client_api::ShardAccountState {
            balance: balance as u128,
            ..Default::default()
        },
    ))
}

fn validate_message_parameters(
    dest: &MsgAddressInt,
    value: &u64,
    payload: &Cell,
    expected_stake: u64,
) -> bool {
    // 1) dest must be the elector address (no pool)
    if *dest != elector_address() {
        eprintln!("withf: dest mismatch: expected elector, got {}", dest);
        return false;
    }

    // 2) value = stake + fee (ELECTOR_STAKE_FEE + NPOOL_COMPUTE_FEE)
    let fee = ELECTOR_STAKE_FEE + NPOOL_COMPUTE_FEE;
    let expected_value = expected_stake + fee;
    if *value != expected_value {
        eprintln!("withf: value mismatch: expected={}, got={}", expected_value, value);
        return false;
    }

    // 3) parse the payload cell (new_stake body)
    let mut slice = match SliceData::load_cell_ref(payload) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("withf: failed to load payload cell: {}", e);
            return false;
        }
    };

    // opcode = NEW_STAKE (0x4e73744b)
    let opcode = match slice.get_next_u32() {
        Ok(v) => v,
        Err(_) => return false,
    };
    if opcode != opcodes::NEW_STAKE {
        eprintln!(
            "withf: opcode mismatch: expected=0x{:08x}, got=0x{:08x}",
            opcodes::NEW_STAKE,
            opcode
        );
        return false;
    }

    // query_id (u64) — just verify it's nonzero
    let query_id = match slice.get_next_u64() {
        Ok(v) => v,
        Err(_) => return false,
    };
    if query_id == 0 {
        eprintln!("withf: query_id should be > 0");
        return false;
    }

    // stake_amount (Coins)
    let coins = match Coins::construct_from(&mut slice) {
        Ok(c) => c,
        Err(_) => return false,
    };
    if coins.as_u128() != expected_stake as u128 {
        eprintln!("withf: stake mismatch: expected={}, got={}", expected_stake, coins.as_u128());
        return false;
    }

    // validator_pubkey (256 bits)
    let pubkey = match slice.get_next_bits(256) {
        Ok(v) => v,
        Err(_) => return false,
    };
    if pubkey != PUB_KEY.to_vec() {
        eprintln!("withf: pubkey mismatch");
        return false;
    }

    // stake_at (u32) = election_id
    let stake_at = match slice.get_next_u32() {
        Ok(v) => v,
        Err(_) => return false,
    };
    if stake_at != ELECTION_ID as u32 {
        eprintln!("withf: stake_at mismatch: expected={}, got={}", ELECTION_ID, stake_at);
        return false;
    }

    // max_factor (u32)
    let max_factor = match slice.get_next_u32() {
        Ok(v) => v,
        Err(_) => return false,
    };
    if max_factor != 196608 {
        eprintln!("withf: max_factor mismatch: expected=196608, got={}", max_factor);
        return false;
    }

    // adnl_addr (256 bits)
    let adnl = match slice.get_next_bits(256) {
        Ok(v) => v,
        Err(_) => return false,
    };
    if adnl != ADNL_ADDR.to_vec() {
        eprintln!("withf: adnl mismatch");
        return false;
    }

    // signature reference cell (512 bits)
    let sig_cell = match slice.checked_drain_reference() {
        Ok(c) => c,
        Err(_) => {
            eprintln!("withf: missing signature reference");
            return false;
        }
    };
    let mut sig_slice = match SliceData::load_cell(sig_cell) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let sig = match sig_slice.get_next_bits(512) {
        Ok(v) => v,
        Err(_) => return false,
    };
    if sig != SIGNATURE.to_vec() {
        eprintln!("withf: signature mismatch");
        return false;
    }
    return true;
}

// ---- Builder helpers ----

fn default_binding(enable: bool) -> NodeBinding {
    NodeBinding { wallet: "wallet".to_string(), pool: None, enable, status: Default::default() }
}

struct TestHarness {
    elector_mock: MockElectorWrapperImpl,
    provider_mock: MockElectionsProviderImpl,
    wallet_mock: MockTonWalletImpl,
    pool_mock: Option<MockSingleNominatorWrapper>,
    toncore_nominator_mocks: Option<(MockSingleNominatorWrapper, MockSingleNominatorWrapper)>,
    elections_config: ElectionsConfig,
    bindings: HashMap<String, NodeBinding>,
}

impl TestHarness {
    fn new() -> Self {
        Self {
            elector_mock: MockElectorWrapperImpl::new(),
            provider_mock: MockElectionsProviderImpl::new(),
            wallet_mock: MockTonWalletImpl::new(),
            pool_mock: None,
            toncore_nominator_mocks: None,
            elections_config: ElectionsConfig {
                policy: StakePolicy::Split50,
                policy_overrides: HashMap::new(),
                max_factor: 3.0,
                tick_interval: 10,
                sleep_period_pct: 0.0,
                waiting_period_pct: 0.3,
                static_adnls: HashMap::new(),
            },
            bindings: HashMap::new(),
        }
    }

    fn with_pool(mut self) -> Self {
        self.pool_mock = Some(MockSingleNominatorWrapper::new());
        self
    }

    fn with_toncore_nominator_pair(mut self) -> Self {
        self.toncore_nominator_mocks =
            Some((MockSingleNominatorWrapper::new(), MockSingleNominatorWrapper::new()));
        self
    }

    async fn build(mut self, node_id: &str) -> ElectionRunner {
        self.bindings.entry(node_id.to_string()).or_insert_with(|| default_binding(true));

        let wallet: Arc<dyn TonWallet> = Arc::new(self.wallet_mock);
        let mut wallets: HashMap<String, Arc<dyn TonWallet>> = HashMap::new();
        wallets.insert(node_id.to_string(), wallet);

        let mut providers: HashMap<String, Box<dyn ElectionsProvider>> = HashMap::new();
        providers.insert(node_id.to_string(), Box::new(self.provider_mock));

        let mut pools: HashMap<String, Arc<dyn NominatorWrapper>> = HashMap::new();
        if let Some(pool) = self.pool_mock {
            pools.insert(node_id.to_string(), Arc::new(pool));
        } else if let Some((p0, p1)) = self.toncore_nominator_mocks {
            let p0: Arc<dyn NominatorWrapper> = Arc::new(p0);
            let p1: Arc<dyn NominatorWrapper> = Arc::new(p1);
            pools.insert(
                node_id.to_string(),
                Arc::new(TonCoreNominatorRouter::from_wrappers([Some(p0), Some(p1)])),
            );
        }

        let elector: Arc<dyn ElectorWrapper> = Arc::new(self.elector_mock);

        ElectionRunner::new(
            &self.elections_config,
            &self.bindings,
            elector,
            providers,
            Arc::new(wallets),
            Arc::new(pools),
        )
    }
}

// ---- Expectation helpers ----

/// Sets up a provider for a fresh election (no existing key):
/// generates new validator key, new adnl addr, signs, builds message, sends boc.
fn setup_default_provider(
    provider: &mut MockElectionsProviderImpl,
    wallet_balance: u64,
    pool_balance: Option<u64>,
) {
    provider.expect_election_parameters().returning(|| Ok(default_cfg15()));

    // validator_config: return empty (no existing key)
    provider.expect_validator_config().returning(|| Ok(ValidatorConfig::new()));

    // get_current_vset: not crucial, return error to skip
    provider.expect_get_current_vset().returning(|| Err(anyhow::anyhow!("no vset")));
    provider.expect_get_next_vset().returning(|| Ok(None));

    // new_validator_key
    provider
        .expect_new_validator_key()
        .returning(|_since, _until| Ok((KEY_ID.to_vec(), PUB_KEY.to_vec())));

    // new_adnl_addr
    provider.expect_new_adnl_addr().returning(|_key_id, _until| Ok(ADNL_ADDR.to_vec()));

    // export_public_key: needed for find_election_key
    provider.expect_export_public_key().returning(|_key_id| Ok(PUB_KEY.to_vec()));

    // sign
    provider.expect_sign().returning(|_key, _data| Ok(SIGNATURE.to_vec()));

    // account (for stake_balance and wallet_balance)
    if pool_balance.is_some() {
        // When pool exists, account is called for pool address (stake_balance)
        // and wallet address (wallet_balance)
        let pool_bal = pool_balance.unwrap();
        provider.expect_account().returning(move |address| {
            if address.contains(&hex::encode(POOL_ADDR)) {
                Ok(fake_account(pool_bal))
            } else {
                Ok(fake_account(wallet_balance))
            }
        });
    } else {
        provider.expect_account().returning(move |_addr| Ok(fake_account(wallet_balance)));
    }

    // send_boc
    provider.expect_send_boc().returning(|_boc| Ok(()));

    // config_param_16
    provider.expect_config_param_16().returning(|| Ok(default_cfg16()));

    // config_param_17
    provider.expect_config_param_17().returning(|| Ok(default_cfg17()));

    // shutdown
    provider.expect_shutdown().returning(|| Ok(()));
}

// Setup default elector contract state:
// - active elections
// - no participants
// - no past elections
// - no frozen stake
fn setup_default_elector(
    elector: &mut MockElectorWrapperImpl,
    election_id: u64,
    returned_stake: u64,
) {
    elector.expect_address().returning(|| Ok(elector_address()));

    elector.expect_get_active_election_id().returning(move || Ok(election_id));

    elector.expect_elections_info().returning(move || {
        Ok(ElectionsInfo {
            election_id,
            elect_close: election_id - 300,
            min_stake: MIN_STAKE,
            total_stake: 0,
            failed: false,
            finished: false,
            participants: vec![],
        })
    });

    elector.expect_past_elections().returning(|| Ok(vec![]));

    elector.expect_compute_returned_stake().returning(move |_addr| Ok(returned_stake));
}

fn setup_elector_no_elections(elector: &mut MockElectorWrapperImpl) {
    elector.expect_address().returning(|| Ok(elector_address()));
    elector.expect_get_active_election_id().returning(|| Ok(0));
}

fn setup_wallet(wallet: &mut MockTonWalletImpl) {
    wallet.expect_address().returning(|| Ok(wallet_address()));
    wallet.expect_message().returning(|_dest, _value, _payload| Ok(dummy_cell()));
}

/// Like `setup_default_provider` but without `expect_account` — caller sets up account mock separately.
fn setup_default_provider_without_account(
    provider: &mut MockElectionsProviderImpl,
    _wallet_balance: u64,
) {
    provider.expect_election_parameters().returning(|| Ok(default_cfg15()));
    provider.expect_validator_config().returning(|| Ok(ValidatorConfig::new()));
    provider.expect_get_current_vset().returning(|| Err(anyhow::anyhow!("no vset")));
    provider.expect_get_next_vset().returning(|| Ok(None));
    provider
        .expect_new_validator_key()
        .returning(|_since, _until| Ok((KEY_ID.to_vec(), PUB_KEY.to_vec())));
    provider.expect_new_adnl_addr().returning(|_key_id, _until| Ok(ADNL_ADDR.to_vec()));
    provider.expect_export_public_key().returning(|_key_id| Ok(PUB_KEY.to_vec()));
    provider.expect_sign().returning(|_key, _data| Ok(SIGNATURE.to_vec()));
    provider.expect_send_boc().returning(|_boc| Ok(()));
    provider.expect_config_param_16().returning(|| Ok(default_cfg16()));
    provider.expect_config_param_17().returning(|| Ok(default_cfg17()));
    provider.expect_shutdown().returning(|| Ok(()));
}

fn setup_pool(pool: &mut MockSingleNominatorWrapper) {
    pool.expect_address().returning(|| Ok(pool_address()));
    pool.expect_inner_pools().returning(|| vec![]);
    pool.expect_storage_reserve().returning(|| SNP_STORAGE_RESERVE);
    pool.expect_pool_kind().returning(|| PoolKind::SNP);
}

fn pool_data_with_state(state: i32) -> PoolData {
    PoolData { state, ..Default::default() }
}

fn setup_toncore_nominator_slot(
    pool: &mut MockSingleNominatorWrapper,
    addr: MsgAddressInt,
    state: i32,
) {
    pool.expect_address().returning(move || Ok(addr.clone()));
    pool.expect_get_pool_data().returning(move || Ok(pool_data_with_state(state)));
    pool.expect_storage_reserve().returning(|| TONCORE_STORAGE_RESERVE);
    pool.expect_pool_kind().returning(|| PoolKind::TONCore);
}

// =====================================================
// TEST: participate in elections (new key, no pool)
// =====================================================

#[tokio::test]
async fn test_participate_new_key_no_pool() {
    let node_id = "node-1";
    let mut harness = TestHarness::new();

    setup_default_elector(&mut harness.elector_mock, ELECTION_ID, 0);
    setup_default_provider(&mut harness.provider_mock, WALLET_BALANCE, None);
    setup_wallet(&mut harness.wallet_mock);
    let expected_stake =
        (WALLET_BALANCE - (ELECTOR_STAKE_FEE + NPOOL_COMPUTE_FEE) - WALLET_STORAGE_RESERVE) / 2;
    // validate the election bid payload
    harness
        .wallet_mock
        .expect_message()
        .withf(move |dest, value, payload| {
            validate_message_parameters(dest, value, payload, expected_stake)
        })
        .returning(|_dest, _value, _payload| Ok(dummy_cell()));

    let mut runner = harness.build(node_id).await;

    let result = runner.run().await;
    assert!(result.is_ok(), "run() failed: {:?}", result.err());

    let node = runner.nodes.get(node_id).unwrap();
    assert!(node.participant.is_some(), "participant should be set after participation");
    let participant = node.participant.as_ref().unwrap();
    assert_eq!(participant.pub_key, PUB_KEY.to_vec());
    assert_eq!(participant.adnl_addr, ADNL_ADDR.to_vec());
    assert_eq!(participant.election_id, ELECTION_ID);
    assert_eq!(participant.max_factor, 196608);
    assert!(participant.stake > 0, "stake should be positive");
    assert_eq!(participant.stake, expected_stake);
    assert!(participant.stake_message_boc.is_some(), "stake message boc should be set");
}

// =====================================================
// TEST: participate in elections with pool
// =====================================================

#[tokio::test]
async fn test_participate_new_key_with_pool() {
    let node_id = "node-1";
    let mut harness = TestHarness::new().with_pool();

    setup_default_elector(&mut harness.elector_mock, ELECTION_ID, 0);
    setup_default_provider(&mut harness.provider_mock, WALLET_BALANCE, Some(POOL_BALANCE));
    setup_wallet(&mut harness.wallet_mock);
    setup_pool(harness.pool_mock.as_mut().unwrap());

    let expected_stake = (POOL_BALANCE - SNP_STORAGE_RESERVE - EXTRA_STORAGE_FEES) / 2;
    // validate the election bid payload
    harness
        .wallet_mock
        .expect_message()
        .withf(move |dest, value, payload| {
            validate_message_parameters(dest, value, payload, expected_stake)
        })
        .returning(|_dest, _value, _payload| Ok(dummy_cell()));

    let mut runner = harness.build(node_id).await;

    let result = runner.run().await;
    assert!(result.is_ok(), "run() failed: {:?}", result.err());

    let node = runner.nodes.get(node_id).unwrap();
    assert!(node.participant.is_some());
    let participant = node.participant.as_ref().unwrap();
    assert!(participant.stake > 0);
    assert_eq!(participant.stake, expected_stake);
    // With pool, wallet_addr should be the pool address
    assert_eq!(participant.wallet_addr, addr_bytes(&pool_address()));
}

// =====================================================
// TEST: participate with existing key (re-stake)
// =====================================================

#[tokio::test]
async fn test_participate_existing_key_not_in_elector() {
    let node_id = "node-1";
    let mut harness = TestHarness::new();

    setup_default_provider(&mut harness.provider_mock, WALLET_BALANCE, Some(POOL_BALANCE));
    // Elector returns active elections, no returned stake
    setup_default_elector(&mut harness.elector_mock, ELECTION_ID, 0);
    setup_wallet(&mut harness.wallet_mock);

    // Return existing key in validator_config
    harness.provider_mock.expect_validator_config().returning(|| {
        let mut keys = HashMap::new();
        keys.insert(
            ELECTION_ID,
            ValidatorEntry {
                key_id: KEY_ID.to_vec(),
                public_key: vec![], // will be filled by export_public_key
                adnl_addrs: vec![(ADNL_ADDR.to_vec(), ELECTION_ID + 7200)],
                expired_at: ELECTION_ID + 7200,
            },
        );
        Ok(ValidatorConfig { keys })
    });

    let mut runner = harness.build(node_id).await;
    runner.refresh_validator_configs().await;
    let result = runner.run().await;
    assert!(result.is_ok(), "run() failed: {:?}", result.err());

    let node = runner.nodes.get(node_id).unwrap();
    assert!(node.participant.is_some());
    let p = node.participant.as_ref().unwrap();
    assert_eq!(p.pub_key, PUB_KEY.to_vec());
    assert_eq!(p.adnl_addr, ADNL_ADDR.to_vec());
}

// =====================================================
// TEST: stake already accepted by elector
// =====================================================

#[tokio::test]
async fn test_stake_already_accepted() {
    let node_id = "node-1";
    let mut harness = TestHarness::new();

    let wallet_addr = addr_bytes(&wallet_address());

    harness.elector_mock.expect_address().returning(|| Ok(elector_address()));
    harness.elector_mock.expect_get_active_election_id().returning(|| Ok(ELECTION_ID));

    let wallet_addr_clone = wallet_addr.clone();
    harness.elector_mock.expect_elections_info().returning(move || {
        Ok(ElectionsInfo {
            election_id: ELECTION_ID,
            elect_close: ELECTION_ID + 600,
            min_stake: MIN_STAKE,
            total_stake: MIN_STAKE,
            failed: false,
            finished: false,
            participants: vec![Participant {
                pub_key: PUB_KEY.to_vec(),
                adnl_addr: ADNL_ADDR.to_vec(),
                wallet_addr: wallet_addr_clone.clone(),
                stake: MIN_STAKE,
                max_factor: 196608,
                election_id: ELECTION_ID,
                stake_message_boc: None,
            }],
        })
    });
    harness.elector_mock.expect_past_elections().returning(|| Ok(vec![]));
    harness.elector_mock.expect_compute_returned_stake().returning(|_| Ok(0));

    setup_wallet(&mut harness.wallet_mock);

    let provider = &mut harness.provider_mock;
    provider.expect_election_parameters().returning(|| Ok(default_cfg15()));
    provider.expect_validator_config().returning(|| {
        let mut keys = HashMap::new();
        keys.insert(
            ELECTION_ID,
            ValidatorEntry {
                key_id: KEY_ID.to_vec(),
                public_key: vec![],
                adnl_addrs: vec![(ADNL_ADDR.to_vec(), ELECTION_ID + 7200)],
                expired_at: ELECTION_ID + 7200,
            },
        );
        Ok(ValidatorConfig { keys })
    });

    provider.expect_export_public_key().returning(|_| Ok(PUB_KEY.to_vec()));
    provider.expect_account().returning(|_| Ok(fake_account(WALLET_BALANCE)));
    provider.expect_config_param_16().returning(|| Ok(default_cfg16()));
    provider.expect_config_param_17().returning(|| Ok(default_cfg17()));
    provider.expect_shutdown().returning(|| Ok(()));

    let mut runner = harness.build(node_id).await;
    runner.refresh_validator_configs().await;
    let result = runner.run().await;
    assert!(result.is_ok(), "run() failed: {:?}", result.err());

    let node = runner.nodes.get(node_id).unwrap();
    assert!(node.stake_accepted, "stake should be accepted");
    assert!(node.participant.is_some());
    assert_eq!(node.participant.as_ref().unwrap().stake, MIN_STAKE);
}

// =====================================================
// TEST: recover stake
// =====================================================

#[tokio::test]
async fn test_recover_stake_returns_funds() {
    let node_id = "node-1";
    let mut harness = TestHarness::new();

    let returned_amount: u64 = 20_000_000_000_000; // 20 000 TON

    // Elector has active elections, but node has stake to recover
    setup_default_elector(&mut harness.elector_mock, ELECTION_ID, returned_amount);
    setup_wallet(&mut harness.wallet_mock);

    let provider = &mut harness.provider_mock;
    provider.expect_election_parameters().returning(|| Ok(default_cfg15()));
    provider.expect_validator_config().returning(|| Ok(ValidatorConfig::new()));

    provider.expect_account().returning(|_| Ok(fake_account(WALLET_BALANCE)));
    // Expect send_boc to be called for recover
    provider.expect_send_boc().times(1).returning(|_| Ok(()));
    provider.expect_config_param_16().returning(|| Ok(default_cfg16()));
    provider.expect_config_param_17().returning(|| Ok(default_cfg17()));
    provider.expect_shutdown().returning(|| Ok(()));

    let mut runner = harness.build(node_id).await;
    let result = runner.run().await;
    assert!(result.is_ok(), "run() failed: {:?}", result.err());

    // When recover_amount > 0, the node should NOT participate in elections
    let node = runner.nodes.get(node_id).unwrap();
    assert!(node.participant.is_none(), "should not participate when recovering stake");
}

// =====================================================
// TEST: recover stake — low wallet balance
// =====================================================

#[tokio::test]
async fn test_recover_stake_low_wallet_balance() {
    let node_id = "node-1";
    let mut harness = TestHarness::new();

    let returned_amount: u64 = 20_000_000_000_000;
    let low_wallet_balance: u64 = 100_000_000; // 0.1 TON — not enough for fees

    setup_default_elector(&mut harness.elector_mock, ELECTION_ID, returned_amount);
    setup_wallet(&mut harness.wallet_mock);

    let provider = &mut harness.provider_mock;
    provider.expect_election_parameters().returning(|| Ok(default_cfg15()));
    provider.expect_validator_config().returning(|| Ok(ValidatorConfig::new()));

    provider.expect_account().returning(move |_| Ok(fake_account(low_wallet_balance)));
    provider.expect_config_param_16().returning(|| Ok(default_cfg16()));
    provider.expect_config_param_17().returning(|| Ok(default_cfg17()));
    provider.expect_shutdown().returning(|| Ok(()));

    let mut runner = harness.build(node_id).await;
    let result = runner.run().await;
    // run() catches per-node errors and continues; it itself returns Ok
    assert!(result.is_ok());

    let node = runner.nodes.get(node_id).unwrap();
    assert!(node.last_error.is_some(), "should have an error on low balance");
}

// =====================================================
// TEST: no active elections
// =====================================================

#[tokio::test]
async fn test_no_active_elections() {
    let node_id = "node-1";
    let mut harness = TestHarness::new();

    setup_elector_no_elections(&mut harness.elector_mock);
    setup_wallet(&mut harness.wallet_mock);

    let provider = &mut harness.provider_mock;
    provider.expect_election_parameters().returning(|| Ok(default_cfg15()));
    provider.expect_validator_config().returning(|| Ok(ValidatorConfig::new()));

    provider.expect_shutdown().returning(|| Ok(()));

    let mut runner = harness.build(node_id).await;
    let result = runner.run().await;
    assert!(result.is_ok());

    assert_eq!(runner.snapshot_cache.last_elections_status, ElectionsStatus::Closed);
}

#[tokio::test]
async fn test_closed_elections_without_submission_stays_not_submitted() {
    let node_id = "node-1";
    let mut harness = TestHarness::new();

    setup_elector_no_elections(&mut harness.elector_mock);
    setup_wallet(&mut harness.wallet_mock);

    let provider = &mut harness.provider_mock;
    provider.expect_election_parameters().returning(|| Ok(default_cfg15()));
    provider.expect_shutdown().returning(|| Ok(()));

    let mut runner = harness.build(node_id).await;
    let result = runner.run().await;
    assert!(result.is_ok());
    assert_eq!(runner.snapshot_cache.last_elections_status, ElectionsStatus::Closed);

    // Simulate stale previous elections snapshot still present in cache.
    runner.snapshot_cache.last_elections = Some(ElectionsSnapshot {
        election_id: ELECTION_ID,
        elections_range: TimeRange {
            start: ELECTION_ID.saturating_sub(1800),
            start_utc: time_format::format_ts(ELECTION_ID.saturating_sub(1800)),
            end: ELECTION_ID.saturating_sub(600),
            end_utc: time_format::format_ts(ELECTION_ID.saturating_sub(600)),
        },
        ..Default::default()
    });

    let store = Arc::new(SnapshotStore::new());
    runner.publish_snapshot(&store).await;
    let snapshot = store.get();
    let node_snapshot = &snapshot.validators.controlled_nodes[0];

    assert!(!node_snapshot.is_validator);
    assert!(node_snapshot.validator_index.is_none());
}

// =====================================================
// TEST: excluded node skips elections
// =====================================================

#[tokio::test]
async fn test_excluded_node_skips_elections() {
    let node_id = "node-1";
    let mut harness = TestHarness::new();
    harness.bindings.insert(node_id.to_string(), default_binding(false));
    setup_default_elector(&mut harness.elector_mock, ELECTION_ID, 0);
    setup_wallet(&mut harness.wallet_mock);

    let provider = &mut harness.provider_mock;
    provider.expect_election_parameters().returning(|| Ok(default_cfg15()));
    provider.expect_validator_config().returning(|| Ok(ValidatorConfig::new()));

    provider.expect_export_public_key().returning(|_| Ok(PUB_KEY.to_vec()));
    provider.expect_account().returning(|_| Ok(fake_account(WALLET_BALANCE)));
    provider.expect_config_param_16().returning(|| Ok(default_cfg16()));
    provider.expect_config_param_17().returning(|| Ok(default_cfg17()));
    provider.expect_shutdown().returning(|| Ok(()));

    let mut runner = harness.build(node_id).await;
    let result = runner.run().await;
    assert!(result.is_ok());

    let node = runner.nodes.get(node_id).unwrap();
    assert!(node.participant.is_none(), "excluded node should not participate");
}

// =====================================================
// TEST: elections finished — stake_accepted set
// =====================================================

#[tokio::test]
async fn test_elections_finished_stake_accepted() {
    let node_id = "node-1";
    let mut harness = TestHarness::new();
    harness.elections_config.policy = StakePolicy::Minimum;

    let wallet_addr_clone = addr_bytes(&wallet_address());
    harness.elector_mock.expect_elections_info().returning(move || {
        Ok(ElectionsInfo {
            election_id: ELECTION_ID,
            elect_close: ELECTION_ID + 3600,
            min_stake: MIN_STAKE,
            total_stake: 0,
            failed: true,
            finished: true,
            participants: vec![Participant {
                pub_key: PUB_KEY.to_vec(),
                adnl_addr: ADNL_ADDR.to_vec(),
                wallet_addr: wallet_addr_clone.clone(),
                stake: MIN_STAKE,
                max_factor: 196608,
                election_id: ELECTION_ID,
                stake_message_boc: None,
            }],
        })
    });

    harness.provider_mock.expect_send_boc().never();

    setup_default_elector(&mut harness.elector_mock, ELECTION_ID, 0);
    setup_default_provider(&mut harness.provider_mock, WALLET_BALANCE, Some(POOL_BALANCE));

    setup_wallet(&mut harness.wallet_mock);
    let mut runner = harness.build(node_id).await;
    let result = runner.run().await;
    assert!(result.is_ok());

    assert_eq!(runner.snapshot_cache.last_elections_status, ElectionsStatus::Finished);
    let node = runner.nodes.get(node_id).unwrap();
    assert!(node.stake_accepted);
}

// =====================================================
// TEST: run_loop cancellation
// =====================================================

#[tokio::test]
async fn test_run_loop_cancellation() {
    let node_id = "node-1";
    let mut harness = TestHarness::new();

    setup_default_provider(&mut harness.provider_mock, WALLET_BALANCE, Some(POOL_BALANCE));
    setup_elector_no_elections(&mut harness.elector_mock);
    setup_wallet(&mut harness.wallet_mock);

    let mut runner = harness.build(node_id).await;

    let mut ctx = CancellationCtx::new();
    let cancel_ctx = ctx.clone();
    let store = Arc::new(SnapshotStore::new());

    // Send cancel signal after short delay
    let cancel_handle = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        ctx.cancel(CancellationReason::GracefullyShutdown());
    });

    let result = runner.run_loop(Duration::from_secs(600), cancel_ctx, store, None).await;
    assert!(result.is_ok(), "run_loop should return Ok on cancel");

    tokio::select! {
        _ = cancel_handle => {}
        _ = tokio::time::sleep(Duration::from_secs(60)) => {
            panic!("run_loop should be cancelled by the cancel signal");
        }
    }
}

// =====================================================
// TEST: run_loop executes tick then cancels
// =====================================================

#[tokio::test]
async fn test_run_loop_tick_then_cancel() {
    let node_id = "node-1";
    let mut harness = TestHarness::new();

    // First call: active elections. Second call onwards: no elections.
    let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let cc = call_count.clone();

    harness.elector_mock.expect_address().returning(|| Ok(elector_address()));
    harness.elector_mock.expect_get_active_election_id().returning(move || {
        let n = cc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n == 0 { Ok(ELECTION_ID) } else { Ok(0) }
    });
    harness.elector_mock.expect_elections_info().returning(move || {
        Ok(ElectionsInfo {
            election_id: ELECTION_ID,
            elect_close: ELECTION_ID + 600,
            min_stake: MIN_STAKE,
            total_stake: 0,
            failed: false,
            finished: false,
            participants: vec![],
        })
    });
    harness.elector_mock.expect_past_elections().returning(|| Ok(vec![]));
    harness.elector_mock.expect_compute_returned_stake().returning(|_| Ok(0));

    setup_wallet(&mut harness.wallet_mock);
    setup_default_provider(&mut harness.provider_mock, WALLET_BALANCE, None);

    let mut runner = harness.build(node_id).await;
    let mut ctx = CancellationCtx::new();
    let cancel_ctx = ctx.clone();
    let store = Arc::new(SnapshotStore::new());

    // Cancel after one tick
    let cancel_handle = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        ctx.cancel(CancellationReason::GracefullyShutdown());
    });

    let result = runner.run_loop(Duration::from_millis(50), cancel_ctx, store.clone(), None).await;
    assert!(result.is_ok());

    cancel_handle.await.unwrap();

    // Snapshot should have been published
    let snapshot = store.get();
    // At least one tick happened
    assert!(snapshot.generated_at > 0);
}

// =====================================================
// TEST: fixed stake policy
// =====================================================

#[tokio::test]
async fn test_fixed_stake_policy() {
    let node_id = "node-1";
    let mut harness = TestHarness::new();
    let fixed_stake: u64 = 15_000_000_000_000; // 15 000 TON
    harness.elections_config.policy = StakePolicy::Fixed(fixed_stake);

    setup_default_elector(&mut harness.elector_mock, ELECTION_ID, 0);
    setup_default_provider(&mut harness.provider_mock, WALLET_BALANCE, None);
    setup_wallet(&mut harness.wallet_mock);

    let mut runner = harness.build(node_id).await;
    let result = runner.run().await;
    assert!(result.is_ok(), "run() failed: {:?}", result.err());

    let node = runner.nodes.get(node_id).unwrap();
    assert!(node.participant.is_some());
    assert_eq!(node.participant.as_ref().unwrap().stake, fixed_stake);
}

// =====================================================
// TEST: minimum stake policy
// =====================================================

#[tokio::test]
async fn test_minimum_stake_policy() {
    let node_id = "node-1";
    let mut harness = TestHarness::new();
    harness.elections_config.policy = StakePolicy::Minimum;

    setup_default_elector(&mut harness.elector_mock, ELECTION_ID, 0);
    setup_default_provider(&mut harness.provider_mock, WALLET_BALANCE, None);
    setup_wallet(&mut harness.wallet_mock);

    let mut runner = harness.build(node_id).await;
    let result = runner.run().await;
    assert!(result.is_ok(), "run() failed: {:?}", result.err());

    let node = runner.nodes.get(node_id).unwrap();
    assert!(node.participant.is_some());
    assert_eq!(node.participant.as_ref().unwrap().stake, MIN_STAKE);
}

// =====================================================
// TEST: recover stake with frozen funds from past elections
// =====================================================

#[tokio::test]
async fn test_recover_with_past_elections_frozen() {
    let node_id = "node-1";
    let mut harness = TestHarness::new();
    let past_election_id = ELECTION_ID - 3600;
    let frozen_stake: u64 = 10_000_000_000_000;

    setup_default_elector(&mut harness.elector_mock, ELECTION_ID, 0);
    setup_default_provider(&mut harness.provider_mock, WALLET_BALANCE, Some(POOL_BALANCE));

    // Past elections with frozen stake
    harness.elector_mock.expect_past_elections().returning(move || {
        let mut frozen_map = HashMap::new();
        frozen_map.insert(
            PUB_KEY,
            FrozenParticipant {
                wallet_addr: WALLET_ADDR,
                weight: 123456789123456789,
                stake: frozen_stake,
                banned: false,
            },
        );
        Ok(vec![PastElections {
            election_id: past_election_id,
            unfreeze_at: ELECTION_ID + 3600,
            stake_held: 7200,
            vset_hash: vec![0u8; 32],
            frozen_map,
            total_stake: frozen_stake,
            bonuses: 0,
        }])
    });

    setup_wallet(&mut harness.wallet_mock);

    // Return key for past election as well as current
    harness.provider_mock.expect_validator_config().returning(move || {
        let mut keys = HashMap::new();
        keys.insert(
            past_election_id,
            ValidatorEntry {
                key_id: KEY_ID.to_vec(),
                public_key: vec![],
                adnl_addrs: vec![(ADNL_ADDR.to_vec(), past_election_id + 7200)],
                expired_at: past_election_id + 7200,
            },
        );
        Ok(ValidatorConfig { keys })
    });

    let mut runner = harness.build(node_id).await;
    let result = runner.run().await;
    assert!(result.is_ok(), "run() failed: {:?}", result.err());

    // Node should participate; the frozen stake should be factored into calc_stake
    let node = runner.nodes.get(node_id).unwrap();
    assert!(node.participant.is_some());
    let p = node.participant.as_ref().unwrap();
    // With Split50 policy, stake = max(total_balance / 2, min_stake)
    // total_balance = frozen_stake + pool_free_balance + 0
    // pool_free_balance = WALLET_BALANCE - gas_fee - SNP_STORAGE_RESERVE
    assert!(p.stake >= MIN_STAKE, "stake should be at least min_stake");
}

// =====================================================
// TEST: low stake balance prevents participation
// =====================================================

#[tokio::test]
async fn test_low_stake_balance() {
    let node_id = "node-1";
    let mut harness = TestHarness::new();
    let low_balance: u64 = 5_000_000_000; // 5 TON — much less than min stake

    setup_default_elector(&mut harness.elector_mock, ELECTION_ID, 0);
    setup_wallet(&mut harness.wallet_mock);

    let provider = &mut harness.provider_mock;
    provider.expect_election_parameters().returning(|| Ok(default_cfg15()));
    provider.expect_validator_config().returning(|| Ok(ValidatorConfig::new()));

    provider.expect_export_public_key().returning(|_| Ok(PUB_KEY.to_vec()));
    provider.expect_account().returning(move |_| Ok(fake_account(low_balance)));
    provider.expect_new_validator_key().returning(|_, _| Ok((KEY_ID.to_vec(), PUB_KEY.to_vec())));
    provider.expect_new_adnl_addr().returning(|_, _| Ok(ADNL_ADDR.to_vec()));
    provider.expect_config_param_16().returning(|| Ok(default_cfg16()));
    provider.expect_config_param_17().returning(|| Ok(default_cfg17()));

    provider.expect_shutdown().returning(|| Ok(()));

    let mut runner = harness.build(node_id).await;
    let result = runner.run().await;
    // run() itself is Ok, but the node should have an error
    assert!(result.is_ok());

    let node = runner.nodes.get(node_id).unwrap();
    assert!(node.last_error.is_some(), "should have an error for low balance");
    let err = node.last_error.as_ref().unwrap();
    assert!(
        err.contains("not enough") || err.contains("low stake"),
        "error should mention insufficient balance, got: {}",
        err
    );
}

// =====================================================
// TEST: shutdown calls provider shutdown
// =====================================================

#[tokio::test]
async fn test_shutdown() {
    let node_id = "node-1";
    let mut harness = TestHarness::new();

    setup_elector_no_elections(&mut harness.elector_mock);
    setup_wallet(&mut harness.wallet_mock);

    let provider = &mut harness.provider_mock;
    provider.expect_shutdown().times(1).returning(|| Ok(()));
    provider.expect_election_parameters().returning(|| Ok(default_cfg15()));
    provider.expect_validator_config().returning(|| Ok(ValidatorConfig::new()));

    let mut runner = harness.build(node_id).await;
    let result = runner.shutdown().await;
    assert!(result.is_ok());
}

// =====================================================
// TEST: multiple nodes
// =====================================================

#[tokio::test]
async fn test_multiple_nodes_one_excluded() {
    // Build runner manually with two nodes
    let mut elector_mock = MockElectorWrapperImpl::new();
    setup_default_elector(&mut elector_mock, ELECTION_ID, 0);

    let elector: Arc<dyn ElectorWrapper> = Arc::new(elector_mock);

    let elections_config = ElectionsConfig {
        policy: StakePolicy::Minimum,
        policy_overrides: HashMap::new(),
        max_factor: 3.0,
        tick_interval: 10,
        sleep_period_pct: 0.0,
        waiting_period_pct: 0.3,
        static_adnls: HashMap::new(),
    };

    let mut bindings = HashMap::new();
    bindings.insert(
        "node-1".to_string(),
        NodeBinding {
            wallet: "w1".to_string(),
            pool: None,
            enable: true,
            status: Default::default(),
        },
    );
    bindings.insert(
        "node-2".to_string(),
        NodeBinding {
            wallet: "w2".to_string(),
            pool: None,
            enable: false,
            status: Default::default(),
        },
    );

    let mut wallet1 = MockTonWalletImpl::new();
    wallet1.expect_address().returning(|| Ok(wallet_address()));
    wallet1.expect_message().returning(|_, _, _| Ok(dummy_cell()));

    let mut wallet2 = MockTonWalletImpl::new();
    wallet2.expect_address().returning(|| Ok(MsgAddressInt::standard(-1, [0xCCu8; 32])));
    wallet2.expect_message().returning(|_, _, _| Ok(dummy_cell()));

    let mut wallets: HashMap<String, Arc<dyn TonWallet>> = HashMap::new();
    wallets.insert("node-1".to_string(), Arc::new(wallet1));
    wallets.insert("node-2".to_string(), Arc::new(wallet2));

    let mut provider1 = MockElectionsProviderImpl::new();
    setup_default_provider(&mut provider1, WALLET_BALANCE, None);

    let mut provider2 = MockElectionsProviderImpl::new();
    provider2.expect_election_parameters().returning(|| Ok(default_cfg15()));
    provider2.expect_validator_config().returning(|| Ok(ValidatorConfig::new()));
    provider2.expect_get_current_vset().returning(|| Err(anyhow::anyhow!("no vset")));
    provider2.expect_get_next_vset().returning(|| Ok(None));
    provider2.expect_export_public_key().returning(|_| Ok(PUB_KEY.to_vec()));
    provider2.expect_account().returning(|_| Ok(fake_account(WALLET_BALANCE)));
    provider2.expect_config_param_16().returning(|| Ok(default_cfg16()));
    provider2.expect_config_param_17().returning(|| Ok(default_cfg17()));
    provider2.expect_shutdown().returning(|| Ok(()));

    let mut providers: HashMap<String, Box<dyn ElectionsProvider>> = HashMap::new();
    providers.insert("node-1".to_string(), Box::new(provider1));
    providers.insert("node-2".to_string(), Box::new(provider2));

    let pools = HashMap::new();

    let mut runner = ElectionRunner::new(
        &elections_config,
        &bindings,
        elector,
        providers,
        Arc::new(wallets),
        Arc::new(pools),
    );

    let result = runner.run().await;
    assert!(result.is_ok());

    // node-1 should participate
    let node1 = runner.nodes.get("node-1").unwrap();
    assert!(node1.participant.is_some(), "node-1 should participate");

    // node-2 is excluded — should not participate
    let node2 = runner.nodes.get("node-2").unwrap();
    assert!(node2.participant.is_none(), "node-2 (excluded) should not participate");
}

// =====================================================
// TEST: elections failed status
// =====================================================

#[tokio::test]
async fn test_elections_failed_still_processes_nodes() {
    let node_id = "node-1";
    let mut harness = TestHarness::new();

    harness.elector_mock.expect_address().returning(|| Ok(elector_address()));
    harness.elector_mock.expect_get_active_election_id().returning(|| Ok(ELECTION_ID));

    harness.elector_mock.expect_elections_info().returning(move || {
        Ok(ElectionsInfo {
            election_id: ELECTION_ID,
            elect_close: ELECTION_ID + 600,
            min_stake: MIN_STAKE,
            total_stake: 0,
            failed: true,
            finished: false,
            participants: vec![],
        })
    });
    harness.elector_mock.expect_past_elections().returning(|| Ok(vec![]));
    harness.elector_mock.expect_compute_returned_stake().returning(|_| Ok(0));

    setup_wallet(&mut harness.wallet_mock);
    setup_default_provider(&mut harness.provider_mock, WALLET_BALANCE, None);

    let mut runner = harness.build(node_id).await;
    let result = runner.run().await;
    assert!(result.is_ok(), "run() failed: {:?}", result.err());

    // Unlike finished elections, failed elections don't cause early return —
    // the runner still processes nodes and tries to participate.
    let node = runner.nodes.get(node_id).unwrap();
    assert!(
        node.participant.is_some(),
        "node should still participate despite elections being marked as failed"
    );

    // The elections snapshot should be built
    assert!(runner.snapshot_cache.last_elections.is_some());
    let snapshot = runner.snapshot_cache.last_elections.as_ref().unwrap();
    assert!(snapshot.failed, "snapshot should reflect failed=true");
}

// =====================================================
// TEST: elections finished but node NOT in participants
// =====================================================

#[tokio::test]
async fn test_elections_finished_node_not_in_participants() {
    let node_id = "node-1";
    let mut harness = TestHarness::new();

    harness.elector_mock.expect_address().returning(|| Ok(elector_address()));
    harness.elector_mock.expect_get_active_election_id().returning(|| Ok(ELECTION_ID));

    // Finished elections with a different participant
    harness.elector_mock.expect_elections_info().returning(move || {
        Ok(ElectionsInfo {
            election_id: ELECTION_ID,
            elect_close: ELECTION_ID + 600,
            min_stake: MIN_STAKE,
            total_stake: MIN_STAKE,
            failed: false,
            finished: true,
            participants: vec![Participant {
                pub_key: [0xFFu8; 32].to_vec(),
                adnl_addr: [0xFFu8; 32].to_vec(),
                wallet_addr: [0xFFu8; 32].to_vec(),
                stake: MIN_STAKE,
                max_factor: 196608,
                election_id: ELECTION_ID,
                stake_message_boc: None,
            }],
        })
    });

    setup_wallet(&mut harness.wallet_mock);

    let provider = &mut harness.provider_mock;
    provider.expect_election_parameters().returning(|| Ok(default_cfg15()));
    provider.expect_validator_config().returning(|| Ok(ValidatorConfig::new()));
    provider.expect_config_param_16().returning(|| Ok(default_cfg16()));
    provider.expect_config_param_17().returning(|| Ok(default_cfg17()));

    provider.expect_shutdown().returning(|| Ok(()));

    let mut runner = harness.build(node_id).await;
    let result = runner.run().await;
    assert!(result.is_ok());

    assert_eq!(runner.snapshot_cache.last_elections_status, ElectionsStatus::Finished);
    let node = runner.nodes.get(node_id).unwrap();
    assert!(!node.stake_accepted, "node should NOT have stake accepted");
}

// =====================================================
// TEST: election parameters — all nodes fail
// =====================================================

#[tokio::test]
async fn test_election_parameters_all_nodes_fail() {
    let node_id = "node-1";
    let mut harness = TestHarness::new();

    setup_elector_no_elections(&mut harness.elector_mock);
    setup_wallet(&mut harness.wallet_mock);

    let provider = &mut harness.provider_mock;
    provider.expect_election_parameters().returning(|| Err(anyhow::anyhow!("cfg15 unavailable")));
    provider.expect_validator_config().returning(|| Ok(ValidatorConfig::new()));

    provider.expect_shutdown().returning(|| Ok(()));

    let mut runner = harness.build(node_id).await;
    let result = runner.run().await;
    assert!(result.is_err(), "run() should fail when all nodes fail to return cfg15");
    let err = result.unwrap_err();
    assert!(
        format!("{:#}", err).contains("election parameters"),
        "error should mention election parameters, got: {:#}",
        err
    );
}

// =====================================================
// TEST: new_validator_key failure
// =====================================================

#[tokio::test]
async fn test_new_validator_key_failure() {
    let node_id = "node-1";
    let mut harness = TestHarness::new();

    setup_default_elector(&mut harness.elector_mock, ELECTION_ID, 0);
    setup_wallet(&mut harness.wallet_mock);

    let provider = &mut harness.provider_mock;
    provider.expect_election_parameters().returning(|| Ok(default_cfg15()));
    provider.expect_validator_config().returning(|| Ok(ValidatorConfig::new()));

    provider.expect_export_public_key().returning(|_| Ok(PUB_KEY.to_vec()));
    provider.expect_account().returning(|_| Ok(fake_account(WALLET_BALANCE)));
    provider.expect_new_validator_key().returning(|_, _| Err(anyhow::anyhow!("keygen failed")));
    provider.expect_config_param_16().returning(|| Ok(default_cfg16()));
    provider.expect_config_param_17().returning(|| Ok(default_cfg17()));
    provider.expect_shutdown().returning(|| Ok(()));

    let mut runner = harness.build(node_id).await;
    let result = runner.run().await;
    assert!(result.is_ok(), "run() should still be Ok, per-node error captured");

    let node = runner.nodes.get(node_id).unwrap();
    assert!(node.last_error.is_some(), "should record key generation error");
    let err = node.last_error.as_ref().unwrap();
    assert!(
        err.contains("keygen failed") || err.contains("validator key"),
        "error should mention key generation, got: {}",
        err
    );
}

// =====================================================
// TEST: send_boc failure
// =====================================================

#[tokio::test]
async fn test_send_boc_failure() {
    let node_id = "node-1";
    let mut harness = TestHarness::new();

    setup_default_elector(&mut harness.elector_mock, ELECTION_ID, 0);
    setup_wallet(&mut harness.wallet_mock);

    let provider = &mut harness.provider_mock;
    provider.expect_election_parameters().returning(|| Ok(default_cfg15()));
    provider.expect_validator_config().returning(|| Ok(ValidatorConfig::new()));

    provider.expect_export_public_key().returning(|_| Ok(PUB_KEY.to_vec()));
    provider.expect_account().returning(|_| Ok(fake_account(WALLET_BALANCE)));
    provider.expect_new_validator_key().returning(|_, _| Ok((KEY_ID.to_vec(), PUB_KEY.to_vec())));
    provider.expect_new_adnl_addr().returning(|_, _| Ok(ADNL_ADDR.to_vec()));

    provider.expect_send_boc().returning(|_| Err(anyhow::anyhow!("broadcast failed")));
    provider.expect_config_param_16().returning(|| Ok(default_cfg16()));
    provider.expect_config_param_17().returning(|| Ok(default_cfg17()));
    provider.expect_shutdown().returning(|| Ok(()));
    provider.expect_sign().returning(|_key, _data| Ok(SIGNATURE.to_vec()));

    let mut runner = harness.build(node_id).await;
    let result = runner.run().await;
    assert!(result.is_ok(), "run() should be Ok, per-node error captured");

    let node = runner.nodes.get(node_id).unwrap();
    assert!(node.last_error.is_some(), "should record send_boc error");
    let err = node.last_error.as_ref().unwrap();
    assert!(
        err.contains("broadcast failed"),
        "error should mention broadcast failure, got: {}",
        err
    );
}

// =====================================================
// TEST: Split50 stake calculation
// =====================================================

#[tokio::test]
async fn test_split50_stake_calculation() {
    let node_id = "node-1";
    let mut harness = TestHarness::new();
    harness.elections_config.policy = StakePolicy::Split50;

    setup_default_elector(&mut harness.elector_mock, ELECTION_ID, 0);
    setup_default_provider(&mut harness.provider_mock, WALLET_BALANCE, None);
    setup_wallet(&mut harness.wallet_mock);

    let mut runner = harness.build(node_id).await;
    let result = runner.run().await;
    assert!(result.is_ok(), "run() failed: {:?}", result.err());

    let node = runner.nodes.get(node_id).unwrap();
    assert!(node.participant.is_some());
    let p = node.participant.as_ref().unwrap();

    let gas_fee = ELECTOR_STAKE_FEE + NPOOL_COMPUTE_FEE;
    let pool_free_balance = WALLET_BALANCE - gas_fee - WALLET_STORAGE_RESERVE;
    let expected = (pool_free_balance / 2).max(MIN_STAKE);
    assert_eq!(
        p.stake, expected,
        "Split50 stake mismatch: expected={}, actual={}",
        expected, p.stake
    );
}

// =====================================================
// TEST: build_elections_snapshot
// =====================================================

#[tokio::test]
async fn test_build_elections_snapshot() {
    let node_id = "node-1";
    let mut harness = TestHarness::new();

    let wallet_addr = addr_bytes(&wallet_address());

    harness.elector_mock.expect_address().returning(|| Ok(elector_address()));
    harness.elector_mock.expect_get_active_election_id().returning(|| Ok(ELECTION_ID));

    let wallet_addr_clone = wallet_addr.clone();
    harness.elector_mock.expect_elections_info().returning(move || {
        Ok(ElectionsInfo {
            election_id: ELECTION_ID,
            elect_close: ELECTION_ID + 600,
            min_stake: MIN_STAKE,
            total_stake: MIN_STAKE * 3,
            failed: false,
            finished: false,
            participants: vec![
                Participant {
                    pub_key: PUB_KEY.to_vec(),
                    adnl_addr: ADNL_ADDR.to_vec(),
                    wallet_addr: wallet_addr_clone.clone(),
                    stake: MIN_STAKE,
                    max_factor: 196608,
                    election_id: ELECTION_ID,
                    stake_message_boc: None,
                },
                Participant {
                    pub_key: [0xDD; 32].to_vec(),
                    adnl_addr: [0xEE; 32].to_vec(),
                    wallet_addr: [0xFF; 32].to_vec(),
                    stake: MIN_STAKE * 2,
                    max_factor: 196608,
                    election_id: ELECTION_ID,
                    stake_message_boc: None,
                },
            ],
        })
    });
    harness.elector_mock.expect_past_elections().returning(|| Ok(vec![]));
    harness.elector_mock.expect_compute_returned_stake().returning(|_| Ok(0));

    setup_wallet(&mut harness.wallet_mock);
    setup_default_provider(&mut harness.provider_mock, WALLET_BALANCE, None);

    let mut runner = harness.build(node_id).await;
    let result = runner.run().await;
    assert!(result.is_ok(), "run() failed: {:?}", result.err());

    let snapshot = runner.snapshot_cache.last_elections.as_ref().expect("snapshot should exist");
    assert_eq!(snapshot.election_id, ELECTION_ID);
    assert_eq!(snapshot.elect_close, ELECTION_ID + 600);
    assert!(!snapshot.finished);
    assert!(!snapshot.failed);
    assert_eq!(snapshot.participants_count, 2);
    assert_eq!(snapshot.min_stake, nanotons_to_dec_string(MIN_STAKE));
    assert_eq!(snapshot.participant_min_stake, Some(nanotons_to_dec_string(MIN_STAKE)));
    assert_eq!(snapshot.participant_max_stake, Some(nanotons_to_dec_string(MIN_STAKE * 2)));
    assert_eq!(snapshot.total_stake, nanotons_to_dec_string(MIN_STAKE * 3));

    // Validation range
    assert_eq!(snapshot.next_validation_range.start, ELECTION_ID);
    assert_eq!(
        snapshot.next_validation_range.end,
        ELECTION_ID + default_cfg15().validators_elected_for as u64
    );

    // Elections range
    assert_eq!(
        snapshot.elections_range.start,
        ELECTION_ID - default_cfg15().elections_start_before as u64
    );
    assert_eq!(
        snapshot.elections_range.end,
        ELECTION_ID - default_cfg15().elections_end_before as u64
    );

    // Check participants snapshot
    assert_eq!(snapshot.participants.len(), 2);
    let our_participant = snapshot
        .participants
        .iter()
        .find(|p| p.is_controlled)
        .expect("should have controlled participant");
    assert_eq!(
        our_participant.pubkey,
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &PUB_KEY)
    );
    assert_eq!(our_participant.stake, nanotons_to_dec_string(MIN_STAKE));

    let other_participant = snapshot
        .participants
        .iter()
        .find(|p| !p.is_controlled)
        .expect("should have non-controlled participant");
    assert_eq!(
        other_participant.pubkey,
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [0xDD; 32])
    );
}

// =====================================================
// TEST: node without wallet is filtered out
// =====================================================

#[tokio::test]
async fn test_node_without_wallet_skipped() {
    let mut elector_mock = MockElectorWrapperImpl::new();
    setup_elector_no_elections(&mut elector_mock);
    let elector: Arc<dyn ElectorWrapper> = Arc::new(elector_mock);

    let elections_config = ElectionsConfig {
        policy: StakePolicy::Minimum,
        policy_overrides: HashMap::new(),
        max_factor: 3.0,
        tick_interval: 10,
        sleep_period_pct: 0.0,
        waiting_period_pct: 0.3,
        static_adnls: HashMap::new(),
    };

    let mut bindings = HashMap::new();
    bindings.insert("node-1".to_string(), default_binding(true));

    // Provider for "node-1" exists, but no wallet for it
    let mut provider1 = MockElectionsProviderImpl::new();
    provider1.expect_shutdown().returning(|| Ok(()));

    let mut providers: HashMap<String, Box<dyn ElectionsProvider>> = HashMap::new();
    providers.insert("node-1".to_string(), Box::new(provider1));

    let wallets: HashMap<String, Arc<dyn TonWallet>> = HashMap::new(); // empty!
    let pools: HashMap<String, Arc<dyn NominatorWrapper>> = HashMap::new();

    let runner = ElectionRunner::new(
        &elections_config,
        &bindings,
        elector,
        providers,
        Arc::new(wallets),
        Arc::new(pools),
    );

    assert!(
        runner.nodes.is_empty(),
        "node without wallet should be filtered out, got {} nodes",
        runner.nodes.len()
    );
}

// =====================================================
// TEST: second run() — key exists and stake in elector
// =====================================================

#[tokio::test]
async fn test_second_tick_stake_already_sent() {
    let node_id = "node-1";
    let mut harness = TestHarness::new();
    let wallet_addr = addr_bytes(&wallet_address());

    // First call: no participants. Second call: node is in participants.
    let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let cc = call_count.clone();
    let wallet_addr_clone = wallet_addr.clone();
    harness.elector_mock.expect_elections_info().times(2).returning(move || {
        let n = cc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n == 0 {
            Ok(ElectionsInfo {
                election_id: ELECTION_ID,
                elect_close: u64::MAX,
                min_stake: MIN_STAKE,
                total_stake: 0,
                failed: false,
                finished: false,
                participants: vec![],
            })
        } else {
            Ok(ElectionsInfo {
                election_id: ELECTION_ID,
                elect_close: u64::MAX,
                min_stake: MIN_STAKE,
                total_stake: MIN_STAKE,
                failed: false,
                finished: false,
                participants: vec![Participant {
                    pub_key: PUB_KEY.to_vec(),
                    adnl_addr: ADNL_ADDR.to_vec(),
                    wallet_addr: wallet_addr_clone.clone(),
                    stake: MIN_STAKE,
                    max_factor: 196608,
                    election_id: ELECTION_ID,
                    stake_message_boc: None,
                }],
            })
        }
    });

    setup_wallet(&mut harness.wallet_mock);

    // Provider: first validator_config call returns empty (new key path),
    // second returns the generated key (re-stake path).
    let vc_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let vcc = vc_count.clone();
    let provider = &mut harness.provider_mock;
    provider.expect_validator_config().times(2).returning(move || {
        let n = vcc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n == 0 {
            Ok(ValidatorConfig::new())
        } else {
            let mut keys = HashMap::new();
            keys.insert(
                ELECTION_ID,
                ValidatorEntry {
                    key_id: KEY_ID.to_vec(),
                    public_key: vec![],
                    adnl_addrs: vec![(ADNL_ADDR.to_vec(), ELECTION_ID + 7200)],
                    expired_at: ELECTION_ID + 7200,
                },
            );
            Ok(ValidatorConfig { keys })
        }
    });

    setup_default_elector(&mut harness.elector_mock, ELECTION_ID, 0);
    setup_default_provider(&mut harness.provider_mock, WALLET_BALANCE, Some(POOL_BALANCE));

    let mut runner = harness.build(node_id).await;

    // First tick: no key → generates new key, sends stake
    runner.refresh_validator_configs().await;
    runner.refresh_validator_set().await;
    let r1 = runner.run().await;
    assert!(r1.is_ok(), "first run() failed: {:?}", r1.err());
    let node = runner.nodes.get(node_id).unwrap();
    assert!(node.participant.is_some(), "should participate after first tick");
    assert!(!node.stake_accepted, "stake not yet accepted by elector");
    assert_eq!(runner.snapshot_cache.last_elections_status, ElectionsStatus::Active);

    // Second tick: validator_config now returns key, elector has participant
    runner.refresh_validator_configs().await;
    runner.refresh_validator_set().await;
    let r2 = runner.run().await;
    assert!(r2.is_ok(), "second run() failed: {:?}", r2.err());
    let node = runner.nodes.get(node_id).unwrap();
    assert!(node.participant.is_some());
    assert!(node.stake_accepted, "stake should be accepted on second tick");
}

// =====================================================
// TEST: publish_snapshot validators
// =====================================================

#[tokio::test]
async fn test_publish_snapshot_validators() {
    let node_id = "node-1";
    let mut harness = TestHarness::new();

    setup_default_elector(&mut harness.elector_mock, ELECTION_ID, 0);
    setup_default_provider(&mut harness.provider_mock, WALLET_BALANCE, None);
    setup_wallet(&mut harness.wallet_mock);

    let mut runner = harness.build(node_id).await;
    let _ = runner.run().await;

    let store = Arc::new(SnapshotStore::new());
    runner.publish_snapshot(&store).await;

    let snapshot = store.get();
    assert!(snapshot.generated_at > 0);

    let validators = &snapshot.validators;
    assert_eq!(validators.controlled_nodes.len(), 1);

    let node_snapshot = &validators.controlled_nodes[0];
    assert_eq!(node_snapshot.node_id, node_id);
    assert!(node_snapshot.wallet_addr.is_some());
    // Node is not in the validator set, so vset-derived fields are None.
    assert!(node_snapshot.pubkey.is_none());
    assert!(node_snapshot.adnl.is_none());
    assert!(node_snapshot.key_id.is_none());
    assert!(node_snapshot.key_election_id.is_none());
    assert!(node_snapshot.stake.is_none());
    assert!(!node_snapshot.is_validator, "not in vset");
}

// =====================================================
// TEST: policy override per node
// =====================================================

#[tokio::test]
async fn test_policy_override_per_node() {
    let node_id = "node-1";
    let fixed_override: u64 = 20_000_000_000_000; // 20 000 TON
    let mut harness = TestHarness::new();
    harness.elections_config.policy = StakePolicy::Minimum;
    harness
        .elections_config
        .policy_overrides
        .insert(node_id.to_string(), StakePolicy::Fixed(fixed_override));

    setup_default_elector(&mut harness.elector_mock, ELECTION_ID, 0);
    setup_default_provider(&mut harness.provider_mock, WALLET_BALANCE, None);
    setup_wallet(&mut harness.wallet_mock);

    let mut runner = harness.build(node_id).await;
    let result = runner.run().await;
    assert!(result.is_ok(), "run() failed: {:?}", result.err());

    let node = runner.nodes.get(node_id).unwrap();
    assert!(node.participant.is_some());
    let p = node.participant.as_ref().unwrap();
    assert_eq!(
        p.stake, fixed_override,
        "per-node override should use Fixed(20000 TON), got: {} nanotons",
        p.stake
    );
}

// =====================================================
// TEST: withf — verify wallet.message() payload and
//       send_boc() receives valid external message
// =====================================================

#[tokio::test]
async fn test_withf_verify_stake_message_payload() {
    let node_id = "node-1";
    let mut harness = TestHarness::new();
    let fixed_stake: u64 = 15_000_000_000_000; // 15 000 TON
    harness.elections_config.policy = StakePolicy::Fixed(fixed_stake);

    setup_default_elector(&mut harness.elector_mock, ELECTION_ID, 0);

    // -- wallet mock: verify dest, value, and payload body --
    let wallet = &mut harness.wallet_mock;
    wallet.expect_address().returning(|| Ok(wallet_address()));

    wallet
        .expect_message()
        .withf(move |dest, value, payload| {
            validate_message_parameters(dest, value, payload, fixed_stake)
        })
        .returning(|_dest, _value, _payload| Ok(dummy_cell()));

    // -- provider mock --
    let provider = &mut harness.provider_mock;
    // -- send_boc: verify the BOC is a parseable single-root Cell --
    provider
        .expect_send_boc()
        .withf(|boc: &[u8]| match read_single_root_boc(boc) {
            Ok(_cell) => true,
            Err(e) => {
                eprintln!("withf send_boc: BOC is not parseable: {}", e);
                false
            }
        })
        .returning(|_| Ok(()));

    setup_default_provider(&mut harness.provider_mock, WALLET_BALANCE, Some(POOL_BALANCE));

    let mut runner = harness.build(node_id).await;
    let result = runner.run().await;
    assert!(result.is_ok(), "run() failed: {:?}", result.err());

    let node = runner.nodes.get(node_id).unwrap();
    assert!(node.participant.is_some());
    assert_eq!(node.participant.as_ref().unwrap().stake, fixed_stake);
}

// =====================================================
// TESTS: compute_node_status
// =====================================================

#[test]
fn test_compute_status_idle_when_excluded_no_recover() {
    use common::app_config::BindingStatus;
    let status = ElectionRunner::compute_node_status(true, false, false, false);
    assert_eq!(status, BindingStatus::Idle);
}

#[test]
fn test_compute_status_draining_when_excluded_with_recover() {
    use common::app_config::BindingStatus;
    let status = ElectionRunner::compute_node_status(true, false, true, false);
    assert_eq!(status, BindingStatus::Draining);
}

#[test]
fn test_compute_status_validating_when_in_vset() {
    use common::app_config::BindingStatus;
    let status = ElectionRunner::compute_node_status(false, true, false, true);
    assert_eq!(status, BindingStatus::Validating);
}

#[test]
fn test_compute_status_validating_overrides_excluded() {
    use common::app_config::BindingStatus;
    let status = ElectionRunner::compute_node_status(true, true, false, false);
    assert_eq!(status, BindingStatus::Validating);
}

#[test]
fn test_compute_status_participating_when_enabled_and_participating() {
    use common::app_config::BindingStatus;
    let status = ElectionRunner::compute_node_status(false, false, false, true);
    assert_eq!(status, BindingStatus::Participating);
}

#[test]
fn test_compute_status_draining_when_enabled_with_recover_no_participant() {
    use common::app_config::BindingStatus;
    let status = ElectionRunner::compute_node_status(false, false, true, false);
    assert_eq!(status, BindingStatus::Draining);
}

#[test]
fn test_compute_status_idle_when_enabled_no_recover_no_participant() {
    use common::app_config::BindingStatus;
    let status = ElectionRunner::compute_node_status(false, false, false, false);
    assert_eq!(status, BindingStatus::Idle);
}

const NANO: u64 = 1_000_000_000;
const FACTOR_3X: u32 = 3 * 65536;

// =====================================================
// AdaptiveSplit50: wait/sleep integration tests
// =====================================================

/// Helper: set up elector with a future elect_close and given participants.
/// past_elections_factory: a closure that produces Vec<PastElections> (since PastElections is not Clone).
fn setup_adaptive_elector(
    elector: &mut MockElectorWrapperImpl,
    election_id: u64,
    elect_close: u64,
    participants: Vec<Participant>,
    past_elections_factory: impl Fn() -> Vec<PastElections> + Send + 'static,
) {
    elector.expect_address().returning(|| Ok(elector_address()));
    elector.expect_get_active_election_id().returning(move || Ok(election_id));

    let total_stake: u64 = participants.iter().map(|p| p.stake).sum();
    elector.expect_elections_info().returning(move || {
        Ok(ElectionsInfo {
            election_id,
            elect_close,
            min_stake: MIN_STAKE,
            total_stake,
            failed: false,
            finished: false,
            participants: participants.clone(),
        })
    });

    elector.expect_past_elections().returning(move || Ok(past_elections_factory()));
    elector.expect_compute_returned_stake().returning(|_| Ok(0));
}

#[tokio::test]
async fn test_adaptive_wait_for_participants() {
    // Elections just opened. Only 5 participants (< min_validators=13).
    // elect_close is far in the future → within waiting_period.
    // Expected: stake=0, node defers.
    let node_id = "node-1";
    let mut harness = TestHarness::new();
    harness.elections_config.policy = StakePolicy::AdaptiveSplit50;
    harness.elections_config.sleep_period_pct = 0.0;
    harness.elections_config.waiting_period_pct = 0.3;

    // elect_close far in the future (now + 10_000s) so we're early in the election.
    let now = common::time_format::now();
    let elect_close = now + 10_000;
    let participants = (0..5u8)
        .map(|i| Participant {
            pub_key: vec![i; 32],
            adnl_addr: vec![0xEE; 32],
            wallet_addr: vec![i; 32],
            stake: 300_000 * NANO,
            max_factor: FACTOR_3X,
            election_id: ELECTION_ID,
            stake_message_boc: None,
        })
        .collect();

    setup_adaptive_elector(
        &mut harness.elector_mock,
        ELECTION_ID,
        elect_close,
        participants,
        || vec![],
    );
    setup_default_provider(&mut harness.provider_mock, WALLET_BALANCE, None);
    setup_wallet(&mut harness.wallet_mock);

    let mut runner = harness.build(node_id).await;
    let result = runner.run().await;
    assert!(result.is_ok(), "run() failed: {:?}", result.err());

    // Node should NOT have participated (deferred).
    let node = runner.nodes.get(node_id).unwrap();
    assert!(node.participant.is_none(), "should defer staking when not enough participants");
}

#[tokio::test]
async fn test_adaptive_proceed_after_wait_timeout() {
    // Elections almost over. Only 5 participants (< min_validators=13).
    // elect_close is very close (now + 10s) → waiting_period has expired.
    // prev_min_eff available from past elections.
    // Expected: proceeds to stake despite few participants.
    let node_id = "node-1";
    let mut harness = TestHarness::new();
    harness.elections_config.policy = StakePolicy::AdaptiveSplit50;
    harness.elections_config.sleep_period_pct = 0.0;
    harness.elections_config.waiting_period_pct = 0.3;

    let now = common::time_format::now();
    let elect_close = now + 10; // almost closed

    let participants = (0..5u8)
        .map(|i| Participant {
            pub_key: vec![i; 32],
            adnl_addr: vec![0xEE; 32],
            wallet_addr: vec![i; 32],
            stake: 300_000 * NANO,
            max_factor: FACTOR_3X,
            election_id: ELECTION_ID,
            stake_message_boc: None,
        })
        .collect();

    // Provide past elections with a known frozen stake so prev_min_eff is available.
    // prev_min_eff = 10_000 TON (well below free_balance ~49k).
    setup_adaptive_elector(
        &mut harness.elector_mock,
        ELECTION_ID,
        elect_close,
        participants,
        || {
            let mut frozen_map = HashMap::new();
            frozen_map.insert(
                [0xAA; 32],
                FrozenParticipant {
                    wallet_addr: [0xBB; 32],
                    weight: 1,
                    stake: 10_000 * NANO,
                    banned: false,
                },
            );
            vec![PastElections {
                election_id: ELECTION_ID - 3600,
                unfreeze_at: ELECTION_ID,
                stake_held: 7200,
                vset_hash: vec![],
                frozen_map,
                total_stake: 10_000 * NANO,
                bonuses: 0,
            }]
        },
    );
    setup_default_provider(&mut harness.provider_mock, WALLET_BALANCE, None);
    setup_wallet(&mut harness.wallet_mock);

    let mut runner = harness.build(node_id).await;
    let result = runner.run().await;
    assert!(result.is_ok(), "run() failed: {:?}", result.err());

    // Node SHOULD have participated (timeout expired, fallback to prev).
    let node = runner.nodes.get(node_id).unwrap();
    assert!(node.participant.is_some(), "should proceed to stake after waiting_period expires");
}

#[tokio::test]
async fn test_adaptive_sleep_period_delays_even_with_enough_participants() {
    // Enough participants (20 > min_validators=13) but sleep_period = 0.99
    // (almost the entire election duration) and election just started.
    // Expected: defers despite having enough participants.
    let node_id = "node-1";
    let mut harness = TestHarness::new();
    harness.elections_config.policy = StakePolicy::AdaptiveSplit50;
    harness.elections_config.sleep_period_pct = 0.99;
    harness.elections_config.waiting_period_pct = 0.99;

    let now = common::time_format::now();
    let elect_close = now + 10_000; // election just started

    let participants: Vec<Participant> = (0..20u8)
        .map(|i| Participant {
            pub_key: vec![i; 32],
            adnl_addr: vec![0xEE; 32],
            wallet_addr: vec![i; 32],
            stake: 300_000 * NANO,
            max_factor: FACTOR_3X,
            election_id: ELECTION_ID,
            stake_message_boc: None,
        })
        .collect();

    setup_adaptive_elector(
        &mut harness.elector_mock,
        ELECTION_ID,
        elect_close,
        participants,
        || vec![],
    );
    setup_default_provider(&mut harness.provider_mock, WALLET_BALANCE, None);
    setup_wallet(&mut harness.wallet_mock);

    let mut runner = harness.build(node_id).await;
    let result = runner.run().await;
    assert!(result.is_ok(), "run() failed: {:?}", result.err());

    let node = runner.nodes.get(node_id).unwrap();
    assert!(
        node.participant.is_none(),
        "should defer staking during sleep_period even with enough participants"
    );
}

// =====================================================
// AdaptiveSplit50: config validation tests
// =====================================================

// =====================================================
// AdaptiveSplit50: three-tick top-up integration test
// =====================================================

#[tokio::test]
async fn test_adaptive_topup_three_ticks() {
    // Full adaptive cycle with election emulation and top-up.
    // Uses 13 other participants (= min_validators, boundary value) so emulation works.
    //
    // Tick 1: 13 participants at 100k TON → emulate → curr_min_eff ≈ 32.5k.
    //         wallet = 100k, half ≈ 50k >= min_eff → stake half.
    //
    // Tick 2: Elector sees our stake (~50k) → stake_accepted=true.
    //         current_stake (~50k) >= min_eff (~32.5k) → no top-up.
    //
    // Tick 3: One participant raises stake to 240k → curr_min_eff jumps to ≈ 60k.
    //         Our entry in elector is filtered out before emulation.
    //         current_stake (~50k) < min_eff (~60k) → sends additional stake to elector.
    //         remaining ≈ 50k, total ≈ 100k, half ≈ 50k < min_eff → stake all remaining.

    let node_id = "node-1";
    let wallet_balance = 100_000 * NANO;
    let mut harness = TestHarness::new();
    harness.elections_config.policy = StakePolicy::AdaptiveSplit50;
    let wallet_addr = addr_bytes(&wallet_address());

    let fee = ELECTOR_STAKE_FEE + NPOOL_COMPUTE_FEE;
    let pool_free_balance = wallet_balance - fee - WALLET_STORAGE_RESERVE;
    let initial_stake = pool_free_balance / 2; // stake half on tick 1

    // --- Helper: build participants with given stakes ---
    fn make_participants(stakes: &[(u64, u8)]) -> Vec<Participant> {
        stakes
            .iter()
            .map(|(stake, id)| {
                let mut pubkey = [0u8; 32];
                pubkey[0] = 0x10 + id; // distinct from PUB_KEY
                Participant {
                    pub_key: pubkey.to_vec(),
                    adnl_addr: vec![0xEE; 32],
                    wallet_addr: pubkey.to_vec(),
                    stake: *stake,
                    max_factor: FACTOR_3X,
                    election_id: ELECTION_ID,
                    stake_message_boc: None,
                }
            })
            .collect()
    }

    // 13 participants at 100k each
    let base_stakes: Vec<(u64, u8)> = (0..13u8).map(|i| (100_000 * NANO, i)).collect();
    // Participant #0 raises to 240k on tick 3
    let raised_stakes: Vec<(u64, u8)> =
        (0..13u8).map(|i| (if i == 0 { 240_000 * NANO } else { 100_000 * NANO }, i)).collect();

    // --- Elector: elections_info varies per tick ---
    let ei_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let ei_cc = ei_count.clone();
    let wallet_addr_clone = wallet_addr.clone();
    let base_stakes_clone = base_stakes.clone();
    harness.elector_mock.expect_elections_info().times(3).returning(move || {
        let n = ei_cc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        match n {
            0 => {
                // Tick 1: 13 participants at 100k, our node not yet in elector
                let participants = make_participants(&base_stakes_clone);
                let total: u64 = participants.iter().map(|p| p.stake).sum();
                Ok(ElectionsInfo {
                    election_id: ELECTION_ID,
                    elect_close: ELECTION_ID - 300,
                    min_stake: MIN_STAKE,
                    total_stake: total,
                    failed: false,
                    finished: false,
                    participants,
                })
            }
            1 => {
                // Tick 2: same 13 at 100k + our node in elector
                let mut participants = make_participants(&base_stakes_clone);
                participants.push(Participant {
                    pub_key: PUB_KEY.to_vec(),
                    adnl_addr: ADNL_ADDR.to_vec(),
                    wallet_addr: wallet_addr_clone.clone(),
                    stake: initial_stake,
                    max_factor: FACTOR_3X,
                    election_id: ELECTION_ID,
                    stake_message_boc: None,
                });
                let total: u64 = participants.iter().map(|p| p.stake).sum();
                Ok(ElectionsInfo {
                    election_id: ELECTION_ID,
                    elect_close: ELECTION_ID - 300,
                    min_stake: MIN_STAKE,
                    total_stake: total,
                    failed: false,
                    finished: false,
                    participants,
                })
            }
            _ => {
                // Tick 3: participant #0 doubled to 200k, rest at 100k + our node
                let mut participants = make_participants(&raised_stakes);
                participants.push(Participant {
                    pub_key: PUB_KEY.to_vec(),
                    adnl_addr: ADNL_ADDR.to_vec(),
                    wallet_addr: wallet_addr_clone.clone(),
                    stake: initial_stake,
                    max_factor: FACTOR_3X,
                    election_id: ELECTION_ID,
                    stake_message_boc: None,
                });
                let total: u64 = participants.iter().map(|p| p.stake).sum();
                Ok(ElectionsInfo {
                    election_id: ELECTION_ID,
                    elect_close: ELECTION_ID - 300,
                    min_stake: MIN_STAKE,
                    total_stake: total,
                    failed: false,
                    finished: false,
                    participants,
                })
            }
        }
    });

    // --- Elector: past_elections (fetched once, cached for same election_id) ---
    // prev_min = 30k — only used as fallback if emulation fails.
    harness.elector_mock.expect_past_elections().times(1).returning(|| {
        let mut frozen_map = HashMap::new();
        frozen_map.insert(
            [0xAA; 32],
            FrozenParticipant {
                wallet_addr: [0xBB; 32],
                weight: 1,
                stake: 30_000 * NANO,
                banned: false,
            },
        );
        Ok(vec![PastElections {
            election_id: ELECTION_ID - 3600,
            unfreeze_at: ELECTION_ID,
            stake_held: 7200,
            vset_hash: vec![],
            frozen_map,
            total_stake: 30_000 * NANO,
            bonuses: 0,
        }])
    });

    // --- Provider: validator_config varies per tick ---
    let vc_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let vcc = vc_count.clone();
    harness.provider_mock.expect_validator_config().times(3).returning(move || {
        let n = vcc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n == 0 {
            // Tick 1: no existing key → runner generates new key
            Ok(ValidatorConfig::new())
        } else {
            // Tick 2-3: key available
            let mut keys = HashMap::new();
            keys.insert(
                ELECTION_ID,
                ValidatorEntry {
                    key_id: KEY_ID.to_vec(),
                    public_key: PUB_KEY.to_vec(),
                    adnl_addrs: vec![(ADNL_ADDR.to_vec(), ELECTION_ID + 7200)],
                    expired_at: ELECTION_ID + 7200,
                },
            );
            Ok(ValidatorConfig { keys })
        }
    });

    // --- Provider: dynamic account balance (decreases after staking) ---
    let account_bal = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(wallet_balance));
    let ab = account_bal.clone();
    harness
        .provider_mock
        .expect_account()
        .returning(move |_| Ok(fake_account(ab.load(std::sync::atomic::Ordering::SeqCst))));

    // --- Rest of elector/provider/wallet setup ---
    setup_default_elector(&mut harness.elector_mock, ELECTION_ID, 0);
    setup_default_provider(&mut harness.provider_mock, wallet_balance, None);
    setup_wallet(&mut harness.wallet_mock);

    let mut runner = harness.build(node_id).await;

    // === Tick 1: emulate election → stake half ===
    runner.refresh_validator_configs().await;
    runner.refresh_validator_set().await;
    let r1 = runner.run().await;
    assert!(r1.is_ok(), "tick 1 failed: {:?}", r1.err());
    let node = runner.nodes.get(node_id).unwrap();
    assert!(node.participant.is_some(), "should participate after tick 1");
    assert!(!node.stake_accepted, "stake not yet accepted after tick 1");
    assert_eq!(
        node.participant.as_ref().unwrap().stake,
        initial_stake,
        "tick 1: should stake half of pool_free"
    );
    // Simulate wallet balance decrease after staking
    account_bal.store(wallet_balance - initial_stake - fee, std::sync::atomic::Ordering::SeqCst);

    // === Tick 2: elector accepted, no top-up (current_stake >= min_eff) ===
    runner.refresh_validator_configs().await;
    runner.refresh_validator_set().await;
    let r2 = runner.run().await;
    assert!(r2.is_ok(), "tick 2 failed: {:?}", r2.err());
    let node = runner.nodes.get(node_id).unwrap();
    assert!(node.stake_accepted, "stake should be accepted on tick 2");
    let tick2_stake = node.participant.as_ref().unwrap().stake;
    assert_eq!(
        tick2_stake, initial_stake,
        "tick 2: no top-up needed (current_stake >= curr_min_eff)"
    );

    // === Tick 3: one participant raised stake → top-up ===
    runner.refresh_validator_configs().await;
    runner.refresh_validator_set().await;
    let r3 = runner.run().await;
    assert!(r3.is_ok(), "tick 3 failed: {:?}", r3.err());
    let node = runner.nodes.get(node_id).unwrap();
    assert!(node.stake_accepted, "stake should still be accepted on tick 3");
    let tick3_stake = node.participant.as_ref().unwrap().stake;

    // Remaining wallet ≈ 50k, current_stake ≈ 50k in elector.
    // total ≈ 100k, half ≈ 50k < min_eff (~60k) → stake all remaining.
    let remaining_balance = wallet_balance - initial_stake - fee;
    let pool_free_tick3 = remaining_balance - fee - WALLET_STORAGE_RESERVE;
    assert!(
        tick3_stake > tick2_stake,
        "tick 3: stake should increase via top-up: tick2={}, tick3={}",
        tick2_stake,
        tick3_stake
    );
    assert_eq!(
        tick3_stake,
        tick2_stake + pool_free_tick3,
        "tick 3: should stake all remaining (half < min_eff)"
    );
}

#[test]
fn test_elections_config_validate_sleep_gt_waiting() {
    let config = ElectionsConfig {
        sleep_period_pct: 0.5,
        waiting_period_pct: 0.3, // sleep > waiting → invalid
        ..ElectionsConfig::default()
    };
    assert!(config.validate(None).is_err());
}

#[test]
fn test_elections_config_validate_sleep_out_of_range() {
    let config = ElectionsConfig {
        sleep_period_pct: 1.5, // > 1.0 → invalid
        ..ElectionsConfig::default()
    };
    assert!(config.validate(None).is_err());
}

#[test]
fn test_elections_config_validate_valid() {
    let config = ElectionsConfig {
        sleep_period_pct: 0.1,
        waiting_period_pct: 0.5,
        ..ElectionsConfig::default()
    };
    assert!(config.validate(None).is_ok());
}

#[test]
fn test_elections_config_defaults() {
    let config = ElectionsConfig::default();
    assert_eq!(config.sleep_period_pct, 0.2);
    assert_eq!(config.waiting_period_pct, 0.4);
}

#[tokio::test]
async fn test_calc_max_factor_clamps_to_network_cap() {
    let mut harness = TestHarness::new();
    setup_elector_no_elections(&mut harness.elector_mock);
    harness.elections_config.max_factor = 5.0;
    let runner = harness.build("node-1").await;
    let network_raw = 3 * 65536u32; // chain allows 3×
    let (raw, mult) = runner.calc_max_factor(network_raw);
    assert_eq!(raw, network_raw, "configured 5× must clamp to network 3× (raw)");
    assert!((mult - 3.0).abs() < 1e-3);
}

#[tokio::test]
async fn test_calc_max_factor_no_clamp_when_below_cap() {
    let mut harness = TestHarness::new();
    setup_elector_no_elections(&mut harness.elector_mock);
    harness.elections_config.max_factor = 2.0;
    let runner = harness.build("node-1").await;
    let network_raw = 3 * 65536u32;
    let (raw, mult) = runner.calc_max_factor(network_raw);
    assert_eq!(raw, 2 * 65536, "2× should pass through when network cap is 3×");
    assert!((mult - 2.0).abs() < 1e-3);
}

#[tokio::test]
async fn test_calc_max_factor_clamps_to_minimum_1x() {
    let mut harness = TestHarness::new();
    setup_elector_no_elections(&mut harness.elector_mock);
    harness.elections_config.max_factor = 0.5;
    let runner = harness.build("node-1").await;
    let network_raw = 3 * 65536u32;
    let (raw, mult) = runner.calc_max_factor(network_raw);
    assert_eq!(raw, 65536, "below 1× fixed-point must clamp to 65536");
    assert!((mult - 1.0).abs() < 1e-3);
}

// Participation status transitions across election lifecycle
// Simulates: Idle → Participating → Submitted → Accepted → Elected → Validating
// Also verifies that stale election flags don't leak after elections close.
#[tokio::test]
async fn test_participation_status_lifecycle() {
    use common::snapshot::ParticipationStatus;

    let node_id = "node-1";
    let mut harness = TestHarness::new();

    setup_elector_no_elections(&mut harness.elector_mock);
    setup_wallet(&mut harness.wallet_mock);
    let provider = &mut harness.provider_mock;
    provider.expect_election_parameters().returning(|| Ok(default_cfg15()));
    provider.expect_validator_config().returning(|| Ok(ValidatorConfig::new()));
    provider.expect_get_current_vset().returning(|| Err(anyhow::anyhow!("no vset")));
    provider.expect_get_next_vset().returning(|| Ok(None));
    provider.expect_export_public_key().returning(|_| Ok(PUB_KEY.to_vec()));
    provider.expect_account().returning(|_| Ok(fake_account(WALLET_BALANCE)));
    provider.expect_shutdown().returning(|| Ok(()));

    let mut runner = harness.build(node_id).await;

    async fn participation_status(r: &ElectionRunner, nid: &str) -> ParticipationStatus {
        r.build_our_participants_snapshot()
            .await
            .into_iter()
            .find(|p| p.node_id == nid)
            .unwrap()
            .status
    }

    // --- Phase 1: Idle (no elections, not validating) ---
    runner.snapshot_cache.last_elections_status = ElectionsStatus::Closed;
    assert_eq!(participation_status(&runner, node_id).await, ParticipationStatus::Idle);

    // --- Phase 2: Participating (elections active, participant set, no submissions) ---
    runner.snapshot_cache.last_elections_status = ElectionsStatus::Active;
    let node = runner.nodes.get_mut(node_id).unwrap();
    node.participant = Some(Participant {
        pub_key: PUB_KEY.to_vec(),
        adnl_addr: ADNL_ADDR.to_vec(),
        election_id: ELECTION_ID,
        wallet_addr: addr_bytes(&wallet_address()),
        stake: 0,
        max_factor: 0,
        stake_message_boc: None,
    });
    assert_eq!(participation_status(&runner, node_id).await, ParticipationStatus::Participating);

    // --- Phase 3: Submitted (stake sent) ---
    let node = runner.nodes.get_mut(node_id).unwrap();
    node.stake_submissions.push(StakeSubmissionRecord {
        stake: 10_000_000_000_000,
        max_factor: 3 * 65536,
        submission_time: time_format::now(),
    });
    assert_eq!(participation_status(&runner, node_id).await, ParticipationStatus::Submitted);

    // --- Phase 4: Accepted (elector accepted the stake) ---
    let node = runner.nodes.get_mut(node_id).unwrap();
    node.stake_accepted = true;
    node.accepted_stake_amount = Some(10_000_000_000_000);
    assert_eq!(participation_status(&runner, node_id).await, ParticipationStatus::Accepted);

    // --- Phase 5: Elected (node appears in p36 / next validator set) ---
    let node = runner.nodes.get_mut(node_id).unwrap();
    node.is_next_validator = true;
    assert_eq!(participation_status(&runner, node_id).await, ParticipationStatus::Elected);

    // --- Phase 6: Validating (p36 → p34, elections closed) ---
    // Node moves from next vset to current vset, elections are done.
    let node = runner.nodes.get_mut(node_id).unwrap();
    node.is_next_validator = false;
    node.is_validator = true;
    runner.snapshot_cache.last_elections_status = ElectionsStatus::Closed;
    assert_eq!(participation_status(&runner, node_id).await, ParticipationStatus::Validating);

    // --- Phase 7: Verify stale flags don't leak ---
    // stake_accepted is still true from phase 4, but elections are closed.
    // Must show Validating, NOT Accepted.
    let node = runner.nodes.get(node_id).unwrap();
    assert!(node.stake_accepted, "stake_accepted should still be true (stale)");
    assert_eq!(participation_status(&runner, node_id).await, ParticipationStatus::Validating);

    // --- Phase 8: New elections start while validating ---
    // Node is still in p34, but new election cycle begins and node submits again.
    runner.snapshot_cache.last_elections_status = ElectionsStatus::Active;
    let node = runner.nodes.get_mut(node_id).unwrap();
    node.stake_accepted = false;
    node.accepted_stake_amount = None;
    node.stake_submissions.clear();
    node.participant = Some(Participant {
        pub_key: PUB_KEY.to_vec(),
        adnl_addr: ADNL_ADDR.to_vec(),
        election_id: ELECTION_ID + 3600,
        wallet_addr: addr_bytes(&wallet_address()),
        stake: 0,
        max_factor: 0,
        stake_message_boc: None,
    });
    node.stake_submissions.push(StakeSubmissionRecord {
        stake: 15_000_000_000_000,
        max_factor: 3 * 65536,
        submission_time: time_format::now(),
    });
    // Should show Submitted (election activity), NOT Validating
    assert_eq!(participation_status(&runner, node_id).await, ParticipationStatus::Submitted);

    // --- Phase 9: Back to idle (not validating, no elections) ---
    let node = runner.nodes.get_mut(node_id).unwrap();
    node.is_validator = false;
    node.participant = None;
    node.stake_submissions.clear();
    runner.snapshot_cache.last_elections_status = ElectionsStatus::Closed;
    assert_eq!(participation_status(&runner, node_id).await, ParticipationStatus::Idle);
}

// =====================================================
// TONCore nominator (two pools) tests
// =====================================================

#[tokio::test]
async fn test_toncore_nominator_selects_free_pool() {
    let node_id = "node-1";
    let mut harness = TestHarness::new().with_toncore_nominator_pair();

    setup_default_elector(&mut harness.elector_mock, ELECTION_ID, 0);
    setup_wallet(&mut harness.wallet_mock);

    let (p0, p1) = harness.toncore_nominator_mocks.as_mut().unwrap();
    // pool[0] busy (state=2), pool[1] free (state=0)
    setup_toncore_nominator_slot(p0, pool_address(), 2);
    setup_toncore_nominator_slot(p1, pool_address_1(), 0);

    // Provider returns pool[1] balance when asked for that address
    let pool1_hex = hex::encode(POOL_ADDR_1);
    harness.provider_mock.expect_account().returning(move |address| {
        if address.contains(&pool1_hex) {
            Ok(fake_account(POOL_BALANCE))
        } else {
            Ok(fake_account(WALLET_BALANCE))
        }
    });

    setup_default_provider_without_account(&mut harness.provider_mock, WALLET_BALANCE);

    // TONCore nominator pair + split50: stake entire liquid balance of the active (free) pool.
    let expected_stake = POOL_BALANCE - TONCORE_STORAGE_RESERVE - EXTRA_STORAGE_FEES;
    harness.wallet_mock.expect_message().returning(|_dest, _value, _payload| Ok(dummy_cell()));

    let mut runner = harness.build(node_id).await;
    let result = runner.run().await;
    assert!(result.is_ok(), "run() failed: {:?}", result.err());

    let node = runner.nodes.get(node_id).unwrap();
    assert!(node.participant.is_some());
    let participant = node.participant.as_ref().unwrap();
    // wallet_addr should be pool[1] (the free one), not pool[0]
    assert_eq!(participant.wallet_addr, addr_bytes(&pool_address_1()));
    assert_eq!(participant.stake, expected_stake);
}

#[tokio::test]
async fn test_toncore_frozen_stake_ignored_when_wallet_addr_mismatches_stake_target() {
    let node_id = "node-1";
    let mut harness = TestHarness::new().with_toncore_nominator_pair();
    let past_election_id = ELECTION_ID - 3600;
    let misleading_frozen_stake: u64 = 500_000_000_000_000;

    setup_default_elector(&mut harness.elector_mock, ELECTION_ID, 0);
    harness.elector_mock.expect_past_elections().returning(move || {
        let mut frozen_map = HashMap::new();
        frozen_map.insert(
            PUB_KEY,
            FrozenParticipant {
                wallet_addr: POOL_ADDR,
                weight: 1,
                stake: misleading_frozen_stake,
                banned: false,
            },
        );
        Ok(vec![PastElections {
            election_id: past_election_id,
            unfreeze_at: ELECTION_ID + 3600,
            stake_held: 7200,
            vset_hash: vec![0u8; 32],
            frozen_map,
            total_stake: misleading_frozen_stake,
            bonuses: 0,
        }])
    });

    setup_wallet(&mut harness.wallet_mock);
    harness.wallet_mock.expect_message().returning(|_dest, _value, _payload| Ok(dummy_cell()));

    let (p0, p1) = harness.toncore_nominator_mocks.as_mut().unwrap();
    setup_toncore_nominator_slot(p0, pool_address(), 2);
    setup_toncore_nominator_slot(p1, pool_address_1(), 0);

    let pool1_hex = hex::encode(POOL_ADDR_1);
    harness.provider_mock.expect_account().returning(move |address| {
        if address.contains(&pool1_hex) {
            Ok(fake_account(POOL_BALANCE))
        } else {
            Ok(fake_account(WALLET_BALANCE))
        }
    });

    setup_default_provider_without_account(&mut harness.provider_mock, WALLET_BALANCE);

    let expected_stake = POOL_BALANCE - TONCORE_STORAGE_RESERVE - EXTRA_STORAGE_FEES;

    let mut runner = harness.build(node_id).await;
    {
        let node = runner.nodes.get_mut(node_id).unwrap();
        let mut keys = HashMap::new();
        keys.insert(
            past_election_id,
            ValidatorEntry {
                key_id: KEY_ID.to_vec(),
                public_key: vec![],
                adnl_addrs: vec![(ADNL_ADDR.to_vec(), past_election_id + 7200)],
                expired_at: past_election_id + 7200,
            },
        );
        node.validator_config = ValidatorConfig { keys };
    }

    let result = runner.run().await;
    assert!(result.is_ok(), "run() failed: {:?}", result.err());

    let node = runner.nodes.get(node_id).unwrap();
    assert!(node.participant.is_some());
    let participant = node.participant.as_ref().unwrap();
    assert_eq!(participant.wallet_addr, addr_bytes(&pool_address_1()));
    assert_eq!(
        participant.stake, expected_stake,
        "frozen entry tied to the other TONCore pool must not inflate calc_stake"
    );
}

#[tokio::test]
async fn test_toncore_nominator_both_pools_busy_skips_elections() {
    let node_id = "node-1";
    let mut harness = TestHarness::new().with_toncore_nominator_pair();

    setup_default_elector(&mut harness.elector_mock, ELECTION_ID, 0);
    setup_wallet(&mut harness.wallet_mock);

    let (p0, p1) = harness.toncore_nominator_mocks.as_mut().unwrap();
    setup_toncore_nominator_slot(p0, pool_address(), 2);
    setup_toncore_nominator_slot(p1, pool_address_1(), 2);

    harness
        .provider_mock
        .expect_account()
        .returning(move |_address| Ok(fake_account(WALLET_BALANCE)));

    setup_default_provider_without_account(&mut harness.provider_mock, WALLET_BALANCE);

    let mut runner = harness.build(node_id).await;
    let result = runner.run().await;
    // Both pools busy → router's address() returns "no one pool is ready".
    // Node is excluded from the current tick; run() still succeeds.
    result.expect("run() should succeed — busy node is just skipped");
    assert_eq!(runner.past_elections_cache_id, ELECTION_ID);
    let node = runner.nodes.get(node_id).unwrap();
    assert!(node.pool_addr_cache.is_none());
    assert!(!node.stake_accepted);
}

#[tokio::test]
async fn test_toncore_nominator_recover_stake_uses_cached_pool_address() {
    let node_id = "node-1";
    let mut harness = TestHarness::new().with_toncore_nominator_pair();

    let returned_for_stake_addr = 50_000_000_000_000u64;

    // Elector returns active elections so run() proceeds to recover
    harness.elector_mock.expect_address().returning(|| Ok(elector_address()));
    harness.elector_mock.expect_get_active_election_id().returning(|| Ok(ELECTION_ID));
    harness.elector_mock.expect_elections_info().returning(move || {
        Ok(ElectionsInfo {
            election_id: ELECTION_ID,
            elect_close: ELECTION_ID - 300,
            min_stake: MIN_STAKE,
            total_stake: 0,
            failed: false,
            finished: false,
            participants: vec![],
        })
    });
    harness.elector_mock.expect_past_elections().returning(|| Ok(vec![]));
    // `recover_stake` asks the elector once using `stake_addr()` (pool_addr_cache → primary pool).
    harness
        .elector_mock
        .expect_compute_returned_stake()
        .returning(move |_addr| Ok(returned_for_stake_addr));

    setup_wallet(&mut harness.wallet_mock);
    harness.wallet_mock.expect_message().returning(|_dest, _value, _payload| Ok(dummy_cell()));

    let (p0, p1) = harness.toncore_nominator_mocks.as_mut().unwrap();
    setup_toncore_nominator_slot(p0, pool_address(), 0);
    setup_toncore_nominator_slot(p1, pool_address_1(), 0);

    harness.provider_mock.expect_election_parameters().returning(|| Ok(default_cfg15()));
    harness.provider_mock.expect_get_current_vset().returning(|| Err(anyhow::anyhow!("no vset")));
    harness.provider_mock.expect_get_next_vset().returning(|| Ok(None));
    harness.provider_mock.expect_validator_config().returning(|| Ok(ValidatorConfig::new()));
    harness
        .provider_mock
        .expect_account()
        .returning(move |_address| Ok(fake_account(WALLET_BALANCE)));
    harness.provider_mock.expect_send_boc().returning(|_boc| Ok(()));
    harness.provider_mock.expect_config_param_16().returning(|| Ok(default_cfg16()));
    harness.provider_mock.expect_config_param_17().returning(|| Ok(default_cfg17()));
    harness.provider_mock.expect_shutdown().returning(|| Ok(()));

    let mut runner = harness.build(node_id).await;
    let result = runner.run().await;
    assert!(result.is_ok(), "run() failed: {:?}", result.err());

    let node = runner.nodes.get(node_id).unwrap();
    assert_eq!(node.last_recover_amount, returned_for_stake_addr);
}

#[tokio::test]
async fn test_toncore_nominator_elections_finished_checks_active_pool_only() {
    let node_id = "node-1";
    let mut harness = TestHarness::new().with_toncore_nominator_pair();

    // Elections are finished with a participant from pool[1]
    harness.elector_mock.expect_address().returning(|| Ok(elector_address()));
    harness.elector_mock.expect_get_active_election_id().returning(|| Ok(ELECTION_ID));
    let pool1_addr_bytes = addr_bytes(&pool_address_1());
    harness.elector_mock.expect_elections_info().returning(move || {
        Ok(ElectionsInfo {
            election_id: ELECTION_ID,
            elect_close: ELECTION_ID - 300,
            min_stake: MIN_STAKE,
            total_stake: 100_000_000_000_000,
            failed: false,
            finished: true,
            participants: vec![Participant {
                pub_key: PUB_KEY.to_vec(),
                adnl_addr: ADNL_ADDR.to_vec(),
                election_id: ELECTION_ID,
                wallet_addr: pool1_addr_bytes.clone(),
                stake: 50_000_000_000_000,
                max_factor: 196608,
                stake_message_boc: None,
            }],
        })
    });
    harness.elector_mock.expect_past_elections().returning(|| Ok(vec![]));
    harness.elector_mock.expect_compute_returned_stake().returning(|_| Ok(0));

    setup_wallet(&mut harness.wallet_mock);

    let (p0, p1) = harness.toncore_nominator_mocks.as_mut().unwrap();
    setup_toncore_nominator_slot(p0, pool_address(), 0);
    setup_toncore_nominator_slot(p1, pool_address_1(), 0);

    harness.provider_mock.expect_election_parameters().returning(|| Ok(default_cfg15()));
    harness.provider_mock.expect_config_param_16().returning(|| Ok(default_cfg16()));
    harness.provider_mock.expect_config_param_17().returning(|| Ok(default_cfg17()));

    let mut runner = harness.build(node_id).await;
    let result = runner.run().await;
    assert!(result.is_ok(), "run() failed: {:?}", result.err());

    let node = runner.nodes.get(node_id).unwrap();
    // Router now resolves a single active pool address; participant from non-active slot is not matched.
    assert!(!node.stake_accepted);
    assert_eq!(node.accepted_stake_amount, None);
}

// =====================================================
// TEST: SnapshotCache::update_next_elections_range
// =====================================================

fn make_vset(utime_since: u32, utime_until: u32) -> ValidatorSet {
    ValidatorSet::new(utime_since, utime_until, 1, vec![ValidatorDescr::default()])
        .expect("validator set")
}

#[test]
fn next_elections_range_before_window_opens_points_at_upcoming_cycle() {
    let cfg15 = default_cfg15();
    let now = time_format::now() as u32;
    // Place utime_until far enough in the future that the upcoming cycle's
    // elections window has not opened yet (i.e. now < utime_until - elections_start_before).
    let utime_until = now + cfg15.elections_start_before + 3600;
    let vset = make_vset(utime_until - cfg15.validators_elected_for, utime_until);
    let mut cache = SnapshotCache { last_validator_set: Some(vset), ..Default::default() };

    cache.update_next_elections_range(&cfg15);

    let range = cache.next_elections_range.expect("range set");
    assert_eq!(range.start, (utime_until - cfg15.elections_start_before) as u64);
    assert_eq!(range.end, (utime_until - cfg15.elections_end_before) as u64);
    assert_eq!(
        range.end - range.start,
        (cfg15.elections_start_before - cfg15.elections_end_before) as u64,
    );
}

#[test]
fn next_elections_range_inside_window_still_points_at_upcoming_cycle() {
    let cfg15 = default_cfg15();
    let now = time_format::now() as u32;
    // Window currently open: now > utime_until - elections_start_before AND
    // now < utime_until - elections_end_before. Pick utime_until = now + (end_before + 600).
    let utime_until = now + cfg15.elections_end_before + 600;
    let vset = make_vset(utime_until - cfg15.validators_elected_for, utime_until);
    let mut cache = SnapshotCache { last_validator_set: Some(vset), ..Default::default() };

    cache.update_next_elections_range(&cfg15);

    let range = cache.next_elections_range.expect("range set");
    assert_eq!(range.start, (utime_until - cfg15.elections_start_before) as u64);
    assert_eq!(range.end, (utime_until - cfg15.elections_end_before) as u64);
}

#[test]
fn next_elections_range_after_window_closed_advances_to_next_next_cycle() {
    let cfg15 = default_cfg15();
    let now = time_format::now() as u32;
    // Upcoming cycle's elections already closed: now >= utime_until - elections_end_before.
    // Pick utime_until = now + 60 so upcoming_elections_end = now - 540.
    let utime_until = now + 60;
    let vset = make_vset(utime_until - cfg15.validators_elected_for, utime_until);
    let mut cache = SnapshotCache { last_validator_set: Some(vset), ..Default::default() };

    cache.update_next_elections_range(&cfg15);

    let range = cache.next_elections_range.expect("range set");
    let next_cycle_id = utime_until + cfg15.validators_elected_for;
    assert_eq!(range.start, (next_cycle_id - cfg15.elections_start_before) as u64);
    assert_eq!(range.end, (next_cycle_id - cfg15.elections_end_before) as u64);
    assert_eq!(
        range.end - range.start,
        (cfg15.elections_start_before - cfg15.elections_end_before) as u64,
    );
}

// =====================================================
// TEST: validator snapshot sources fields from vset
// =====================================================

const ADNL_VSET: [u8; 32] = [0x05u8; 32];
const FROZEN_STAKE: u64 = 30_000_000_000_000; // 30 000 TON

/// Build a ValidatorSet containing one validator with the given pubkey and ADNL.
fn make_vset_with_validator(
    utime_since: u32,
    utime_until: u32,
    pubkey: &[u8; 32],
    adnl: &[u8; 32],
) -> ValidatorSet {
    let vd = ValidatorDescr::with_params(
        SigPubKey::with_bytes(*pubkey),
        100,
        Some(UInt256::from(*adnl)),
    );
    ValidatorSet::new(utime_since, utime_until, 1, vec![vd]).expect("validator set")
}

#[tokio::test]
async fn test_validator_snapshot_uses_vset_data() {
    let node_id = "node-1";
    let mut harness = TestHarness::new();

    // Elector: minimal (no active elections needed for snapshot).
    setup_elector_no_elections(&mut harness.elector_mock);

    // Provider: return validator_config with an existing key entry for ELECTION_ID.
    harness.provider_mock.expect_validator_config().returning(|| {
        let mut keys = HashMap::new();
        keys.insert(
            ELECTION_ID,
            crate::providers::ValidatorEntry {
                key_id: KEY_ID.to_vec(),
                public_key: vec![],
                adnl_addrs: vec![(ADNL_ADDR.to_vec(), ELECTION_ID + 7200)],
                expired_at: ELECTION_ID + 7200,
            },
        );
        Ok(ValidatorConfig { keys })
    });
    harness.provider_mock.expect_export_public_key().returning(|_| Ok(PUB_KEY.to_vec()));
    harness.provider_mock.expect_election_parameters().returning(|| Ok(default_cfg15()));
    harness.provider_mock.expect_get_current_vset().returning(|| Err(anyhow::anyhow!("skip")));
    harness.provider_mock.expect_get_next_vset().returning(|| Ok(None));
    harness.provider_mock.expect_shutdown().returning(|| Ok(()));

    setup_wallet(&mut harness.wallet_mock);

    let mut runner = harness.build(node_id).await;
    runner.refresh_validator_configs().await;

    // Inject a validator set containing our node with a DIFFERENT adnl than the elections one.
    let vset = make_vset_with_validator(
        ELECTION_ID as u32,
        ELECTION_ID as u32 + 3600,
        &PUB_KEY,
        &ADNL_VSET,
    );
    runner.snapshot_cache.last_validator_set = Some(vset);

    // Inject past_elections with frozen stake for our pubkey.
    let mut frozen_map = HashMap::new();
    frozen_map.insert(
        PUB_KEY,
        FrozenParticipant {
            wallet_addr: [0xAA; 32],
            weight: 100,
            stake: FROZEN_STAKE,
            banned: false,
        },
    );
    runner.past_elections = vec![PastElections {
        election_id: ELECTION_ID,
        unfreeze_at: ELECTION_ID + 7200,
        stake_held: 7200,
        vset_hash: vec![0; 32],
        frozen_map,
        total_stake: FROZEN_STAKE,
        bonuses: 0,
    }];

    let store = Arc::new(SnapshotStore::new());
    runner.publish_snapshot(&store).await;

    let snapshot = store.get();
    let node_snapshot = &snapshot.validators.controlled_nodes[0];

    assert!(node_snapshot.is_validator, "should be in vset");
    assert_eq!(node_snapshot.validator_index, Some(0));

    // pubkey and adnl come from vset, NOT from elections data.
    assert_eq!(
        node_snapshot.pubkey,
        Some(base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &PUB_KEY)),
    );
    assert_eq!(
        node_snapshot.adnl,
        Some(base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &ADNL_VSET)),
        "adnl must come from vset, not elections"
    );

    // key_id should come from the vset-matched config entry (KEY_ID).
    assert_eq!(
        node_snapshot.key_id,
        Some(base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &KEY_ID)),
    );
    assert_eq!(node_snapshot.key_election_id, Some(ELECTION_ID));

    // stake comes from frozen_map, not participant.
    assert_eq!(
        node_snapshot.stake,
        Some(nanotons_to_dec_string(FROZEN_STAKE)),
        "stake must come from past_elections frozen_map"
    );
}

#[tokio::test]
async fn test_validator_snapshot_none_when_not_in_vset() {
    let node_id = "node-1";
    let mut harness = TestHarness::new();

    setup_default_elector(&mut harness.elector_mock, ELECTION_ID, 0);
    setup_default_provider(&mut harness.provider_mock, WALLET_BALANCE, None);
    setup_wallet(&mut harness.wallet_mock);

    let mut runner = harness.build(node_id).await;
    let _ = runner.run().await;

    // No vset injected — node is NOT a validator.
    let store = Arc::new(SnapshotStore::new());
    runner.publish_snapshot(&store).await;

    let snapshot = store.get();
    let node_snapshot = &snapshot.validators.controlled_nodes[0];

    assert!(!node_snapshot.is_validator, "not in vset");
    assert!(node_snapshot.pubkey.is_none(), "pubkey must be None when not in vset");
    assert!(node_snapshot.adnl.is_none(), "adnl must be None when not in vset");
    assert!(node_snapshot.key_id.is_none(), "key_id must be None when not in vset");
    assert!(
        node_snapshot.key_election_id.is_none(),
        "key_election_id must be None when not in vset"
    );
    assert!(node_snapshot.stake.is_none(), "stake must be None when not in vset");
}

// =====================================================
// TEST: static ADNL — register_adnl_addr is called
// =====================================================

#[tokio::test]
async fn test_static_adnl_uses_register_instead_of_new() {
    let node_id = "node-1";
    let static_adnl = [0xAA_u8; 32];
    let static_adnl_b64 =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &static_adnl);

    let mut harness = TestHarness::new();
    harness.elections_config.static_adnls.insert(node_id.to_string(), static_adnl_b64);

    setup_default_elector(&mut harness.elector_mock, ELECTION_ID, 0);

    // Setup provider WITHOUT new_adnl_addr — it must NOT be called.
    // Instead, register_adnl_addr must be called.
    harness.provider_mock.expect_election_parameters().returning(|| Ok(default_cfg15()));
    harness.provider_mock.expect_validator_config().returning(|| Ok(ValidatorConfig::new()));
    harness.provider_mock.expect_get_current_vset().returning(|| Err(anyhow::anyhow!("no vset")));
    harness.provider_mock.expect_get_next_vset().returning(|| Ok(None));
    harness
        .provider_mock
        .expect_new_validator_key()
        .returning(|_since, _until| Ok((KEY_ID.to_vec(), PUB_KEY.to_vec())));
    harness.provider_mock.expect_export_public_key().returning(|_key_id| Ok(PUB_KEY.to_vec()));
    harness.provider_mock.expect_sign().returning(|_key, _data| Ok(SIGNATURE.to_vec()));
    harness.provider_mock.expect_account().returning(move |_addr| Ok(fake_account(WALLET_BALANCE)));
    harness.provider_mock.expect_send_boc().returning(|_boc| Ok(()));
    harness.provider_mock.expect_config_param_16().returning(|| Ok(default_cfg16()));
    harness.provider_mock.expect_config_param_17().returning(|| Ok(default_cfg17()));
    harness.provider_mock.expect_shutdown().returning(|| Ok(()));

    // register_adnl_addr: expect exactly 1 call with the static ADNL key_id
    let expected_adnl = static_adnl.to_vec();
    harness
        .provider_mock
        .expect_register_adnl_addr()
        .withf(move |adnl_key_id, _perm_key_id, _until| adnl_key_id == expected_adnl.as_slice())
        .times(1)
        .returning(|_adnl, _perm, _until| Ok(()));

    // new_adnl_addr must NOT be called (no expectation = panics if called)

    setup_wallet(&mut harness.wallet_mock);
    harness.wallet_mock.expect_message().returning(|_dest, _value, _payload| Ok(dummy_cell()));

    let mut runner = harness.build(node_id).await;
    let result = runner.run().await;
    assert!(result.is_ok(), "run() failed: {:?}", result.err());

    let node = runner.nodes.get(node_id).unwrap();
    assert!(node.participant.is_some(), "participant should be set");
    let participant = node.participant.as_ref().unwrap();
    assert_eq!(participant.adnl_addr, static_adnl.to_vec(), "ADNL must be the static one");
}
