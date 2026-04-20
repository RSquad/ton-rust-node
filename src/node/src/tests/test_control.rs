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
#[cfg(feature = "telemetry")]
use crate::collator_test_bundle::create_engine_telemetry;
use crate::{
    collator_test_bundle::create_engine_allocated,
    config::TonNodeConfig,
    engine::Engine,
    engine_traits::EngineOperations,
    internal_db::{state_gc_resolver::AllowStateGcSmartResolver, InternalDb, InternalDbConfig},
    network::{
        control::{ControlQuerySubscriber, ControlServer, DataSource, StatusReporter},
        node_network::NodeNetwork,
    },
    shard_state::ShardStateStuff,
    shard_states_keeper::PinnedShardStateGuard,
    test_helper::{
        create_network, gen_master_state, gen_shard_state, gen_test_account, init_test_log,
        test_async, GenMasterStateParams,
    },
    validating_utils::{supported_capabilities, supported_version},
    validator::validator_manager::ValidationStatus,
};
use adnl::{
    client::{AdnlClient, AdnlClientConfig},
    server::AdnlServerConfig,
};
use std::{
    collections::HashMap,
    fs::remove_dir_all,
    ops::Deref,
    sync::{
        atomic::{AtomicBool, AtomicU16, AtomicU8, Ordering},
        Arc,
    },
};
use storage::block_handle_db::BlockHandle;
use ton_api::{
    serialize_boxed,
    ton::{
        self,
        accountaddress::AccountAddress,
        engine::validator::{ControlQueryError, JsonConfig, KeyHash, Stats, Success},
        lite_server::ConfigInfo,
        raw::{AppliedShardsInfo, ShardAccountMeta, ShardAccountState},
        rpc::{
            engine::validator::{
                AddValidatorAdnlAddress, AddValidatorPermanentKey, ControlQuery, GenerateKeyPair,
                GetConfig, GetSelectedStats, GetStats, ImportPrivateKey,
            },
            lite_server::GetConfigAll,
            raw::{
                GetAccountByBlock, GetAccountMetaByBlock, GetAppliedShardsInfo,
                GetShardAccountMeta, GetShardAccountState,
            },
        },
    },
    AnyBoxedSerialize, TLObject,
};
use ton_block::{
    base64_encode, error, fail, Account, AccountId, BlockIdExt, ConfigParamEnum, ConfigParams,
    Deserializable, KeyId, Message, Result, Serializable, ShardIdent, UInt256, UnixTime,
};

// key pair for server
// "pub_key": "cujCRU4rQbSw48yHVHxQtRPhUlbo+BuZggFTQSu04Y8="
// "pvt_key": "cJIxGZviebMQWL726DRejqVzRTSXPv/1sO/ab6XOZXk="

// key pair for client
// "pub_key": "RYokIiD5AFkzfTBgC6NhtAGFKm0+gwhN4suTzaW0Sjw="
// "pvt_key": "oEivbTDjSOSCgooUM0DAS2z2hIdnLw/PT82A/OFLDmA="

const ADNL_SERVER_CONFIG: &str = r#"{
    "address": "127.0.0.1:port",
    "server_key": {
        "type_id": 1209251014,
        "pvt_key": "cJIxGZviebMQWL726DRejqVzRTSXPv/1sO/ab6XOZXk="
    },
    "clients": {
        "list": [
            {
                "type_id": 1209251014,
                "pub_key": "RYokIiD5AFkzfTBgC6NhtAGFKm0+gwhN4suTzaW0Sjw="
            }
        ]
    }
}"#;

const ADNL_CLIENT_CONFIG: &str = r#"{
    "server_address": "127.0.0.1:port",
    "server_key": {
        "type_id": 1209251014,
        "pub_key": "cujCRU4rQbSw48yHVHxQtRPhUlbo+BuZggFTQSu04Y8="
    },
    "client_key": {
        "type_id": 1209251014,
        "pvt_key": "oEivbTDjSOSCgooUM0DAS2z2hIdnLw/PT82A/OFLDmA="
    }
}"#;

const IP_NODE: &str = "127.0.0.1:4191";
const DEFAULT_CONFIG: &str = "../node/configs/default_config_localhost.json";
const DEFAULT_CONTROL_PORT: u16 = 4925;

async fn generate_keypair(client: &mut AdnlClient) -> Result<UInt256> {
    let answer: KeyHash = request(client, GenerateKeyPair).await?;
    Ok(answer.only().key_hash)
}

async fn import_private_key(client: &mut AdnlClient, private_key: &str) -> Result<UInt256> {
    use ton_block::base64_decode;
    let private_key_bytes = base64_decode(private_key)?;
    let private_key = ton_api::ton::PrivateKey::Pk_Ed25519(ton::pk::privatekey::Ed25519 {
        key: UInt256::with_array(private_key_bytes.as_slice().try_into()?),
    });
    let answer: KeyHash = request(client, ImportPrivateKey { key: private_key }).await?;
    Ok(answer.only().key_hash)
}

fn get_next_control_port() -> u16 {
    static PORT: AtomicU16 = AtomicU16::new(DEFAULT_CONTROL_PORT);
    PORT.fetch_add(1, Ordering::Relaxed)
}

async fn query(client: &mut AdnlClient, query: &TLObject) -> Result<TLObject> {
    let control_query = ControlQuery { data: serialize_boxed(query)? }.into_tl_object().into();
    match client.query(&control_query).await?.downcast::<ControlQueryError>() {
        Ok(error) => fail!("Error response to {:?}: {:?}", query, error),
        Err(answer) => Ok(answer),
    }
}

async fn request<Q, A>(client: &mut AdnlClient, request: Q) -> Result<A>
where
    A: AnyBoxedSerialize,
    Q: AnyBoxedSerialize,
{
    let boxed = request.into_tl_object();
    query(client, &boxed)
        .await?
        .downcast::<A>()
        .map_err(|answer| error!("Unsupported answer to {:?}: {:?}", boxed, answer))
}

async fn start_control_with_options(
    data_source: DataSource,
    config: Option<TonNodeConfig>,
    test_name: &str,
) -> Result<(ControlServer, AdnlClient, String, Arc<KeyId>)> {
    let network = create_network(config, Some(test_name), IP_NODE).await.unwrap();
    let key_id = network.get_key_id_by_tag(NodeNetwork::TAG_OVERLAY_KEY)?;
    let port = format!(":{}", get_next_control_port());
    let config = ADNL_SERVER_CONFIG.replace(":port", &port);
    let config = AdnlServerConfig::from_json(&config)?;
    let control = ControlServer::with_params(
        config,
        data_source,
        network.config_handler(),
        network.config_handler(),
        Some(&network),
    )
    .await?;
    let client_config = ADNL_CLIENT_CONFIG.replace(":port", &port);
    let (_, config) = AdnlClientConfig::from_json(&client_config)?;
    let client = AdnlClient::connect(&config).await?;
    Ok((control, client, client_config, key_id))
}

async fn start_control(
    data_source: DataSource,
    test_name: &str,
) -> Result<(ControlServer, AdnlClient, String, Arc<KeyId>)> {
    start_control_with_options(data_source, None, test_name).await
}

async fn start_control_with_config(
    data_source: DataSource,
    config: TonNodeConfig,
    test_name: &str,
) -> Result<(ControlServer, AdnlClient, String, Arc<KeyId>)> {
    start_control_with_options(data_source, Some(config), test_name).await
}

async fn recreate_client(old_client: AdnlClient, config: &str) -> AdnlClient {
    old_client.shutdown().await.unwrap();
    let (_, config) = AdnlClientConfig::from_json(config).unwrap();
    AdnlClient::connect(&config).await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn test_get_all_config_params() {
    struct TestEngine {
        config_addr: AccountId,
        state_id: BlockIdExt,
        state: Arc<ShardStateStuff>,
    }

    impl TestEngine {
        fn new() -> Self {
            let state = gen_master_state(
                Default::default(),
                #[cfg(feature = "telemetry")]
                None,
                None,
            );
            let state_id = state.block_id().clone();
            Self {
                config_addr: state
                    .state()
                    .unwrap()
                    .read_custom()
                    .unwrap()
                    .unwrap()
                    .config
                    .config_address()
                    .unwrap(),
                state_id,
                state,
            }
        }
    }

    #[async_trait::async_trait]
    impl EngineOperations for TestEngine {
        async fn load_last_applied_mc_state(&self) -> Result<Arc<ShardStateStuff>> {
            Ok(self.state.clone())
        }
    }

    init_test_log();
    let engine = Arc::new(TestEngine::new());
    let (control, mut client, _, _) =
        start_control(DataSource::Engine(engine.clone()), "test_get_all_config_params")
            .await
            .unwrap();
    let answer: ConfigInfo =
        request(&mut client, GetConfigAll { mode: 0, id: engine.state_id.clone() }).await.unwrap();

    let config_params = ConfigParams::construct_from_bytes(answer.config_proof()).unwrap();
    let param0 = config_params.config(0).unwrap().unwrap();
    let param0 = match param0 {
        ConfigParamEnum::ConfigParam0(param) => param,
        _ => panic!("ConfigParams id bad!"),
    };
    assert_eq!(param0.config_addr, engine.config_addr);

    client.shutdown().await.unwrap();
    control.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_getaccount() {
    struct TestEngine {
        account: Account,
        master_state_id: BlockIdExt,
        master_state: Arc<ShardStateStuff>,
        shard_state_id: BlockIdExt,
        shard_state: Arc<ShardStateStuff>,
    }

    impl TestEngine {
        fn new() -> Self {
            #[cfg(feature = "telemetry")]
            let telemetry = create_engine_telemetry();
            let allocated = create_engine_allocated();

            let account = gen_test_account();
            let (shard_state_id, shard_state) = gen_shard_state(
                None,
                &[&account],
                #[cfg(feature = "telemetry")]
                Some(telemetry.clone()),
                Some(allocated.clone()),
                None,
            );
            let master_state = gen_master_state(
                GenMasterStateParams {
                    shard_state_id: Some(shard_state_id.clone()),
                    ..Default::default()
                },
                #[cfg(feature = "telemetry")]
                Some(telemetry.clone()),
                Some(allocated.clone()),
            );
            let master_state_id = master_state.block_id().clone();
            Self { account, master_state_id, master_state, shard_state_id, shard_state }
        }
    }

    #[async_trait::async_trait]
    impl EngineOperations for TestEngine {
        async fn load_last_applied_mc_state(&self) -> Result<Arc<ShardStateStuff>> {
            Ok(self.master_state.clone())
        }
        fn load_shard_client_mc_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
            Ok(Some(Arc::new(self.master_state_id.clone())))
        }
        async fn load_state(&self, block_id: &BlockIdExt) -> Result<Arc<ShardStateStuff>> {
            if *block_id == self.master_state_id {
                Ok(self.master_state.clone())
            } else if *block_id == self.shard_state_id {
                Ok(self.shard_state.clone())
            } else {
                fail!("Wrong block ID {}", block_id)
            }
        }
        async fn load_and_pin_state(&self, block_id: &BlockIdExt) -> Result<PinnedShardStateGuard> {
            if *block_id == self.master_state_id {
                PinnedShardStateGuard::new(
                    self.master_state.clone(),
                    Arc::new(AllowStateGcSmartResolver::new(10)),
                )
            } else if *block_id == self.shard_state_id {
                PinnedShardStateGuard::new(
                    self.shard_state.clone(),
                    Arc::new(AllowStateGcSmartResolver::new(10)),
                )
            } else {
                fail!("Wrong block ID {}", block_id)
            }
        }

        fn find_full_block_id(&self, root_hash: &UInt256) -> Result<Option<BlockIdExt>> {
            Ok(if *root_hash == self.master_state_id.root_hash {
                Some(self.master_state_id.clone())
            } else if *root_hash == self.shard_state_id.root_hash {
                Some(self.shard_state_id.clone())
            } else {
                None
            })
        }
    }

    init_test_log();
    let engine = Arc::new(TestEngine::new());
    let (control, mut client, config, _) =
        start_control(DataSource::Engine(engine.clone()), "test_getaccount").await.unwrap();

    /* let account1 = AccountAddress {
        account_address: "-1:7777777777777777777777777777777777777777777777777777777777777777".to_string()
    };
    let empty_answer: ShardAccountState = request(
        &mut client, GetShardAccountState {account_address: account1}
    ).await.unwrap();
    println!("{:?}", empty_answer);
    */
    let account2 =
        AccountAddress { account_address: format!("{}", engine.account.get_addr().unwrap()) };
    let answer: ShardAccountState =
        request(&mut client, GetShardAccountState { account_address: account2.clone() })
            .await
            .unwrap();
    assert!(answer.shard_account().is_some());

    let mut client = recreate_client(client, &config).await;
    let answer: ShardAccountMeta =
        request(&mut client, GetShardAccountMeta { account_address: account2.clone() })
            .await
            .unwrap();
    assert!(answer.shard_account_meta().is_some());

    let mut client = recreate_client(client, &config).await;
    let account_id = UInt256::from(engine.account.get_id().unwrap().get_bytestring(0));
    let answer: ShardAccountState = request(
        &mut client,
        GetAccountByBlock {
            account_id: account_id.clone(),
            block_root_hash: engine.shard_state_id.root_hash.clone(),
        },
    )
    .await
    .unwrap();
    assert!(answer.shard_account().is_some());

    let mut client = recreate_client(client, &config).await;
    let answer: ShardAccountMeta = request(
        &mut client,
        GetAccountMetaByBlock {
            account_id,
            block_root_hash: engine.shard_state_id.root_hash.clone(),
        },
    )
    .await
    .unwrap();
    assert!(answer.shard_account_meta().is_some());

    client.shutdown().await.unwrap();
    control.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_get_applied_shards_info() {
    struct TestEngine {
        applied_master_block_id: BlockIdExt,
        applied_shard_block_id: BlockIdExt,
        shard_client_master_state_id: BlockIdExt,
        shard_client_master_state: Arc<ShardStateStuff>,
    }

    impl TestEngine {
        fn new() -> Self {
            #[cfg(feature = "telemetry")]
            let telemetry = create_engine_telemetry();
            let allocated = create_engine_allocated();

            let (applied_shard_block_id, _) = gen_shard_state(
                None,
                &[],
                #[cfg(feature = "telemetry")]
                Some(telemetry.clone()),
                Some(allocated.clone()),
                None,
            );
            let shard_client_master_state = gen_master_state(
                GenMasterStateParams {
                    shard_state_id: Some(applied_shard_block_id.clone()),
                    ..Default::default()
                },
                #[cfg(feature = "telemetry")]
                Some(telemetry.clone()),
                Some(allocated.clone()),
            );
            let shard_client_master_state_id = shard_client_master_state.block_id().clone();

            let mut applied_master_block_id = shard_client_master_state_id.clone();
            applied_master_block_id.seq_no += 1;

            Self {
                applied_master_block_id,
                applied_shard_block_id,
                shard_client_master_state_id,
                shard_client_master_state,
            }
        }
    }

    #[async_trait::async_trait]
    impl EngineOperations for TestEngine {
        fn load_last_applied_mc_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
            Ok(Some(Arc::new(self.applied_master_block_id.clone())))
        }
        fn load_shard_client_mc_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
            Ok(Some(Arc::new(self.shard_client_master_state_id.clone())))
        }
        async fn load_state(&self, block_id: &BlockIdExt) -> Result<Arc<ShardStateStuff>> {
            if *block_id == self.shard_client_master_state_id {
                Ok(self.shard_client_master_state.clone())
            } else {
                fail!("Wrong block ID {}", block_id)
            }
        }
    }

    init_test_log();
    let engine = Arc::new(TestEngine::new());
    let (control, mut client, _, _) =
        start_control(DataSource::Engine(engine.clone()), "test_get_applied_shards_info")
            .await
            .unwrap();

    let answer: AppliedShardsInfo = request(&mut client, GetAppliedShardsInfo {}).await.unwrap();
    assert_eq!(
        answer.shards(),
        &vec![engine.applied_shard_block_id.clone(), engine.applied_master_block_id.clone()]
    );

    client.shutdown().await.unwrap();
    control.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_connect_to_control() {
    struct TestSource;
    impl StatusReporter for TestSource {
        fn get_report(&self) -> u32 {
            0
        }
    }

    init_test_log();
    let (control, mut client, _, _) =
        start_control(DataSource::Status(Arc::new(TestSource)), "test_connect_to_control")
            .await
            .unwrap();

    let key_hash = generate_keypair(&mut client).await.unwrap();
    log::debug!("key hash: {}", base64_encode(key_hash));
    client.shutdown().await.unwrap();
    control.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_connect_to_control_with_import_private_key() {
    struct TestSource;
    impl StatusReporter for TestSource {
        fn get_report(&self) -> u32 {
            0
        }
    }

    init_test_log();
    let (control, mut client, _, _) = start_control(
        DataSource::Status(Arc::new(TestSource)),
        "test_connect_to_control_with_import_private_key",
    )
    .await
    .unwrap();

    let pvt_key = "TcbLvGlM7Dk/F+nHyd4MYwG2ys14YPxgGIdf0xh+Tr8=";
    log::debug!("pvt_key: {pvt_key}");
    let key_hash = import_private_key(&mut client, pvt_key).await.unwrap();
    log::debug!("key_hash: {}", base64_encode(&key_hash));
    assert_eq!(base64_encode(&key_hash), "bWTFYRDLaHY6FlS64a/2hVRfdqudGuQ7+jfz8GKo6M0=");

    let ttl = 36000;
    let election_date = 1763281111;
    let query = AddValidatorPermanentKey { key_hash, election_date, ttl };
    let _answer: Success = request(&mut client, query).await.unwrap();

    let election_date = 1763287777;
    let pvt_key = "5UqVPHm/95e70Q4juONPpBfKmOWM63/zQ4hhnJ86Jwo=";
    let key_hash = import_private_key(&mut client, pvt_key).await.unwrap();

    let permanent_key_hash = key_hash.clone();
    let query = AddValidatorPermanentKey { key_hash, election_date, ttl };
    let _answer: Success = request(&mut client, query).await.unwrap();

    let pvt_key = "cyO76Whw2CKnSbEftGllH53Ph29SHdqzGra9Wb93bOE=";
    let key_hash = import_private_key(&mut client, pvt_key).await.unwrap();

    let query = AddValidatorAdnlAddress { permanent_key_hash, key_hash, ttl };
    let _answer: Success = request(&mut client, query).await.unwrap();

    let answer: JsonConfig = request(&mut client, GetConfig).await.unwrap();

    println!("Config data: {}", answer.data());

    let json = serde_json::from_str::<serde_json::Value>(answer.data()).unwrap();
    assert_eq!(json["adnl"].as_array().unwrap().len(), 3);
    assert_eq!(json["dht"].as_array().unwrap().len(), 1);
    assert_eq!(json["validators"].as_array().unwrap().len(), 2);

    client.shutdown().await.unwrap();
    control.shutdown().await;
}

struct TestSendMsgEngine {
    expected_data: Vec<u8>,
}

#[async_trait::async_trait]
impl EngineOperations for TestSendMsgEngine {
    async fn redirect_external_message(&self, message_data: &[u8]) -> Result<()> {
        assert_eq!(message_data, &self.expected_data);
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_control_send_message() {
    init_test_log();

    let body = Message::default().write_to_bytes().unwrap();
    let engine = TestSendMsgEngine { expected_data: body.clone() };
    let config = TonNodeConfig::from_file(
        "../target",
        "config_test_control.json",
        None,
        DEFAULT_CONFIG,
        None,
    )
    .unwrap();

    let (control, mut client, _, _) = start_control_with_config(
        DataSource::Engine(Arc::new(engine)),
        config,
        "test_control_send_message",
    )
    .await
    .unwrap();

    let _answer: Success =
        request(&mut client, ton::rpc::lite_server::SendMessage { body }).await.unwrap();
    client.shutdown().await.unwrap();
    control.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_control_db_restore() {
    struct TestSource {
        broken: AtomicBool,
    }

    impl StatusReporter for TestSource {
        fn get_report(&self) -> u32 {
            if self.broken.load(Ordering::Relaxed) {
                Engine::SYNC_STATUS_DB_BROKEN
            } else {
                Engine::SYNC_STATUS_CHECKING_DB
            }
        }
    }

    init_test_log();
    let status = Arc::new(TestSource { broken: AtomicBool::new(false) });
    let (control, mut client, _, _) =
        start_control(DataSource::Status(status.clone()), "test_control_db_restore").await.unwrap();

    let answer: Stats =
        request(&mut client, GetSelectedStats { filter: "node_status".to_string() }).await.unwrap();
    let answer = answer.only();
    let answer = &answer.stats.deref()[0];
    assert_eq!(answer.key, "node_status");
    assert_eq!(answer.value, "\"checking_db\"");

    status.broken.store(true, Ordering::Relaxed);

    let answer: Stats =
        request(&mut client, GetSelectedStats { filter: "node_status".to_string() }).await.unwrap();
    let answer = answer.only();
    let answer = &answer.stats.deref()[0];
    assert_eq!(answer.key, "node_status");
    assert_eq!(answer.value, "\"db_broken\"");
    client.shutdown().await.unwrap();
    control.shutdown().await;
}

#[test]
fn test_convert_for_stats() {
    assert_eq!("Disabled", &format!("{:?}", ValidationStatus::from_u8(5)));
    assert_eq!("Disabled", &format!("{:?}", ValidationStatus::from_u8(0)));
    assert_eq!("Waiting", &format!("{:?}", ValidationStatus::from_u8(1)));
    assert_eq!("Active", &format!("{:?}", ValidationStatus::from_u8(2)));

    let shard_id = ShardIdent::with_tagged_prefix(15, 0xABCD_0000_0000_0000u64).unwrap();
    let root_hash =
        "bac24be401b3489f90018d08137c4063f24bfc6def86a61836060d6dbc32e703".parse().unwrap();
    let file_hash =
        "3baf367e57116fcf5df3c7333a7ea4aa5704dac36e696b4c7dfbda383babe9ae".parse().unwrap();
    let block_id = BlockIdExt::with_params(shard_id.clone(), 100500, root_hash, file_hash);
    assert_eq!(
        r#"{"shard":"15:abcd000000000000","seq_no":100500,"rh":"bac24be401b3489f90018d08137c4063f24bfc6def86a61836060d6dbc32e703","fh":"3baf367e57116fcf5df3c7333a7ea4aa5704dac36e696b4c7dfbda383babe9ae"}"#,
        ControlQuerySubscriber::block_id_to_json(&block_id).as_str()
    );

    let now = UnixTime::now();
    let map = lockfree::map::Map::new();
    map.insert(ShardIdent::masterchain(), now - 5);
    assert_eq!(
        "{\n  \"-1:8000000000000000\": 5\n}",
        ControlQuerySubscriber::statistics_to_json(&map, now as i64, true).as_str()
    );
    let map = lockfree::map::Map::new();
    map.insert(shard_id, now - 130);
    assert_eq!(
        "{\n  \"15:abcd000000000000\": 130\n}",
        ControlQuerySubscriber::statistics_to_json(&map, now as i64, true).as_str()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_stats() {
    const DB_PATH: &str = "../target/multi_node_db";

    struct TestEngine {
        db: InternalDb,
        master_state_id: BlockIdExt,
        master_state: Arc<ShardStateStuff>,
        last_validation_time: lockfree::map::Map<ShardIdent, u64>,
    }

    impl TestEngine {
        async fn new() -> Self {
            #[cfg(feature = "telemetry")]
            let telemetry = create_engine_telemetry();
            let allocated = create_engine_allocated();
            let master_state = gen_master_state(
                Default::default(),
                #[cfg(feature = "telemetry")]
                Some(telemetry.clone()),
                Some(allocated.clone()),
            );
            let master_state_id = master_state.block_id().clone();
            remove_dir_all(DB_PATH).ok();
            let db_config =
                InternalDbConfig { db_directory: String::from(DB_PATH), ..Default::default() };
            let db = InternalDb::with_update(
                db_config,
                false,
                false,
                false,
                &|| Ok(()),
                None,
                Arc::new(AtomicU8::new(0)),
                None,
                #[cfg(feature = "telemetry")]
                telemetry.clone(),
                allocated.clone(),
            )
            .await
            .unwrap();
            db.create_or_load_block_handle(&master_state_id, None, Some(1), None)
                .unwrap()
                ._to_created()
                .unwrap();
            Self {
                db,
                master_state_id,
                master_state,
                last_validation_time: lockfree::map::Map::new(),
            }
        }
    }

    #[async_trait::async_trait]
    impl EngineOperations for TestEngine {
        fn calc_tps(&self, _period: u64) -> Result<u32> {
            Ok(0)
        }
        fn get_sync_status(&self) -> u32 {
            Engine::SYNC_STATUS_SYNC_BLOCKS
        }
        fn last_collation_time(&self) -> &lockfree::map::Map<ShardIdent, u64> {
            &self.last_validation_time
        }
        fn last_validation_time(&self) -> &lockfree::map::Map<ShardIdent, u64> {
            &self.last_validation_time
        }
        fn load_block_handle(&self, id: &BlockIdExt) -> Result<Option<Arc<BlockHandle>>> {
            self.db.load_block_handle(id)
        }
        fn load_last_applied_mc_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
            Ok(Some(Arc::new(self.master_state_id.clone())))
        }
        async fn load_last_applied_mc_state(&self) -> Result<Arc<ShardStateStuff>> {
            Ok(self.master_state.clone())
        }
        fn load_shard_client_mc_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
            Ok(Some(Arc::new(self.master_state_id.clone())))
        }
        fn validation_status(&self) -> ValidationStatus {
            ValidationStatus::Active
        }
    }

    struct Ethalon<'a> {
        val: &'a str,
        mask: u32,
    }

    fn add_ethalon<'a>(map: &mut HashMap<&'a str, Ethalon<'a>>, key: &'a str, val: &'a str) {
        let ethalon = Ethalon { val, mask: 1u32 << map.iter().count() };
        map.insert(key, ethalon);
    }

    fn value_ok(val: &str, ethalon: &str) -> Option<()> {
        if val != ethalon {
            None
        } else {
            Some(())
        }
    }

    fn check_stats(stats: &Stats, engine: &Arc<TestEngine>, key_id: &Arc<KeyId>, new_format: bool) {
        let master_block_id = ControlQuerySubscriber::block_id_to_json(&engine.master_state_id);
        let node_version = format!("\"{}\"", env!("CARGO_PKG_VERSION"));
        let overlay_key = format!("\"{}\"", key_id);
        let supported_capabilities = format!("{}", supported_capabilities());
        let supported_version = format!("{}", supported_version());
        let timediff = UnixTime::now();
        let mut ethalon_stats = HashMap::new();
        if !new_format {
            add_ethalon(&mut ethalon_stats, "collation_stats", "{}");
        }
        if new_format {
            add_ethalon(&mut ethalon_stats, "global_id", "0");
        }
        add_ethalon(&mut ethalon_stats, "in_current_vset_p34", "false");
        add_ethalon(&mut ethalon_stats, "in_next_vset_p36", "false");
        add_ethalon(&mut ethalon_stats, "last_applied_masterchain_block_id", &master_block_id);
        if new_format {
            add_ethalon(&mut ethalon_stats, "last_collation_ago_sec", "{}");
            add_ethalon(&mut ethalon_stats, "last_validation_ago_sec", "{}");
        }
        add_ethalon(&mut ethalon_stats, "masterchainblocknumber", "0");
        add_ethalon(&mut ethalon_stats, "masterchainblocktime", "1");
        if new_format {
            add_ethalon(&mut ethalon_stats, "node_status", "\"synchronization_by_blocks\"");
        }
        add_ethalon(&mut ethalon_stats, "node_version", &node_version);
        add_ethalon(&mut ethalon_stats, "public_overlay_key_id", &overlay_key);
        add_ethalon(&mut ethalon_stats, "shards_timediff", "timediff");
        if new_format {
            add_ethalon(&mut ethalon_stats, "supported_block", &supported_version);
            add_ethalon(&mut ethalon_stats, "supported_capabilities", &supported_capabilities);
        }
        if !new_format {
            add_ethalon(&mut ethalon_stats, "sync_status", "\"synchronization_by_blocks\"");
        }
        add_ethalon(&mut ethalon_stats, "timediff", "timediff");
        add_ethalon(&mut ethalon_stats, "tps_10", "0");
        add_ethalon(&mut ethalon_stats, "tps_300", "0");
        if !new_format {
            add_ethalon(&mut ethalon_stats, "validation_stats", "{}");
        }
        add_ethalon(&mut ethalon_stats, "validation_status", "\"Active\"");

        let mut mask = u32::MAX >> (32 - ethalon_stats.len());
        for stat in stats.stats().deref() {
            let ethalon = ethalon_stats
                .get(&stat.key as &str)
                .unwrap_or_else(|| panic!("Key {} is not found in Stats ethalon", stat.key));
            if (mask & ethalon.mask) == 0 {
                panic!("Doubled stat {} in Stat reply", stat.key)
            } else {
                mask &= !ethalon.mask
            }
            match ethalon.val {
                "timediff" => {
                    let ethalon = format!("{}", timediff);
                    value_ok(&stat.value, &ethalon)
                        .or_else(|| {
                            let ethalon = format!("{}", timediff - 1);
                            value_ok(&stat.value, &ethalon)
                        })
                        .or_else(|| {
                            let ethalon = format!("{}", timediff + 1);
                            value_ok(&stat.value, &ethalon)
                        })
                }
                _ => value_ok(&stat.value, ethalon.val),
            }
            .unwrap_or_else(|| {
                panic!(
                    "Value for key {} does not match: {}, expected {}",
                    stat.key, stat.value, ethalon.val
                )
            })
        }

        if mask != 0 {
            panic!("Some stats ({:x}) did not found in Stat reply", mask)
        }
    }

    async fn test() -> Result<()> {
        init_test_log();
        let engine = Arc::new(TestEngine::new().await);
        let (control, mut client, _, key_id) =
            start_control(DataSource::Engine(engine.clone()), "test_stats").await?;

        let answer: Stats = request(&mut client, GetStats).await?;
        check_stats(&answer, &engine, &key_id, false);

        let answer: Stats =
            request(&mut client, GetSelectedStats { filter: "*".to_string() }).await?;
        check_stats(&answer, &engine, &key_id, true);

        client.shutdown().await?;
        control.shutdown().await;
        Ok(())
    }

    test_async(
        || Box::pin(test()),
        || {
            remove_dir_all(DB_PATH).ok();
        },
    )
    .await;
}
