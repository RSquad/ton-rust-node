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
    collator_test_bundle::create_engine_allocated,
    config::{JsonRpcServerConfig, JsonRpcServerConfigJson},
    confirmed_blocks::{ConfirmedBlockEvent, ConfirmedBlockEvents, ConfirmedBlockSource},
    engine_traits::{EngineOperations, Stoppable},
    internal_db::state_gc_resolver::AllowStateGcSmartResolver,
    rpc_server::{
        confirmed_block_events_handler, jsonrpc_handler, rest_ok, wallets::WalletLibrary,
        ConfirmedBlockEventsQuery, Ctx, JsonRpcRequest, RpcRegistry, RpcServer,
    },
    shard_state::ShardStateStuff,
    shard_states_keeper::PinnedShardStateGuard,
    test_helper::{gen_master_state, gen_test_account, GenMasterStateParams},
};
use http_body_util::BodyExt;
use std::{collections::HashMap, sync::Arc};
use ton_block::{
    base64_decode, base64_encode, error, read_single_root_boc, Account, AccountIdPrefixFull,
    BlockIdExt, BuilderData, ConfigParam8, ConfigParamEnum, ConfigParams, Deserializable,
    GlobalVersion, HashmapE, LibDescr, Libraries, MsgAddressInt, Result, ShardIdent, UInt256,
};
use warp::Reply;

struct MockEngine {
    last_mc_block_id: Arc<BlockIdExt>,
    zerostate_id: BlockIdExt,
    states: HashMap<BlockIdExt, Arc<ShardStateStuff>>,
    lookup_by_seqno: HashMap<(AccountIdPrefixFull, u32), (BlockIdExt, Vec<u8>)>,
    confirmed_block_events: ConfirmedBlockEvents,
    gc_resolver: Arc<AllowStateGcSmartResolver>,
}

impl MockEngine {
    fn new(states: Vec<Arc<ShardStateStuff>>) -> Self {
        assert!(!states.is_empty(), "mock engine requires at least one state");
        let last_state_id = states[0].block_id().clone();
        let mut state_map = HashMap::new();
        for state in states {
            state_map.insert(state.block_id().clone(), state);
        }
        Self {
            last_mc_block_id: Arc::new(last_state_id.clone()),
            zerostate_id: last_state_id,
            states: state_map,
            lookup_by_seqno: HashMap::new(),
            confirmed_block_events: ConfirmedBlockEvents::new(),
            gc_resolver: Arc::new(AllowStateGcSmartResolver::new(u64::MAX)),
        }
    }

    fn insert_lookup_by_seqno(
        &mut self,
        prefix: AccountIdPrefixFull,
        seqno: u32,
        block_id: BlockIdExt,
        block_data: Vec<u8>,
    ) {
        self.lookup_by_seqno.insert((prefix, seqno), (block_id, block_data));
    }

    fn get_state(&self, block_id: &BlockIdExt) -> Result<Arc<ShardStateStuff>> {
        println!("get_state {:?}", block_id);
        self.states
            .get(block_id)
            .cloned()
            .ok_or_else(|| error!("state {block_id} not found in mock engine"))
    }

    fn confirmed_block_events(&self) -> ConfirmedBlockEvents {
        self.confirmed_block_events.clone()
    }
}

#[async_trait::async_trait]
impl EngineOperations for MockEngine {
    fn load_last_applied_mc_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
        println!("load_last_applied_mc_block_id");
        Ok(Some(self.last_mc_block_id.clone()))
    }

    async fn load_last_applied_mc_state(&self) -> Result<Arc<ShardStateStuff>> {
        println!("load_last_applied_mc_state");
        self.states.get(&self.last_mc_block_id).cloned().ok_or_else(|| {
            error!("last mc state {} not found in mock engine", self.last_mc_block_id)
        })
    }

    fn load_shard_client_mc_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
        Ok(None)
    }

    fn zerostate_id(&self) -> Result<&BlockIdExt> {
        println!("zerostate_id");
        Ok(&self.zerostate_id)
    }

    async fn load_state(&self, block_id: &BlockIdExt) -> Result<Arc<ShardStateStuff>> {
        println!("load_state");
        self.get_state(block_id)
    }

    async fn load_and_pin_state(&self, block_id: &BlockIdExt) -> Result<PinnedShardStateGuard> {
        println!("load_and_pin_state");
        let state = self.get_state(block_id)?;
        PinnedShardStateGuard::new(state, self.gc_resolver.clone())
    }

    async fn redirect_external_message(&self, _message_data: &[u8]) -> Result<()> {
        Ok(())
    }

    async fn lookup_block_by_seqno(
        &self,
        prefix: &AccountIdPrefixFull,
        seqno: u32,
    ) -> Result<Option<(BlockIdExt, Vec<u8>)>> {
        Ok(self.lookup_by_seqno.get(&(prefix.clone(), seqno)).cloned())
    }

    fn confirmed_block_events(&self) -> Option<ConfirmedBlockEvents> {
        Some(self.confirmed_block_events())
    }
}

fn ctx_with_engine(engine: Arc<dyn EngineOperations>) -> Ctx {
    Ctx { engine, wallet_library: Arc::new(WalletLibrary::new().unwrap()) }
}

fn make_master_state(account: &Account) -> Arc<ShardStateStuff> {
    let mut config = ConfigParams::default();
    let global_version = GlobalVersion { version: 17, capabilities: 0x1ee };
    config.set_config(ConfigParamEnum::ConfigParam8(ConfigParam8 { global_version })).unwrap();

    let publisher = account.get_id().unwrap().clone();
    let mut libraries = Libraries::new();
    let code = BuilderData::with_raw(vec![0x77], 1).unwrap().into_cell().unwrap(); // PUSHINT 1
    let key = code.repr_hash().clone();
    println!("key1: {}", serialize_uint256(&key));
    libraries.set(&key, &LibDescr::from_lib_data_by_publisher(code, publisher.clone())).unwrap();
    let code = BuilderData::with_raw(vec![0x78], 2).unwrap().into_cell().unwrap(); // PUSHINT 2
    let key = code.repr_hash().clone();
    println!("key2: {}", serialize_uint256(&key));
    libraries.set(&key, &LibDescr::from_lib_data_by_publisher(code, publisher.clone())).unwrap();

    let state = gen_master_state(
        GenMasterStateParams { config, libraries, accounts: &[account], ..Default::default() },
        #[cfg(feature = "telemetry")]
        Some(create_engine_telemetry()),
        Some(create_engine_allocated()),
    );
    state
}

fn account_address(account: &Account) -> String {
    MsgAddressInt::with_standart(None, -1, account.get_id().unwrap().clone()).unwrap().to_string()
}

fn build_registry(account: &Account) -> (RpcRegistry, Arc<ShardStateStuff>) {
    let master_state = make_master_state(account);
    let engine: Arc<dyn EngineOperations> = Arc::new(MockEngine::new(vec![master_state.clone()]));
    let (registry, _) = RpcRegistry::with_context(ctx_with_engine(engine));
    (registry, master_state)
}

fn dummy_ctx() -> Ctx {
    let account = gen_test_account();
    let master_state = make_master_state(&account);
    let engine: Arc<dyn EngineOperations> = Arc::new(MockEngine::new(vec![master_state]));
    ctx_with_engine(engine)
}

async fn call_jsonrpc(
    registry: &RpcRegistry,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: method.to_string(),
        params: Some(params),
        id: Some(serde_json::json!(1)),
    };
    let reply = jsonrpc_handler(request, registry.clone()).await.expect("jsonrpc call");
    response_to_json(reply.into_response()).await
}

async fn response_to_json(response: warp::reply::Response) -> serde_json::Value {
    let body = response.into_body();
    let collected = body.collect().await.expect("response body");
    serde_json::from_slice(&collected.to_bytes()).expect("response json")
}

#[tokio::test(flavor = "current_thread")]
async fn get_masterchain_info_returns_state_metadata() {
    let account = gen_test_account();
    let master_state = make_master_state(&account);
    let engine: Arc<dyn EngineOperations> = Arc::new(MockEngine::new(vec![master_state.clone()]));
    let ctx = ctx_with_engine(engine);

    let response = get_masterchain_info(NoParams {}, ctx).await.unwrap();

    pretty_assertions::assert_eq!(response["@type"], "blocks.masterchainInfo");
    pretty_assertions::assert_eq!(response["last"], serialize_block_id(master_state.block_id()));
    // init mirrors zerostate id but with shard rendered as the literal "0" (toncenter parity)
    let mut expected_init = serialize_block_id(master_state.block_id());
    expected_init
        .as_object_mut()
        .unwrap()
        .insert("shard".to_string(), serde_json::Value::String("0".to_string()));
    pretty_assertions::assert_eq!(response["init"], expected_init);
    pretty_assertions::assert_eq!(
        response["state_root_hash"],
        serialize_uint256(&master_state.root_cell().repr_hash())
    );
}

#[tokio::test(flavor = "current_thread")]
async fn get_address_information_reads_account_state() {
    let account = gen_test_account();
    let master_state = make_master_state(&account);
    let engine: Arc<dyn EngineOperations> = Arc::new(MockEngine::new(vec![master_state.clone()]));
    let ctx = ctx_with_engine(engine);
    let params = GetAddressInformationParams { address: account_address(&account), seqno: None };

    let response = get_address_information(params, ctx).await.unwrap();

    pretty_assertions::assert_eq!(response["@type"], "raw.fullAccountState");
    pretty_assertions::assert_eq!(
        response["balance"],
        serde_json::json!(account.balance().unwrap().coins.to_string())
    );
    pretty_assertions::assert_eq!(response["state"], "active");
    pretty_assertions::assert_eq!(
        response["block_id"],
        serialize_block_id(master_state.block_id())
    );
    pretty_assertions::assert_eq!(
        response["code"],
        serde_json::json!(serialize_cell_opt(account.code()))
    );
    pretty_assertions::assert_eq!(
        response["data"],
        serde_json::json!(serialize_cell_opt(account.data()))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn rest_get_address_information() {
    let account = gen_test_account();
    let master_state = make_master_state(&account);
    let engine: Arc<dyn EngineOperations> = Arc::new(MockEngine::new(vec![master_state.clone()]));
    let ctx = ctx_with_engine(engine);
    let params = GetAddressInformationParams { address: account_address(&account), seqno: None };

    let handler_result = get_address_information(params, ctx).await.unwrap();
    let body = response_to_json(rest_ok(handler_result)).await;
    pretty_assertions::assert_eq!(body["ok"], serde_json::Value::Bool(true));
    let result = &body["result"];
    pretty_assertions::assert_eq!(result["@type"], serde_json::json!("raw.fullAccountState"));
    pretty_assertions::assert_eq!(result["state"], serde_json::json!("active"));
    pretty_assertions::assert_eq!(result["block_id"], serialize_block_id(&master_state.block_id()));
    assert!(result["balance"].as_str().is_some());
    assert!(body["@extra"].as_str().is_some());
    assert!(result["last_transaction_id"].is_object());
}

#[tokio::test(flavor = "current_thread")]
async fn jsonrpc_get_address_information() {
    let account = gen_test_account();
    let (registry, master_state) = build_registry(&account);

    let response = call_jsonrpc(
        &registry,
        "getAddressInformation",
        serde_json::json!({ "address": account_address(&account) }),
    )
    .await;

    pretty_assertions::assert_eq!(response["ok"], serde_json::Value::Bool(true));
    assert!(response["@extra"].as_str().is_some());
    let result = &response["result"];
    pretty_assertions::assert_eq!(result["@type"], serde_json::json!("raw.fullAccountState"));
    pretty_assertions::assert_eq!(result["block_id"], serialize_block_id(master_state.block_id()));
    assert!(result["balance"].as_str().is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn jsonrpc_get_account_returns_boc() {
    let account = gen_test_account();
    let (registry, _) = build_registry(&account);
    let address = account_address(&account);

    let response =
        call_jsonrpc(&registry, "getAccount", serde_json::json!({ "address": address })).await;

    pretty_assertions::assert_eq!(response["ok"], serde_json::Value::Bool(true));
    let expected = get_account(
        GetAddressInformationParams { address: account_address(&account), seqno: None },
        registry.ctx.clone(),
    )
    .await
    .unwrap();
    pretty_assertions::assert_eq!(response["result"], expected);
}

#[tokio::test(flavor = "current_thread")]
async fn get_block_accepts_string_shard_in_jsonrpc() {
    let account = gen_test_account();
    let master_state = make_master_state(&account);
    let mut engine = MockEngine::new(vec![master_state]);

    let shard_i64 = i64::MIN;
    let shard_u64 = shard_i64 as u64;
    let prefix = AccountIdPrefixFull::workchain(-1, shard_u64);
    let seqno = 45262950u32;
    let block_id = BlockIdExt::with_params(
        ShardIdent::with_tagged_prefix(-1, shard_u64).unwrap(),
        seqno,
        UInt256::default(),
        UInt256::default(),
    );
    let block_data = vec![1, 2, 3, 4, 5];
    engine.insert_lookup_by_seqno(prefix.clone(), seqno, block_id.clone(), block_data.clone());

    let ctx = ctx_with_engine(Arc::new(engine));
    let (registry, _) = RpcRegistry::with_context(ctx);
    let rpc_response = call_jsonrpc(
        &registry,
        "getBlock",
        serde_json::json!({
            "workchain": -1,
            "shard": shard_i64.to_string(),
            "seqno": seqno,
            "root_hash": "",
            "file_hash": "",
        }),
    )
    .await;
    pretty_assertions::assert_eq!(rpc_response["ok"], serde_json::Value::Bool(true));
    let response = rpc_response["result"].clone();

    pretty_assertions::assert_eq!(response["id"], serialize_block_id(&block_id));
    pretty_assertions::assert_eq!(response["boc"], serde_json::json!(base64_encode(&block_data)));
}

#[tokio::test(flavor = "current_thread")]
async fn jsonrpc_get_block_not_found_returns_404() {
    let account = gen_test_account();
    let (registry, _) = build_registry(&account);

    let response = call_jsonrpc(
        &registry,
        "getBlock",
        serde_json::json!({
            "workchain": 0,
            "shard": "4611686018427387904",
            "seqno": 0
        }),
    )
    .await;

    pretty_assertions::assert_eq!(response["ok"], serde_json::Value::Bool(false));
    pretty_assertions::assert_eq!(response["code"], serde_json::json!(404));
    let message = response["error"].as_str().unwrap_or_default();
    assert!(message.contains("block not found"));
    assert!(message.contains("zerostate"));
}

#[tokio::test(flavor = "current_thread")]
async fn jsonrpc_send_boc() {
    let account = gen_test_account();
    let (registry, _) = build_registry(&account);

    let response =
        call_jsonrpc(&registry, "sendBoc", serde_json::json!({ "boc": "aGVsbG8=" })).await;

    pretty_assertions::assert_eq!(response["ok"], serde_json::Value::Bool(true));
    assert!(response["@extra"].as_str().is_some());
    let result = &response["result"];
    pretty_assertions::assert_eq!(result["@type"], serde_json::json!("ok"));
}

#[tokio::test(flavor = "current_thread")]
async fn detect_address_from_friendly() {
    let ctx = dummy_ctx();
    let params = DetectAddressParams {
        address: "EQCD39VS5jcptHL8vMjEXrzGaRcCVYto7HUn4bpAOg8xqB2N".to_string(),
    };

    let response = detect_address(params, ctx).await.unwrap();

    pretty_assertions::assert_eq!(
        response,
        serde_json::json!({
            "raw_form": "0:83dfd552e63729b472fcbcc8c45ebcc6691702558b68ec7527e1ba403a0f31a8",
            "bounceable": {
                "b64": "EQCD39VS5jcptHL8vMjEXrzGaRcCVYto7HUn4bpAOg8xqB2N",
                "b64url": "EQCD39VS5jcptHL8vMjEXrzGaRcCVYto7HUn4bpAOg8xqB2N",
            },
            "non_bounceable": {
                "b64": "UQCD39VS5jcptHL8vMjEXrzGaRcCVYto7HUn4bpAOg8xqEBI",
                "b64url": "UQCD39VS5jcptHL8vMjEXrzGaRcCVYto7HUn4bpAOg8xqEBI",
            },
            "given_type": "friendly_bounceable",
            "testnet": false,
        })
    );
}

#[tokio::test(flavor = "current_thread")]
async fn detect_address_from_raw() {
    let ctx = dummy_ctx();
    let params = DetectAddressParams {
        address: "0:83dfd552e63729b472fcbcc8c45ebcc6691702558b68ec7527e1ba403a0f31a8".to_string(),
    };

    let response = detect_address(params, ctx).await.unwrap();

    pretty_assertions::assert_eq!(
        response,
        serde_json::json!({
            "raw_form": "0:83dfd552e63729b472fcbcc8c45ebcc6691702558b68ec7527e1ba403a0f31a8",
            "bounceable": {
                "b64": "EQCD39VS5jcptHL8vMjEXrzGaRcCVYto7HUn4bpAOg8xqB2N",
                "b64url": "EQCD39VS5jcptHL8vMjEXrzGaRcCVYto7HUn4bpAOg8xqB2N",
            },
            "non_bounceable": {
                "b64": "UQCD39VS5jcptHL8vMjEXrzGaRcCVYto7HUn4bpAOg8xqEBI",
                "b64url": "UQCD39VS5jcptHL8vMjEXrzGaRcCVYto7HUn4bpAOg8xqEBI",
            },
            "given_type": "raw_form",
            "testnet": false,
        })
    );
}

#[tokio::test(flavor = "current_thread")]
async fn pack_address_handles_friendly_and_raw() {
    let ctx = dummy_ctx();
    let friendly = "EQCD39VS5jcptHL8vMjEXrzGaRcCVYto7HUn4bpAOg8xqB2N".to_string();
    let raw = "0:83dfd552e63729b472fcbcc8c45ebcc6691702558b68ec7527e1ba403a0f31a8".to_string();

    let packed_from_friendly =
        pack_address(DetectAddressParams { address: friendly.clone() }, ctx.clone()).await.unwrap();
    pretty_assertions::assert_eq!(packed_from_friendly, serde_json::json!(friendly));

    let packed_from_raw =
        pack_address(DetectAddressParams { address: raw }, ctx.clone()).await.unwrap();
    pretty_assertions::assert_eq!(packed_from_raw, serde_json::json!(friendly));
}

#[tokio::test(flavor = "current_thread")]
async fn unpack_address_handles_friendly_and_raw() {
    let ctx = dummy_ctx();
    let friendly = "EQCD39VS5jcptHL8vMjEXrzGaRcCVYto7HUn4bpAOg8xqB2N".to_string();
    let raw = "0:83dfd552e63729b472fcbcc8c45ebcc6691702558b68ec7527e1ba403a0f31a8".to_string();

    let unpacked_from_friendly =
        unpack_address(DetectAddressParams { address: friendly }, ctx.clone()).await.unwrap();
    pretty_assertions::assert_eq!(unpacked_from_friendly, serde_json::json!(raw));

    let unpacked_from_raw =
        unpack_address(DetectAddressParams { address: raw.clone() }, ctx).await.unwrap();
    pretty_assertions::assert_eq!(unpacked_from_raw, serde_json::json!(raw));
}

fn prepare_http_server_with_config(
) -> (JsonRpcServerConfig, Arc<dyn EngineOperations>, Account, std::net::SocketAddr) {
    let account = gen_test_account();
    let master_state = make_master_state(&account);
    let engine: Arc<dyn EngineOperations> = Arc::new(MockEngine::new(vec![master_state.clone()]));
    let json_config = JsonRpcServerConfigJson { address: "127.0.0.1:18082".to_string() };
    let config = JsonRpcServerConfig::from_json_config(&json_config).unwrap();
    let httpaddr = config.address.clone();
    (config, engine, account, httpaddr)
}

async fn prepare_http_server_auto(
) -> (tokio::net::TcpListener, Arc<dyn EngineOperations>, Account, std::net::SocketAddr) {
    let account = gen_test_account();
    let master_state = make_master_state(&account);
    let engine: Arc<dyn EngineOperations> = Arc::new(MockEngine::new(vec![master_state.clone()]));
    let listener = tokio::net::TcpListener::bind("localhost:0").await.expect("failed to bind");
    let httpaddr = listener.local_addr().expect("failed to get local addr");
    (listener, engine, account, httpaddr)
}

#[tokio::test(flavor = "current_thread")]
async fn http_test_get() {
    let (listener, engine, account, httpaddr) = prepare_http_server_auto().await;
    let accaddr = account.get_addr().unwrap().clone();
    let server = Box::new(RpcServer::start_with_listener(listener, engine).await.unwrap());
    http_server_test_client_get(httpaddr, accaddr).await;
    server.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn http_test_jsonrpc() {
    let (listener, engine, account, httpaddr) = prepare_http_server_auto().await;
    let accaddr = account.get_addr().unwrap().clone();
    let server = Box::new(RpcServer::start_with_listener(listener, engine).await.unwrap());
    http_server_test_client_jsonrpc(httpaddr.clone(), accaddr.clone()).await;
    server.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn confirmed_block_events_handler_streams_block_data() {
    let account = gen_test_account();
    let master_state = make_master_state(&account);
    let engine = MockEngine::new(vec![master_state]);
    let events = engine.confirmed_block_events();
    let block_id = BlockIdExt::with_params(
        ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).unwrap(),
        77,
        UInt256::from([2; 32]),
        UInt256::from([3; 32]),
    );
    let block_data = vec![9, 8, 7];
    let block_id_2 = BlockIdExt::with_params(
        ShardIdent::with_tagged_prefix(0, 0xc000_0000_0000_0000).unwrap(),
        78,
        UInt256::from([4; 32]),
        UInt256::from([5; 32]),
    );
    let block_data_2 = vec![6, 5, 4];
    let engine: Arc<dyn EngineOperations> = Arc::new(engine);
    let ctx = ctx_with_engine(engine);
    let response = confirmed_block_events_handler(
        ConfirmedBlockEventsQuery { include_data: None, limit: Some(2) },
        ctx,
    )
    .await
    .expect("SSE handler failed");

    events.notify(ConfirmedBlockEvent {
        id: block_id.clone(),
        data: Arc::new(block_data.clone()),
        source: ConfirmedBlockSource::PRE_APPLIED,
    });
    events.notify(ConfirmedBlockEvent {
        id: block_id_2.clone(),
        data: Arc::new(block_data_2.clone()),
        source: ConfirmedBlockSource::PRE_APPLIED,
    });

    let body = read_sse_response_body(response).await;
    assert!(body.contains("confirmed_block"));
    let payloads = sse_payloads(&body);
    pretty_assertions::assert_eq!(payloads.len(), 2);

    pretty_assertions::assert_eq!(payloads[0]["status"], serde_json::json!("confirmed"));
    pretty_assertions::assert_eq!(
        payloads[0]["block"]["@type"],
        serde_json::json!("liteServer.blockData")
    );
    pretty_assertions::assert_eq!(payloads[0]["block"]["id"], serialize_block_id(&block_id));
    pretty_assertions::assert_eq!(
        payloads[0]["block"]["data"],
        serde_json::json!(base64_encode(&block_data))
    );
    pretty_assertions::assert_eq!(payloads[1]["block"]["id"], serialize_block_id(&block_id_2));
    pretty_assertions::assert_eq!(
        payloads[1]["block"]["data"],
        serde_json::json!(base64_encode(&block_data_2))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn confirmed_block_events_handler_can_omit_block_data() {
    let account = gen_test_account();
    let master_state = make_master_state(&account);
    let engine = MockEngine::new(vec![master_state]);
    let events = engine.confirmed_block_events();
    let block_id = BlockIdExt::with_params(
        ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).unwrap(),
        79,
        UInt256::from([6; 32]),
        UInt256::from([7; 32]),
    );
    let engine: Arc<dyn EngineOperations> = Arc::new(engine);
    let ctx = ctx_with_engine(engine);
    let response = confirmed_block_events_handler(
        ConfirmedBlockEventsQuery { include_data: Some(false), limit: Some(1) },
        ctx,
    )
    .await
    .expect("SSE handler failed");

    events.notify(ConfirmedBlockEvent {
        id: block_id.clone(),
        data: Arc::new(Vec::new()),
        source: ConfirmedBlockSource::PRE_APPLIED,
    });

    let body = read_sse_response_body(response).await;
    let payloads = sse_payloads(&body);
    pretty_assertions::assert_eq!(payloads.len(), 1);
    pretty_assertions::assert_eq!(payloads[0]["status"], serde_json::json!("confirmed"));
    pretty_assertions::assert_eq!(payloads[0]["block"]["id"], serialize_block_id(&block_id));
    assert!(payloads[0]["block"].get("data").is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn confirmed_block_events_handler_rejects_zero_limit() {
    let account = gen_test_account();
    let master_state = make_master_state(&account);
    let engine: Arc<dyn EngineOperations> = Arc::new(MockEngine::new(vec![master_state]));
    let ctx = ctx_with_engine(engine);

    let response = confirmed_block_events_handler(
        ConfirmedBlockEventsQuery { include_data: None, limit: Some(0) },
        ctx,
    )
    .await
    .expect("SSE handler failed");

    pretty_assertions::assert_eq!(response.status(), warp::http::StatusCode::BAD_REQUEST);
}

async fn read_sse_response_body(response: warp::reply::Response) -> String {
    let collected =
        tokio::time::timeout(std::time::Duration::from_secs(3), response.into_body().collect())
            .await
            .expect("SSE response timed out")
            .expect("SSE response failed");
    String::from_utf8(collected.to_bytes().to_vec()).expect("SSE body must be UTF-8")
}

fn sse_payloads(body: &str) -> Vec<serde_json::Value> {
    body.lines()
        .filter(|line| line.starts_with("data:"))
        .map(|line| line.trim_start_matches("data:").trim_start())
        .map(|data| serde_json::from_str(data).expect("SSE data must be JSON"))
        .collect()
}

async fn http_server_test_client_jsonrpc(address: std::net::SocketAddr, _account: MsgAddressInt) {
    //wait a little while rpc_server gets ready
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let url = format!("http://{}/jsonRPC", address);
    let client = reqwest::Client::new();
    let res = client
        .post(url)
        .body(
            serde_json::to_string(&serde_json::json!(
                {
                    "id":1,
                    "jsonrpc":"2.0",
                    "method":"getMasterchainInfo",
                    "params":{}
                }
            ))
            .unwrap(),
        )
        .send()
        .await
        .unwrap();
    let response: serde_json::Value = serde_json::from_str(&res.text().await.unwrap()).unwrap();
    pretty_assertions::assert_eq!(response["ok"], serde_json::Value::Bool(true));
    let response = &response["result"];
    pretty_assertions::assert_eq!(response["@type"], serde_json::json!("blocks.masterchainInfo"));
    println!("JSONRPC response {:?}", response);
    // tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
}

#[tokio::test(flavor = "current_thread")]
async fn http_test_jsonrpc_bad_req() {
    let (listener, engine, account, httpaddr) = prepare_http_server_auto().await;
    let accaddr = account.get_addr().unwrap().clone();
    let server = Box::new(RpcServer::start_with_listener(listener, engine).await.unwrap());
    http_server_test_client_jsonrpc_bad_request(httpaddr, accaddr).await;
    server.shutdown().await;
}

async fn http_server_test_client_jsonrpc_bad_request(
    address: std::net::SocketAddr,
    _account: MsgAddressInt,
) {
    //wait a little while rpc_server gets ready
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let url = format!("http://{}/jsonRPC", address);
    let client = reqwest::Client::new();
    let res = client
        .post(url)
        .body(
            serde_json::to_string(&serde_json::json!(
                {
                    "id":1,
                    "jsonrpc":"2.0",
                    "method":"getAddressInformation",
                    "params":{}
                }
            ))
            .unwrap(),
        )
        .send()
        .await
        .unwrap();
    println!("res {:?}", res);
    pretty_assertions::assert_eq!(res.status(), 422);
    let response: serde_json::Value = serde_json::from_str(&res.text().await.unwrap()).unwrap();
    println!("repsonse {:?}", response);
    pretty_assertions::assert_eq!(response["error"].is_string(), true);
}

#[tokio::test]
async fn test_get_config_param() {
    let account = gen_test_account();
    let (registry, _master_state) = build_registry(&account);
    let ctx = registry.ctx;

    let p = r#"{"config_id":8}"#;
    let response = get_config_param(serde_json::from_str(p).unwrap(), ctx.clone()).await.unwrap();
    println!("response {response:#}");
    pretty_assertions::assert_eq!(response["@type"].as_str().unwrap(), "configInfo");
    let config = response["config"].as_object().unwrap();
    pretty_assertions::assert_eq!(config["@type"], "tvm.cell");
    pretty_assertions::assert_eq!(config["bytes"], "te6ccgEBAQEADwAAGsQAAAARAAAAAAAAAe4=");

    // for absent parameters, empty cell is returned
    let p = r#"{"config_id":999}"#;
    let response = get_config_param(serde_json::from_str(p).unwrap(), ctx).await.unwrap();
    println!("response {response:#}");
    pretty_assertions::assert_eq!(response["@type"].as_str().unwrap(), "configInfo");
    let config = response["config"].as_object().unwrap();
    pretty_assertions::assert_eq!(config["@type"], "tvm.cell");
    pretty_assertions::assert_eq!(config["bytes"], "");
}

#[tokio::test]
async fn test_get_libraries() {
    let account = gen_test_account();
    let (registry, _master_state) = build_registry(&account);
    let ctx = registry.ctx;

    let key0 = "uikYyJR+myWvmsG4gzV3VBc+WBL4B6PW5kKhRwlZU5U="; // 0xba2918c8947e9b25af9ac1b883357754173e5812f807a3d6e642a14709595395
    let key1 = "kK7Illr6uxbrw8ubQI665xthjXh4i8gNCYQ1k8rJjaQ=";
    let key2 = "CZCDtqBfRTwghRphsmS2QF4w1cmDATOEntZt4Yk+RVQ=";
    let p = serde_json::json!({
        "libraries":["ba2918c8947e9b25af9ac1b883357754173e5812f807a3d6e642a14709595395", key0]
    });
    println!("p {p:#}");
    let response = get_libraries(serde_json::from_value(p).unwrap(), ctx.clone()).await.unwrap();
    println!("response {response:#}");
    pretty_assertions::assert_eq!(response["@type"].as_str().unwrap(), "smc.libraryResult");
    let libraries = response["result"].as_array().unwrap();
    pretty_assertions::assert_eq!(libraries.len(), 0);

    let p = serde_json::json!({"libraries":[key1]});
    println!("p {p:#}");
    let response = get_libraries(serde_json::from_value(p).unwrap(), ctx.clone()).await.unwrap();
    println!("response {response:#}");
    pretty_assertions::assert_eq!(response["@type"].as_str().unwrap(), "smc.libraryResult");
    let libraries = response["result"].as_array().unwrap();
    pretty_assertions::assert_eq!(libraries.len(), 1);
    pretty_assertions::assert_eq!(libraries[0]["@type"], "smc.libraryEntry");
    pretty_assertions::assert_eq!(libraries[0]["hash"], key1);
    pretty_assertions::assert_eq!(libraries[0]["data"], "te6ccgEBAQEAAwAAAUA=");

    let p = serde_json::json!({"libraries":[key1, key2]});
    println!("p {p:#}");
    let response = get_libraries(serde_json::from_value(p).unwrap(), ctx.clone()).await.unwrap();
    println!("response {response:#}");
    pretty_assertions::assert_eq!(response["@type"].as_str().unwrap(), "smc.libraryResult");
    let libraries = response["result"].as_array().unwrap();
    pretty_assertions::assert_eq!(libraries.len(), 2);
    pretty_assertions::assert_eq!(libraries[0]["@type"], "smc.libraryEntry");
    pretty_assertions::assert_eq!(libraries[0]["hash"], key1);
    pretty_assertions::assert_eq!(libraries[0]["data"], "te6ccgEBAQEAAwAAAUA=");
    pretty_assertions::assert_eq!(libraries[1]["@type"], "smc.libraryEntry");
    pretty_assertions::assert_eq!(libraries[1]["hash"], key2);
    pretty_assertions::assert_eq!(libraries[1]["data"], "te6ccgEBAQEAAwAAAWA=");

    let p = serde_json::json!({"libraries":[]});
    println!("p {p:#}");
    let response = get_libraries(serde_json::from_value(p).unwrap(), ctx.clone()).await.unwrap();
    println!("response {response:#}");
    pretty_assertions::assert_eq!(response["@type"].as_str().unwrap(), "smc.libraryResult");
    let libraries = response["result"].as_array().unwrap();
    pretty_assertions::assert_eq!(libraries.len(), 0);
}

#[tokio::test]
async fn test_get_libraries_ext() {
    let account = gen_test_account();
    let (registry, master_state) = build_registry(&account);

    let response = call_jsonrpc(&registry, "getLibrariesExt", serde_json::json!({})).await;
    pretty_assertions::assert_eq!(response["ok"], serde_json::Value::Bool(true));
    assert!(response["@extra"].as_str().is_some());

    let result = response["result"].clone();
    pretty_assertions::assert_eq!(result["@type"], serde_json::json!("smc.libraryResultExt"));
    pretty_assertions::assert_eq!(result["block_id"], serialize_block_id(master_state.block_id()));
    pretty_assertions::assert_eq!(result["libraries_count"], serde_json::json!(2));

    let dict_boc = result["dict_boc"].as_str().unwrap();
    assert!(!dict_boc.is_empty());

    let root = read_single_root_boc(base64_decode(dict_boc).unwrap()).unwrap();
    let raw_libraries = HashmapE::with_hashmap(256, Some(root));
    let source_libraries = master_state.state().unwrap().libraries();

    source_libraries
        .iterate_slices_with_keys(|mut key, mut value| -> Result<bool> {
            let hash = UInt256::construct_from(&mut key)?;
            let descr = LibDescr::construct_from(&mut value)?;
            let bucket = raw_libraries.get(hash.clone().into())?.expect("library entry must exist");
            let lib = bucket.reference(0).expect("raw dict value must point to code");
            pretty_assertions::assert_eq!(*lib.repr_hash(), hash);
            pretty_assertions::assert_eq!(lib, descr.lib().clone());
            Ok(true)
        })
        .unwrap();
}

#[tokio::test]
async fn test_parse_int() {
    let p: IntOrStr = serde_json::from_str("-9223372036854775808").unwrap();
    pretty_assertions::assert_eq!(p.as_i64().unwrap(), -9223372036854775808);
}

#[tokio::test(flavor = "current_thread")]
async fn http_test_jsonrpc_bad_url() {
    let (listener, engine, _account, httpaddr) = prepare_http_server_auto().await;
    let server = Box::new(RpcServer::start_with_listener(listener, engine).await.unwrap());
    http_server_test_client_jsonrpc_bad_url(httpaddr).await;
    server.shutdown().await;
}

async fn http_server_test_client_jsonrpc_bad_url(address: std::net::SocketAddr) {
    //wait a little while rpc_server gets ready
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let url = format!("http://{}/jsonRPC11", address);
    let client = reqwest::Client::new();
    let res = client
        .post(url)
        .body(
            serde_json::to_string(&serde_json::json!(
                {
                    "id":1,
                    "jsonrpc":"2.0",
                    "method":"getAddressInformation",
                    "params":{}
                }
            ))
            .unwrap(),
        )
        .send()
        .await
        .unwrap();
    println!("res {:?}", res);
    pretty_assertions::assert_eq!(res.status(), 404);
}

#[tokio::test(flavor = "current_thread")]
async fn http_test_jsonrpc_bad_content_type() {
    let (listener, engine, _account, httpaddr) = prepare_http_server_auto().await;

    let server = Box::new(RpcServer::start_with_listener(listener, engine).await.unwrap());
    tokio::task::yield_now().await;

    let url = format!("http://{}/jsonRPC", httpaddr);
    let client = reqwest::Client::new();
    let s: String = "{}".to_string();
    let res = client.post(url).body(s).send().await.unwrap();
    println!("res {:?}", res);
    pretty_assertions::assert_eq!(res.status(), 422);
    server.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn http_server_test_with_config() {
    let (config, engine, account, httpaddr) = prepare_http_server_with_config();
    let accaddr = account.get_addr().unwrap().clone();
    let server = Box::new(RpcServer::start(config, engine).await.unwrap());
    http_server_test_client_get(httpaddr.clone(), accaddr.clone()).await;
    server.shutdown().await;
}

async fn http_server_test_client_get(address: std::net::SocketAddr, _account: MsgAddressInt) {
    //wait a little while rpc_server gets ready
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let url = format!("http://{}/getMasterchainInfo", address);
    let client = reqwest::Client::new();
    let res = client.get(url).send().await.unwrap();
    let response: serde_json::Value = serde_json::from_str(&res.text().await.unwrap()).unwrap();
    pretty_assertions::assert_eq!(response["ok"], serde_json::Value::Bool(true));
    let response = &response["result"];
    pretty_assertions::assert_eq!(response["@type"], serde_json::json!("blocks.masterchainInfo"));
    println!("HTTP GET response {:?}", response);
}

#[tokio::test]
async fn test_boc_hash() {
    let boc1_b64 = "te6cckEBAQEAcQAA3v8AIN0gggFMl7ohggEznLqxn3Gw7UTQ0x/THzHXC//jBOCk8mCDCNcYINMf0x/TH/gjE7vyY+1E0NMf0x/T/9FRMrryoVFEuvKiBPkBVBBV+RDyo/gAkyDXSpbTB9QC+wDo0QGkyMsfyx/L/8ntVBC9ba0=";
    let boc2_b64 = "te6ccgEBAQEAcQAA3v8AIN0gggFMl7ohggEznLqxn3Gw7UTQ0x/THzHXC//jBOCk8mCDCNcYINMf0x/TH/gjE7vyY+1E0NMf0x/T/9FRMrryoVFEuvKiBPkBVBBV+RDyo/gAkyDXSpbTB9QC+wDo0QGkyMsfyx/L/8ntVA==";
    let boc1 = read_single_root_boc(base64_decode(boc1_b64).unwrap()).unwrap();
    let boc2 = read_single_root_boc(base64_decode(boc2_b64).unwrap()).unwrap();
    let hash1 = boc1.repr_hash().clone();
    let hash2 = boc2.repr_hash().clone();
    println!("Hash1 {:?}", hash1);
    println!("Hash2 {:?}", hash2);
    pretty_assertions::assert_eq!(hash1, hash2);
}

/* RPC API examples:
GET getAddressInformation address:string, [seqno: integer]
curl 'http://127.0.0.1:8083/getAddressInformation?address=Ef9wG0C7iZ3F_TTf9WxYZf8hCffFbDhMhvxJbOZYjyuBEeDN'
{"ok":true,"result":{"@type":"raw.fullAccountState","balance":0,"extra_currencies":[],"code":"","data":"","last_transaction_id":{"@type":"internal.transactionId","lt":"0","hash":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="},"block_id":{"@type":"ton.blockIdExt","workchain":-1,"shard":"-9223372036854775808","seqno":199473,"root_hash":"xu1BQjCKdrVgE/ZKPTUHp8z8RYdk0tGzcOxffki9hMI=","file_hash":"KpCa0jV0TMWCDffOWSrFrqM+v/gLXjQVgQeE4T2o+/E="},"frozen_hash":"","sync_utime":1762349089,"@extra":"1762349102.9697325:0:0.4365522111831328","state":"uninitialized"}}
curl -X 'POST' 'http://127.0.0.1:8083/jsonRPC' -d '{"id":"1","jsonrpc":"2.0","method":"getAddressInformation","params":{"address":"Ef9wG0C7iZ3F_TTf9WxYZf8hCffFbDhMhvxJbOZYjyuBEeDN"}}' -HContent-type:\ application/json
{"ok":true,"result":{"@type":"raw.fullAccountState","balance":0,"extra_currencies":[],"code":"","data":"","last_transaction_id":{"@type":"internal.transactionId","lt":"0","hash":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="},"block_id":{"@type":"ton.blockIdExt","workchain":-1,"shard":"-9223372036854775808","seqno":199447,"root_hash":"eOBuFzt1J942scIPGKqvJB+VRCcG5PkMVZHSaI2FRDg=","file_hash":"bzYLFT3+bw2iH3VsehNf55ebbfNFO3XIwO7n7sMClWs="},"frozen_hash":"","sync_utime":1762349033,"@extra":"1762349047.5678587:0:0.24929099535909038","state":"uninitialized"},"jsonrpc":"2.0","id":"1"}


GET getTransactions address:string, [limit: integer], [lt:integer], [hash:string], [to_lt:integer], [archival:bool]
POST runGetMethod

curl -X 'POST' 'http://127.0.0.1:8083/jsonRPC' -d '{"id":"1","jsonrpc":"2.0","method":"runGetMethod","params":{"address":"EQAOtrk3eOGwebP9KMNVSmIGo3mA0bN1e12SiBf0fgkWDEj8","method":"get_jetton_data","stack":[]}}' -HContent-type:\ application/json
{"ok":true,"result":{"@type":"smc.runResult","gas_used":5679,"stack":[["num","0x5af3107a4000"],["num","-0x1"],["cell",{"bytes":"te6cckEBAQEAJAAAQ4AN94XMMzx2uAY3ActSwgwtPjcAQ7kjl4y8CK6aqVuXv/DS1hZm","object":{"data":{"b64":"gA33hcwzPHa4BjcBy1LCDC0+NwBDuSOXjLwIrpqpW5e/4A==","len":267},"refs":[],"special":false}}],["cell",{"bytes":"te6cckEBBwEAbAABAwDAAQIBIAIDAUO/+HLr21FNnJfCg7fwrlF5Ap4rYRnDlGJxnk9G7Y90E+ZABAFDv/dAfpePAaQHEUEbGst3Opa92T+oO7XKhDUBPIxLOskfQAYBAgAFABxodHRwczovL2RvbWFpbgAEADnMkxnB","object":{"data":{"b64":"AIA=","len":9},"refs":[{"data":{"b64":"AA==","len":2},"refs":[{"data":{"b64":"v/hy69tRTZyXwoO38K5ReQKeK2EZw5RicZ5PRu2PdBPmAA==","len":265},"refs":[{"data":{"b64":"AA==","len":8},"refs":[{"data":{"b64":"aHR0cHM6Ly9kb21haW4=","len":112},"refs":[],"special":false}],"special":false}],"special":false},{"data":{"b64":"v/dAfpePAaQHEUEbGst3Opa92T+oO7XKhDUBPIxLOskfAA==","len":265},"refs":[{"data":{"b64":"ADk=","len":16},"refs":[],"special":false}],"special":false}],"special":false}],"special":false}}],["cell",{"bytes":"te6cckEBAQEAIwAIQgK6KRjIlH6bJa+awbiDNXdUFz5YEvgHo9bmQqFHCVlTlSN648M=","object":{"data":{"b64":"AropGMiUfpslr5rBuIM1d1QXPlgS+Aej1uZCoUcJWVOV","len":264},"refs":[],"special":true}}]],"exit_code":0,"@extra":"1762348895.9547586:0:0.1817755467858576","block_id":{"@type":"ton.blockIdExt","workchain":-1,"shard":"-9223372036854775808","seqno":199378,"root_hash":"p3hgBImhz4mwrzmRN48dJ9IJyqLIGSFz3y8HNctDYYQ=","file_hash":"8U4MFB3fSB91tDEJNDpmvuf/F1YadHGEx8h3A/Ghz9g="},"last_transaction_id":{"@type":"internal.transactionId","lt":"673594000001","hash":"H7Ra7Zb4N1xv+lycGluWqAHh9qnh1D1dcxjQP9PHLYU="}},"jsonrpc":"2.0","id":"1"}


curl -X 'POST' \
  'http://127.0.0.1:8083/runGetMethod' \
  -H 'accept: application/json' \
  -H 'Content-Type: application/json' \
  -d '{ "address": "EQAOtrk3eOGwebP9KMNVSmIGo3mA0bN1e12SiBf0fgkWDEj8", "method": "get_jetton_data", "stack": [ ] }'
{"ok":true,"result":{"@type":"smc.runResult","gas_used":5679,"stack":[["num","0x5af3107a4000"],["num","-0x1"],["cell",{"bytes":"te6cckEBAQEAJAAAQ4AN94XMMzx2uAY3ActSwgwtPjcAQ7kjl4y8CK6aqVuXv/DS1hZm","object":{"data":{"b64":"gA33hcwzPHa4BjcBy1LCDC0+NwBDuSOXjLwIrpqpW5e/4A==","len":267},"refs":[],"special":false}}],["cell",{"bytes":"te6cckEBBwEAbAABAwDAAQIBIAIDAUO/+HLr21FNnJfCg7fwrlF5Ap4rYRnDlGJxnk9G7Y90E+ZABAFDv/dAfpePAaQHEUEbGst3Opa92T+oO7XKhDUBPIxLOskfQAYBAgAFABxodHRwczovL2RvbWFpbgAEADnMkxnB","object":{"data":{"b64":"AIA=","len":9},"refs":[{"data":{"b64":"AA==","len":2},"refs":[{"data":{"b64":"v/hy69tRTZyXwoO38K5ReQKeK2EZw5RicZ5PRu2PdBPmAA==","len":265},"refs":[{"data":{"b64":"AA==","len":8},"refs":[{"data":{"b64":"aHR0cHM6Ly9kb21haW4=","len":112},"refs":[],"special":false}],"special":false}],"special":false},{"data":{"b64":"v/dAfpePAaQHEUEbGst3Opa92T+oO7XKhDUBPIxLOskfAA==","len":265},"refs":[{"data":{"b64":"ADk=","len":16},"refs":[],"special":false}],"special":false}],"special":false}],"special":false}}],["cell",{"bytes":"te6cckEBAQEAIwAIQgK6KRjIlH6bJa+awbiDNXdUFz5YEvgHo9bmQqFHCVlTlSN648M=","object":{"data":{"b64":"AropGMiUfpslr5rBuIM1d1QXPlgS+Aej1uZCoUcJWVOV","len":264},"refs":[],"special":true}}]],"exit_code":0,"@extra":"1762348809.6002166:0:0.7746039522506065","block_id":{"@type":"ton.blockIdExt","workchain":-1,"shard":"-9223372036854775808","seqno":199339,"root_hash":"e2UfE1UD6aOS9/VoMfoZO3sBd7eiFF874qr8Bht4wGE=","file_hash":"0JjmvduRSafREjyhHn46lM5K389EFXEohf1+/WLQs3A="},"last_transaction_id":{"@type":"internal.transactionId","lt":"673594000001","hash":"H7Ra7Zb4N1xv+lycGluWqAHh9qnh1D1dcxjQP9PHLYU="}}}




sendBoc
lookupBlock

{"id":"1","jsonrpc":"2.0","method":"sendBoc","params":{"boc":"te6cckEBAgEArgAB34n/zDml+LUxbX2Mjawrx1Mu70zRfjxkQTJOjtmb3oBKm/4GBR6R666UpGdkI5QKyJhl9LpOWk6CEfYuaZ6s5TOcrCBTKNF8De1NTmu97xvKOVK4MzO0iPMY1rT/NJeVCBawIAAAAVNIMSpAAAABOBwBAHJCf6GWGIKQRmy5J2vwgC/47QYZ6tB3TL/ThNaSVlJ5t0RfqBKgXyAAAAAAAAAAAAAAAAAAAAAAAABJtbKU"}}
{"id":"1","jsonrpc":"2.0","method":"getAddressInformation","params":{"address":"Ef9DLDEFIIzZck7X4QBf8doMM9Wg7pl_pwmtJKyk826Iv8yJ"}}

{"id":"1","jsonrpc":"2.0","method":"sendBoc","params":{"boc":"te6cckEBAgEArgAB34n+hlhiCkEZsuSdr8IAv+O0GGerQd0y/04TWklZSebdEX4Hl1xsq05UCIiY9VwAeLfOs5zg2tkhqR4GjEHtpYO7Ob4hnh/RBYTsHR43zJKNLINB09kh1WS6G1W6WylQDlIoIAAAAVNIMS4AAAAACBwBAHJCf/MOaX4tTFtfYyNrCvHUy7vTNF+PGRBMk6O2ZvegEqb/qBKgXyAAAAAAAAAAAAAAAAAAAAAAAAA8aLAk"}}
{"ok":true,"result":{"@type":"ok","@extra":"1762010510.805682:0:0.1897749877717545"},"jsonrpc":"2.0","id":"1"}


curl -sS -X POST http://127.0.0.1:8083/jsonRPC   -H 'Content-Type: application/json'   -d '{
    "jsonrpc":"2.0",
    "id":1,
    "method":"getAddressInformation",
    "params":{"address":"Ef9wG0C7iZ3F_TTf9WxYZf8hCffFbDhMhvxJbOZYjyuBEeDN"}
  }'
{"ok":true,"result":{"@type":"raw.fullAccountState","balance":0,"extra_currencies":[],"code":"","data":"","last_transaction_id":{"@type":"internal.transactionId","lt":"0","hash":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="},"block_id":{"@type":"ton.blockIdExt","workchain":-1,"shard":"-9223372036854775808","seqno":1222,"root_hash":"zQwPCY+igXv5ffPbyKw0uZAJrYRov7hZiNZDdoA9qCA=","file_hash":"cqTg6ycNgUQAD6++ItOkOKo0pj73aD+695D0UarjXjw="},"frozen_hash":"","sync_utime":1760953103,"@extra":"1760953115.6070707:0:0.669076554920487","state":"uninitialized"},"jsonrpc":"2.0","id":"1"}

*/
