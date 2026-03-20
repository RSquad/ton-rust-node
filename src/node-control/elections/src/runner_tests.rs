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
};
use contracts::{
    ElectionsInfo, ElectorWrapper, NominatorWrapper, Participant, TonWallet,
    elector::{FrozenParticipant, PastElections},
    nominator::{NominatorRoles, PoolData, opcodes},
};
use mockall::mock;
use std::{collections::HashMap, sync::Arc, time::Duration};
use ton_block::{
    BuilderData, Cell, ConfigParam15, Deserializable, Coins as Coins, MsgAddressInt, SliceData,
    ValidatorSet, read_single_root_boc,
};

// ---- Address helpers ----

const POOL_ADDR: [u8; 32] = [0xBBu8; 32];

fn wallet_address() -> MsgAddressInt {
    MsgAddressInt::standard(-1, [0xAAu8; 32])
}

fn pool_address() -> MsgAddressInt {
    MsgAddressInt::standard(-1, POOL_ADDR)
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
    }
}

// ---- Mock: ElectorWrapper ----

mock! {
    ElectorWrapperImpl {}

    #[async_trait::async_trait]
    impl contracts::SmartContract for ElectorWrapperImpl {
        async fn balance(&self) -> anyhow::Result<u64>;
        fn address(&self) -> MsgAddressInt;
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
        fn address(&self) -> MsgAddressInt;
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
    NominatorWrapperImpl {}

    #[async_trait::async_trait]
    impl contracts::SmartContract for NominatorWrapperImpl {
        async fn balance(&self) -> anyhow::Result<u64>;
        fn address(&self) -> MsgAddressInt;
    }

    #[async_trait::async_trait]
    impl NominatorWrapper for NominatorWrapperImpl {
        async fn get_roles(&self) -> anyhow::Result<NominatorRoles>;
        async fn get_pool_data(&self) -> anyhow::Result<PoolData>;
        fn state_init(&self) -> Option<ton_block::StateInit>;
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
    pool_mock: Option<MockNominatorWrapperImpl>,
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
            elections_config: ElectionsConfig {
                policy: StakePolicy::Split50,
                policy_overrides: HashMap::new(),
                max_factor: 3.0,
                tick_interval: 10,
            },
            bindings: HashMap::new(),
        }
    }

    fn with_pool(mut self) -> Self {
        self.pool_mock = Some(MockNominatorWrapperImpl::new());
        self
    }

    fn build(mut self, node_id: &str) -> ElectionRunner {
        self.bindings.entry(node_id.to_string()).or_insert_with(|| default_binding(true));

        let wallet: Arc<dyn TonWallet> = Arc::new(self.wallet_mock);
        let mut wallets: HashMap<String, Arc<dyn TonWallet>> = HashMap::new();
        wallets.insert(node_id.to_string(), wallet);

        let mut providers: HashMap<String, Box<dyn ElectionsProvider>> = HashMap::new();
        providers.insert(node_id.to_string(), Box::new(self.provider_mock));

        let mut pools: HashMap<String, Arc<dyn NominatorWrapper>> = HashMap::new();
        if let Some(pool) = self.pool_mock {
            pools.insert(node_id.to_string(), Arc::new(pool));
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
    elector.expect_address().returning(|| elector_address());

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
    elector.expect_address().returning(|| elector_address());
    elector.expect_get_active_election_id().returning(|| Ok(0));
}

fn setup_wallet(wallet: &mut MockTonWalletImpl) {
    wallet.expect_address().returning(|| wallet_address());
    wallet.expect_message().returning(|_dest, _value, _payload| Ok(dummy_cell()));
}

fn setup_pool(pool: &mut MockNominatorWrapperImpl) {
    pool.expect_address().returning(|| pool_address());
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
        (WALLET_BALANCE - (ELECTOR_STAKE_FEE + NPOOL_COMPUTE_FEE) - MIN_NANOTON_FOR_STORAGE) / 2;
    // validate the election bid payload
    harness
        .wallet_mock
        .expect_message()
        .withf(move |dest, value, payload| {
            validate_message_parameters(dest, value, payload, expected_stake)
        })
        .returning(|_dest, _value, _payload| Ok(dummy_cell()));

    let mut runner = harness.build(node_id);

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

    let expected_stake = (POOL_BALANCE - MIN_NANOTON_FOR_STORAGE) / 2;
    // validate the election bid payload
    harness
        .wallet_mock
        .expect_message()
        .withf(move |dest, value, payload| {
            validate_message_parameters(dest, value, payload, expected_stake)
        })
        .returning(|_dest, _value, _payload| Ok(dummy_cell()));

    let mut runner = harness.build(node_id);

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

    let mut runner = harness.build(node_id);
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

    harness.elector_mock.expect_address().returning(|| elector_address());
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
    provider.expect_shutdown().returning(|| Ok(()));

    let mut runner = harness.build(node_id);
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
    provider.expect_shutdown().returning(|| Ok(()));

    let mut runner = harness.build(node_id);
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
    provider.expect_shutdown().returning(|| Ok(()));

    let mut runner = harness.build(node_id);
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

    let mut runner = harness.build(node_id);
    let result = runner.run().await;
    assert!(result.is_ok());

    assert_eq!(runner.snapshot_cache.last_elections_status, ElectionsStatus::Closed);
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
    provider.expect_shutdown().returning(|| Ok(()));

    let mut runner = harness.build(node_id);
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
    let mut runner = harness.build(node_id);
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

    let mut runner = harness.build(node_id);

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

    harness.elector_mock.expect_address().returning(|| elector_address());
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

    let mut runner = harness.build(node_id);
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

    let mut runner = harness.build(node_id);
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

    let mut runner = harness.build(node_id);
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
                wallet_addr: [0xAAu8; 32],
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

    let mut runner = harness.build(node_id);
    let result = runner.run().await;
    assert!(result.is_ok(), "run() failed: {:?}", result.err());

    // Node should participate; the frozen stake should be factored into calc_stake
    let node = runner.nodes.get(node_id).unwrap();
    assert!(node.participant.is_some());
    let p = node.participant.as_ref().unwrap();
    // With Split50 policy, stake = max(total_balance / 2, min_stake)
    // total_balance = frozen_stake + pool_free_balance + 0
    // pool_free_balance = WALLET_BALANCE - gas_fee - MIN_NANOTON_FOR_STORAGE
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

    provider.expect_shutdown().returning(|| Ok(()));

    let mut runner = harness.build(node_id);
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

    let mut runner = harness.build(node_id);
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
    wallet1.expect_address().returning(|| wallet_address());
    wallet1.expect_message().returning(|_, _, _| Ok(dummy_cell()));

    let mut wallet2 = MockTonWalletImpl::new();
    wallet2.expect_address().returning(|| MsgAddressInt::standard(-1, [0xCCu8; 32]));
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
    provider2.expect_export_public_key().returning(|_| Ok(PUB_KEY.to_vec()));
    provider2.expect_account().returning(|_| Ok(fake_account(WALLET_BALANCE)));
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

    harness.elector_mock.expect_address().returning(|| elector_address());
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

    let mut runner = harness.build(node_id);
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

    harness.elector_mock.expect_address().returning(|| elector_address());
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

    provider.expect_shutdown().returning(|| Ok(()));

    let mut runner = harness.build(node_id);
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

    let mut runner = harness.build(node_id);
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
    provider.expect_shutdown().returning(|| Ok(()));

    let mut runner = harness.build(node_id);
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
    provider.expect_shutdown().returning(|| Ok(()));
    provider.expect_sign().returning(|_key, _data| Ok(SIGNATURE.to_vec()));

    let mut runner = harness.build(node_id);
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

    let mut runner = harness.build(node_id);
    let result = runner.run().await;
    assert!(result.is_ok(), "run() failed: {:?}", result.err());

    let node = runner.nodes.get(node_id).unwrap();
    assert!(node.participant.is_some());
    let p = node.participant.as_ref().unwrap();

    // With Split50: stake = max(total_balance / 2, min_stake)
    // total_balance = frozen_stake(0) + pool_free_balance + elections_stake(0)
    // pool_free_balance = WALLET_BALANCE - gas_fee - MIN_NANOTON_FOR_STORAGE
    let gas_fee = ELECTOR_STAKE_FEE + NPOOL_COMPUTE_FEE;
    let pool_free_balance = WALLET_BALANCE - gas_fee - MIN_NANOTON_FOR_STORAGE;
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

    harness.elector_mock.expect_address().returning(|| elector_address());
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

    let mut runner = harness.build(node_id);
    let result = runner.run().await;
    assert!(result.is_ok(), "run() failed: {:?}", result.err());

    let snapshot = runner.snapshot_cache.last_elections.as_ref().expect("snapshot should exist");
    assert_eq!(snapshot.election_id, ELECTION_ID);
    assert_eq!(snapshot.elect_close, ELECTION_ID + 600);
    assert!(!snapshot.finished);
    assert!(!snapshot.failed);
    assert_eq!(snapshot.participants_count, 2);
    assert_eq!(snapshot.min_stake, nanotons_to_dec_string(MIN_STAKE));
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
    assert_eq!(our_participant.pubkey, hex::encode(&PUB_KEY));
    assert_eq!(our_participant.stake, nanotons_to_dec_string(MIN_STAKE));

    let other_participant = snapshot
        .participants
        .iter()
        .find(|p| !p.is_controlled)
        .expect("should have non-controlled participant");
    assert_eq!(other_participant.pubkey, hex::encode([0xDD; 32]));
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

    let mut runner = harness.build(node_id);

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

    let mut runner = harness.build(node_id);
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
    assert!(node_snapshot.pubkey.is_some());
    assert!(node_snapshot.adnl.is_some());
    assert!(node_snapshot.stake.is_some());
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

    let mut runner = harness.build(node_id);
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
    wallet.expect_address().returning(|| wallet_address());

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

    let mut runner = harness.build(node_id);
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
