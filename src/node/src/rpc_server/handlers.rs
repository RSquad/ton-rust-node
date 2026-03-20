/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    block::{get_block_proof, get_last_liteserver_state_block, get_shard_block_proof, BlockStuff},
    rpc_server::{
        serializers::{
            serialize_block_id, serialize_cell_opt, serialize_shard_account, serialize_stack,
            serialize_transaction, serialize_uint256, RPCStackEntry,
        },
        token::{
            parse_jetton_master_data, parse_jetton_wallet_data, parse_nft_collection,
            parse_nft_item_data,
        },
        ApiError, Ctx, JsonResult, RpcRegistryBuilder,
    },
    shard_states_keeper::PinnedShardStateGuard,
};
use std::{str::FromStr, sync::Arc};
use ton_api::ton::{lite_server::BlockLink, tvm::StackEntry, Bool};
use ton_block::{
    address_crc, base64_decode, base64_decode_url_safe, base64_encode, error, fail,
    read_single_root_boc, ton_method_id, write_boc, Account, AccountIdPrefixFull, AccountStatus,
    Block, BlockIdExt, Cell, Coins, Deserializable, ExtBlkRef, ExternalInboundMessageHeader,
    HashmapAugType, HashmapType, KeyExtBlkRef, Message, MsgAddressInt, Result, Serializable,
    ShardAccount, ShardIdent, SliceData, StateInit, StorageUsageCalc, Transaction,
    TransactionDescr, UInt256, UnixTime, ADDR_FORMAT_BOUNCE, ADDR_FORMAT_TESTNET,
    ADDR_FORMAT_URL_SAFE,
};
use ton_executor::{
    BlockchainConfig, ExecuteParams, OrdinaryTransactionExecutor, TransactionExecutor,
};
use ton_vm::{executor::BehaviorModifiers, smart_contract_info::PrevBlocksInfo};

// ---------- [ Add routes here ] -----------
// handlers should be implemented at the end of file or in external file
pub(crate) fn register(registry: &mut RpcRegistryBuilder) {
    // -- [ accounts ] --
    registry.add_jsonrpc("getAddressInformation", get_address_information, true);
    registry.add_jsonrpc("getExtendedAddressInformation", get_extended_address_information, true);
    registry.add_jsonrpc("getWalletInformation", get_wallet_information, true);
    registry.add_jsonrpc("getTransactions", get_transactions, true);
    registry.add_jsonrpc("getAddressBalance", get_address_balance, true);
    registry.add_jsonrpc("getAddressState", get_address_state, true);
    registry.add_jsonrpc("packAddress", pack_address, true);
    registry.add_jsonrpc("unpackAddress", unpack_address, true);
    registry.add_jsonrpc("getTokenData", get_token_data, true);
    registry.add_jsonrpc("detectAddress", detect_address, true);

    // -- [ blocks ] --
    registry.add_jsonrpc("getMasterchainInfo", get_masterchain_info, true);
    registry.add_jsonrpc("getMasterchainBlockSignatures", get_masterchain_block_signatures, true);
    registry.add_jsonrpc("getConsensusBlock", get_consensus_block, true);
    registry.add_jsonrpc("getShardBlockProof", get_shard_proof, true);
    registry.add_jsonrpc("lookupBlock", lookup_block, true);
    registry.add_jsonrpc("shards", get_shards, true);
    registry.add_jsonrpc("getBlockTransactions", get_block_transactions, true);
    registry.add_jsonrpc("getBlockTransactionsExt", get_block_transactions_ext, true);
    registry.add_jsonrpc("getBlockHeader", get_block_header, true);
    registry.add_jsonrpc("getBlock", get_block, true);
    registry.add_jsonrpc("getOutMsgQueueSizes", get_out_msg_queue_sizes, true);

    // -- [ transactions ] --
    registry.add_jsonrpc("tryLocateTx", try_locate_result_tx, true);
    registry.add_jsonrpc("tryLocateResultTx", try_locate_result_tx, true);
    registry.add_jsonrpc("tryLocateSourceTx", try_locate_source_tx, true);

    // -- [ get config ] --
    registry.add_jsonrpc("getConfigParam", get_config_param, true);
    registry.add_jsonrpc("getLibraries", get_libraries, true);

    // -- [ run method ] --
    registry.add_jsonrpc("runGetMethod", run_get_method, false);

    // -- [ send ] --
    registry.add_jsonrpc("sendBoc", send_boc, false);
    registry.add_jsonrpc("sendBocReturnHash", send_boc_return_hash, false);
    registry.add_jsonrpc("sendQuery", send_query, false);
    registry.add_jsonrpc("estimateFee", estimate_fee, false);

    // Node-specific JSON-RPC extensions
    registry.add_jsonrpc("getAccount", get_account, true);
    registry.add_jsonrpc("_stack", run_stack_test, false);
}

fn read_single_root_boc_with_bytes_from_base64(boc: &str, param: &str) -> Result<(Cell, Vec<u8>)> {
    let bytes = base64_decode(boc)
        .map_err(|e| ApiError::bad_request(format!("Invalid base64 {param} BOC: {e}")))?;
    let cell = read_single_root_boc(&bytes).map_err(|e| {
        ApiError::bad_request(format!("Invalid {param} BOC (single-root required): {e}"))
    })?;
    Ok((cell, bytes))
}

fn read_single_root_boc_from_base64(boc: &str, param: &str) -> Result<Cell> {
    let (cell, _) = read_single_root_boc_with_bytes_from_base64(boc, param)?;
    Ok(cell)
}

async fn get_mc_state_id(ctx: &Ctx, mc_seq_no: Option<u32>) -> Result<BlockIdExt> {
    let mc_block_id = get_last_liteserver_state_block(&ctx.engine)?;
    let mc_state = ctx.engine.load_and_pin_state(&mc_block_id).await?;
    let mc_block_id = if let Some(mc_seq_no) = mc_seq_no {
        mc_state.state().find_block_id(mc_seq_no)?
    } else {
        (*mc_block_id).clone()
    };
    Ok(mc_block_id)
}

struct AccountContext {
    address: MsgAddressInt,
    mc_block_id: BlockIdExt,
    mc_state: PinnedShardStateGuard,
    #[allow(dead_code)] // store for pin state
    acc_state: PinnedShardStateGuard,
    shard_account: ShardAccount,
}

impl AccountContext {
    async fn with_address(ctx: &Ctx, address: &str, mc_seq_no: Option<u32>) -> Result<Self> {
        let address = parse_address(address)?;
        let workchain_id = address.workchain_id();
        let mc_block_id = get_mc_state_id(ctx, mc_seq_no).await?;
        let mc_state = ctx.engine.load_and_pin_state(&mc_block_id).await?;
        let acc_state = if address.is_masterchain() {
            mc_state.clone()
        } else {
            let prefix = AccountIdPrefixFull::prefix(&address)?;
            let top_id = mc_state
                .state()
                .shards()
                .map_err(|e| error!("failed to get shards for workchain {workchain_id}: {e}"))?
                .find_shard_by_prefix(&prefix)?
                .ok_or_else(|| error!("no shard found for address {address}"))?
                .block_id;
            ctx.engine.load_and_pin_state(&top_id).await?
        };
        // it could be not found in last state - so we can prepare fake
        let shard_account = acc_state
            .state()
            .shard_account(address.address())
            .map_err(|e| error!("failed to get account {address} from shard state: {e}",))?
            .unwrap_or_default();

        Ok(AccountContext { address, mc_state, acc_state, mc_block_id, shard_account })
    }

    fn read_account(&self) -> Result<Account> {
        self.shard_account
            .read_account()
            .map_err(|e| error!("failed to read account {}: {e}", self.address))
    }

    fn mc_state_root_cell(&self) -> Cell {
        self.mc_state.state().root_cell().clone()
    }

    fn last_transaction(&self) -> (u64, UInt256) {
        (self.shard_account.last_trans_lt(), self.shard_account.last_trans_hash().clone())
    }
}

const MAX_TRANSACTION_COUNT: u32 = 16;

#[derive(Debug, serde::Deserialize)]
struct GetTransactionsParams {
    address: String,
    limit: Option<u32>,
    lt: Option<u64>,
    hash: Option<String>,
    _to_lt: Option<u64>,
    _archival: Option<bool>,
}

async fn get_transactions(p: GetTransactionsParams, ctx: Ctx) -> JsonResult {
    if let Some(x) = p.limit {
        //Mimic toncenter behaviour
        if x > 100 {
            fail!(ApiError::Mimic("Ensure limit value is less than or equal to 100".to_string()))
        }
    };
    let acc_ctx = AccountContext::with_address(&ctx, &p.address, None).await?;
    let workchain_id = acc_ctx.address.workchain_id();
    let account_id = acc_ctx.address.address();
    let mut remaining = p.limit.unwrap_or(10).min(MAX_TRANSACTION_COUNT);
    let (mut lt, mut expected_hash) = match p.lt.zip(p.hash) {
        Some((lt, hash)) => (lt, hash.parse()?),
        None => acc_ctx.last_transaction(),
    };

    let mut export = Vec::new();

    // prefix for index by lt
    let prefix = AccountIdPrefixFull::prefix(&acc_ctx.address)?;

    'main: while remaining != 0 && lt != 0 {
        // abort_getTransactions: if you haven't found anything yet, it's an error; otherwise, we finish with a partial result
        let Some((block_id, data)) = ctx.engine.lookup_block_by_lt(&prefix, lt).await? else {
            break;
        };
        // sanity: the block must cover our address
        if !block_id.shard().contains_full_prefix(&prefix) {
            fail!(
                "obtained a block {block_id} that cannot contain specified account {account_id:x}"
            )
        }

        // deserialize strictly with the correct id — otherwise there will be a `wrong root hash`
        let block_stuff = BlockStuff::deserialize_block(block_id.clone(), Arc::new(data))?;

        let Some(acc_block) = block_stuff.get_account(account_id)? else {
            fail!("block with id: {block_id} does not contain account: {account_id:x}")
        };
        let mut found_any_in_this_block = false;
        // search for a transaction with exactly the right lt
        while let Some(slice) = acc_block.transactions().get_as_slice(&lt)? {
            let tr_cell = slice.reference(0)?;
            // check hash if exist
            if !expected_hash.is_zero() && tr_cell.repr_hash() != expected_hash {
                fail!("transaction hash mismatch: prev_trans_lt/hash invalid for wc={workchain_id}, lt={lt}")
            }

            // unpacking and monotony prev_trans_lt
            let tr = Transaction::construct_from_cell(tr_cell.clone())?;
            if tr.prev_trans_lt() >= lt {
                fail!("previous transaction time is not less than the current one")
            }
            found_any_in_this_block = true;
            if remaining == 0 {
                break 'main;
            }

            // Step back up the chain
            lt = tr.prev_trans_lt();
            if lt == 0 {
                break 'main;
            }
            expected_hash = tr.prev_trans_hash().clone();
            // saving the found transaction
            export.push(serialize_transaction(&tr, tr_cell, &p.address)?);
            remaining -= 1;
        }
        // exact-behaivor: block by lt, by tx not
        if !found_any_in_this_block {
            break;
        }
        // continue cycle: new lt already set prev_trans_lt
    }
    if export.is_empty() {
        fail!("cannot compute block with specified transaction: no block by lt={lt}")
    }
    Ok(serde_json::json!(export))
}

#[derive(Debug, serde::Deserialize)]
struct NoParams {}

async fn get_consensus_block(_: NoParams, ctx: Ctx) -> JsonResult {
    let Some(mc_block_id) = &ctx.engine.load_last_applied_mc_block_id()? else {
        fail!("Cannot load load_last_applied_mc_block_id")
    };
    Ok(serde_json::json!({
        "consensus_block": mc_block_id.seq_no(),
        "timestamp": UnixTime::now_f64(),
    }))
}

async fn get_masterchain_info(_: NoParams, ctx: Ctx) -> JsonResult {
    let mc_block_id = get_last_liteserver_state_block(&ctx.engine)?;
    let mc_state = ctx.engine.load_state(&mc_block_id).await?;
    let state_root_hash = mc_state.root_cell().repr_hash();
    let zerostate_block: BlockIdExt = ctx.engine.zerostate_id()?.clone();
    Ok(serde_json::json!({
        "@type": "blocks.masterchainInfo",
        "last": serialize_block_id(&mc_block_id),
        "state_root_hash": serialize_uint256(&state_root_hash),
        "init": serialize_block_id(&zerostate_block),
        "@extra": extra(0),
    }))
}

fn account_status(s: &AccountStatus) -> &'static str {
    match s {
        AccountStatus::AccStateNonexist => "uninitialized",
        AccountStatus::AccStateUninit => "uninitialized",
        AccountStatus::AccStateActive => "active",
        AccountStatus::AccStateFrozen => "frozen",
    }
}

#[derive(Debug, serde::Deserialize)]
struct GetAddressInformationParams {
    address: String,
    #[serde(default)]
    seqno: Option<u32>,
}

async fn get_account(p: GetAddressInformationParams, ctx: Ctx) -> JsonResult {
    let acc_ctx = AccountContext::with_address(&ctx, &p.address, p.seqno).await?;
    let boc_bytes = write_boc(&acc_ctx.shard_account.account_cell())?;
    Ok(serde_json::json!(base64_encode(&boc_bytes)))
}

async fn get_extended_address_information(p: GetAddressInformationParams, ctx: Ctx) -> JsonResult {
    let acc_ctx = AccountContext::with_address(&ctx, &p.address, p.seqno).await?;
    let account = acc_ctx.read_account()?;

    let balance = account.balance().cloned().unwrap_or_default();
    let frozen_hash =
        account.frozen_hash().map_or(String::new(), |h| serialize_uint256(h).to_string());

    let result = serde_json::json!({
          "@type": "fullAccountState",
          "address": {
            "@type": "accountAddress",
            "account_address": p.address,
          },
          "balance": balance.coins.to_string(),
          "extra_currencies": [],
          "last_transaction_id": serialize_shard_account(&acc_ctx.shard_account),
          "block_id": serialize_block_id(&acc_ctx.mc_block_id),
          "sync_utime": account.last_paid(),
          "account_state": {
            "@type": "raw.accountState",
            "code": serialize_cell_opt(account.code()),
            "data": serialize_cell_opt(account.data()),
            "frozen_hash": frozen_hash,
          },
          "revision": 0,
          "@extra": extra(0),
    });

    Ok(result)
}

async fn get_address_state(p: GetAddressInformationParams, ctx: Ctx) -> JsonResult {
    let acc_ctx = AccountContext::with_address(&ctx, &p.address, p.seqno).await?;
    let account = acc_ctx.read_account()?;
    let state = account_status(&account.status());
    Ok(serde_json::json!(state))
}

async fn get_wallet_information(p: GetAddressInformationParams, ctx: Ctx) -> JsonResult {
    let acc_ctx = AccountContext::with_address(&ctx, &p.address, p.seqno).await?;
    let account = acc_ctx.read_account()?;

    let balance = account.balance().cloned().unwrap_or_default();
    let state = account_status(&account.status());

    let mut result = serde_json::Map::new();
    result.insert("wallet".into(), serde_json::Value::Bool(false));
    result.insert("balance".into(), serde_json::Value::String(balance.coins.to_string()));
    result.insert("extra_currencies".into(), serde_json::Value::Array(Vec::new()));
    result.insert("account_state".into(), serde_json::Value::String(state.into()));
    result.insert("last_transaction_id".into(), serialize_shard_account(&acc_ctx.shard_account));

    /*
    {
        "wallet": true,
        "balance": "9645905685270316",
        "extra_currencies": [],
        "account_state": "active",
        "wallet_type": "wallet v3 r2",
        "seqno": 160,
        "last_transaction_id": {
          "@type": "internal.transactionId",
          "lt": "2963494000003",
          "hash": "EQfSgyENDUidzQ7ZelQRUg2fmtxHhB96LBNWHWcSy5o="
        },
        "wallet_id": 42
    }
    */

    if let Some(code) = account.code() {
        if let Some(info) = ctx
            .wallet_library
            .find_by_code(code)
            .map_err(|err| error!("failed to detect wallet contract: {err}"))?
        {
            result.insert("wallet".into(), serde_json::Value::Bool(true));
            result.insert(
                "wallet_type".into(),
                serde_json::Value::String(info.wallet_type().to_string()),
            );

            if let Some(data) = account.data() {
                let extracted = info
                    .extract(data)
                    .map_err(|err| error!("failed to extract wallet data: {err}"))?;
                for (key, value) in extracted {
                    result.insert(key, value);
                }
            }
        }
    }

    Ok(serde_json::Value::Object(result))
}

async fn get_address_information(p: GetAddressInformationParams, ctx: Ctx) -> JsonResult {
    let acc_ctx = AccountContext::with_address(&ctx, &p.address, p.seqno).await?;
    let account = acc_ctx.read_account()?;
    let balance = account.balance().cloned().unwrap_or_default();
    let frozen_hash =
        account.frozen_hash().map_or(String::new(), |h| serialize_uint256(h).to_string());
    let state = account_status(&account.status());
    let result = serde_json::json!({
        "@type": "raw.fullAccountState",
        "balance": balance.coins.to_string(),
        "extra_currencies": [], // TODO: fill extra currencies
        "code": serialize_cell_opt(account.code()),
        "data": serialize_cell_opt(account.data()),
        "last_transaction_id": serialize_shard_account(&acc_ctx.shard_account),
        "block_id": serialize_block_id(&acc_ctx.mc_block_id),
        "frozen_hash": frozen_hash,
        "sync_utime": account.last_paid(),
        "@extra": extra(0),
        "state": state,
    });
    Ok(result)
}

async fn get_address_balance(p: GetAddressInformationParams, ctx: Ctx) -> JsonResult {
    let acc_ctx = AccountContext::with_address(&ctx, &p.address, p.seqno).await?;
    let account = acc_ctx.read_account()?;
    Ok(serde_json::json!(account.balance().cloned().unwrap_or_default().coins.to_string()))
}

#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
enum UIntOrStr {
    Str(String),
    Int(u32),
}

#[derive(Debug, serde::Deserialize)]
struct RunGetMethodParams {
    address: String,
    method: UIntOrStr,
    stack: Vec<RPCStackEntry>,
    seqno: Option<u32>,
}

async fn run_get_method(p: RunGetMethodParams, ctx: Ctx) -> JsonResult {
    let acc_ctx = AccountContext::with_address(&ctx, &p.address, p.seqno).await?;
    let method_id = match p.method {
        UIntOrStr::Str(s) => ton_method_id(&s),
        UIntOrStr::Int(i) => i,
    };
    let mc_state_cell = acc_ctx.mc_state_root_cell();
    let stack = p.stack.into_iter().map(|e| e.into()).collect();
    let result = ton_vm::run_smc_method(&acc_ctx.shard_account, mc_state_cell, method_id, stack)?;
    let stack = serialize_stack(result.stack)?;

    Ok(serde_json::json!({
        "@type": "smc.runResult",
        "gas_used":  result.gas_used,
        "stack": stack,
        "exit_code": result.exit_code,
        "@extra": extra(0),
        "block_id": serialize_block_id(&acc_ctx.mc_block_id),
        "last_transaction_id": serialize_shard_account(&acc_ctx.shard_account),
    }))
}

#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
enum IntOrStr {
    Str(String),
    Int(i64),
}

impl IntOrStr {
    fn as_i32(&self) -> Result<i32> {
        match self {
            IntOrStr::Str(s) => s.parse().map_err(|e| {
                ApiError::unprocessable_entry(format!("Cannot parse integer {s}: {e}"), -63422)
                    .into()
            }),
            IntOrStr::Int(i) => Ok(*i as i32),
        }
    }
    fn as_i64(&self) -> Result<i64> {
        match self {
            IntOrStr::Str(s) => s.parse().map_err(|e| {
                ApiError::unprocessable_entry(format!("Cannot parse integer {s}: {e}"), -63422)
                    .into()
            }),
            IntOrStr::Int(i) => Ok(*i),
        }
    }
}

#[derive(Debug, serde::Deserialize)]
struct LookupBlockParams {
    workchain: IntOrStr,
    shard: IntOrStr,
    lt: Option<u64>,
    unixtime: Option<u32>,
    seqno: Option<u32>,
}

async fn lookup_block(p: LookupBlockParams, ctx: Ctx) -> JsonResult {
    let prefix = p.shard.as_i64()? as u64;
    let prefix = AccountIdPrefixFull::workchain(p.workchain.as_i32()?, prefix);
    let mut result = None;
    if let Some(lt) = p.lt {
        result = ctx.engine.lookup_block_by_lt(&prefix, lt).await?;
    } else if let Some(seqno) = p.seqno {
        result = ctx.engine.lookup_block_by_seqno(&prefix, seqno).await?;
    } else if let Some(utime) = p.unixtime {
        // get first - maybe need last?
        ctx.engine
            .lookup_blocks_by_utime(
                &prefix,
                utime,
                Box::new(|block_id, data| {
                    result = Some((block_id, data));
                    Ok(false)
                }),
            )
            .await?
    } else {
        fail!("at least one of lt, unixtime, seqno must be specified")
    };
    let Some((block_id, _data)) = result else { fail!("no block found with specified parameters") };
    Ok(serde_json::json!({
        "block_id": serialize_block_id(&block_id),
        "@extra": extra(0),
    }))
}

#[derive(Debug, serde::Deserialize)]
struct MasterchainSeqnoParams {
    seqno: Option<u32>,
}

async fn get_shards(p: MasterchainSeqnoParams, ctx: Ctx) -> JsonResult {
    let Some(seqno) = p.seqno else { fail!(ApiError::MissingParam("seqno".into())) };
    let mc_block_id = get_mc_state_id(&ctx, Some(seqno)).await?;
    let mc_state = ctx.engine.load_and_pin_state(&mc_block_id).await?;
    let shards = mc_state.state().top_blocks_all()?;
    Ok(serde_json::json!({
        "@type": "blocks.shards",
        "shards": shards.iter().map(|id| serialize_block_id(id)).collect::<Vec<_>>(),
    }))
}

async fn get_masterchain_block_signatures(p: MasterchainSeqnoParams, ctx: Ctx) -> JsonResult {
    let Some(seqno) = p.seqno else { fail!(ApiError::MissingParamTC("seqno".into())) };
    let mc_block_id = get_mc_state_id(&ctx, Some(seqno)).await?;
    let Some(handle) = ctx.engine.load_block_handle(&mc_block_id)? else {
        fail!("cannot load block handle {mc_block_id}")
    };
    let block = ctx.engine.load_block(&handle).await?;
    let Some(custom) = block.block()?.read_extra()?.read_custom()? else {
        fail!("no custom extra in block {mc_block_id}")
    };
    let mut signatures = Vec::new();
    custom.prev_blk_signatures().iterate(|descr| {
        signatures.push(serde_json::json!({
            "@type": "blocks.signature",
            "node_id_short": serialize_uint256(&descr.node_id_short),
            "signature": base64_encode(descr.sign.as_bytes())
        }));
        Ok(true)
    })?;
    Ok(serde_json::json!({
        "@type": "blocks.blockSignatures",
        "id": serialize_block_id(&mc_block_id),
        "signatures": signatures
    }))
}

#[derive(Debug, serde::Deserialize)]
struct GetShardBlockProofParams {
    workchain: IntOrStr,
    shard: IntOrStr,
    seqno: u32,
    from: Option<u32>,
}

async fn get_shard_proof(p: GetShardBlockProofParams, ctx: Ctx) -> JsonResult {
    let workchain = p.workchain.as_i32()?;
    let shard_prefix = p.shard.as_i64()? as u64;
    let seqno = p.seqno;
    let engine = &ctx.engine;
    let prefix = AccountIdPrefixFull::workchain(workchain, shard_prefix);
    let (block_id, _raw) = engine
        .lookup_block_by_seqno(&prefix, seqno)
        .await?
        .ok_or_else(|| error!("no shard block found for seqno {seqno}"))?;

    let shard_proof = get_shard_block_proof(engine, block_id).await?;
    let mc_id = shard_proof.masterchain_id.clone();

    let mc_prefix = AccountIdPrefixFull::any_masterchain();
    let from_id = if let Some(from) = p.from {
        engine
            .lookup_block_by_seqno(&mc_prefix, from)
            .await?
            .ok_or_else(|| error!("cannot find masterchain block with seqno {from}"))?
            .0
    } else {
        (*get_last_liteserver_state_block(&ctx.engine)?).clone()
    };

    if mc_id.seq_no > from_id.seq_no {
        fail!("from mc block is too old");
    }

    let partial = get_block_proof(engine, 0x1001, from_id.clone(), Some(mc_id.clone())).await?;
    if !matches!(partial.complete, Bool::BoolTrue) {
        fail!("mc proof is not complete");
    }
    if partial.from != from_id || partial.to != mc_id {
        fail!("got invalid mc proof chain (from/to mismatch)");
    }
    if partial.steps.len() > 1 {
        fail!("mc proof chain is too long");
    }

    let mut mc_proof_json = Vec::new();
    for step in partial.steps {
        match step {
            BlockLink::LiteServer_BlockLinkBack(back) => {
                mc_proof_json.push(serde_json::json!({
                    "@type": "blocks.blockLinkBack",
                    "to_key_block": matches!(back.to_key_block, Bool::BoolTrue),
                    "from": serialize_block_id(&back.from),
                    "to": serialize_block_id(&back.to),
                    "dest_proof": base64_encode(&back.dest_proof),
                    "proof": base64_encode(&back.proof),
                    "state_proof": base64_encode(&back.state_proof),
                }));
            }
            _ => {
                fail!("unsupported mc proof step type (expected BlockLinkBack only)")
            }
        }
    }
    let links_json: Vec<_> = shard_proof
        .links
        .into_iter()
        .map(|link| {
            serde_json::json!({
                 "@type": "blocks.shardBlockLink",
                "id": serialize_block_id(&link.id),
                "proof": base64_encode(&link.proof),
            })
        })
        .collect();
    Ok(serde_json::json!({
        "@type": "blocks.shardBlockProof",
        "from": serialize_block_id(&from_id),
        "mc_id": serialize_block_id(&mc_id),
        "links": links_json,
        "mc_proof": mc_proof_json, // getBlockProof(from -> mc_id)
        "@extra": extra(0),
    }))
}

#[derive(Debug, serde::Deserialize)]
struct SendBocParams {
    boc: String,
}

async fn send_boc(p: SendBocParams, ctx: Ctx) -> JsonResult {
    let body = base64_decode(&p.boc)
        .map_err(|e| ApiError::bad_request(format!("Invalid base64 BOC: {e}")))?;
    ctx.engine.redirect_external_message(&body).await?;
    Ok(serde_json::json!({
        "@type": "ok",
        "@extra": extra(0),
    }))
}

async fn send_boc_return_hash(p: SendBocParams, ctx: Ctx) -> JsonResult {
    let (root, bytes) = read_single_root_boc_with_bytes_from_base64(&p.boc, "BOC")?;
    let raw_b64 = base64_encode(root.repr_hash().as_slice());

    let mut s = SliceData::load_cell(root)
        .map_err(|e| ApiError::bad_request(format!("Can't construct slice from cell: {e}")))?;
    let mut msg = Message::construct_from(&mut s)
        .map_err(|e| ApiError::bad_request(format!("Not a Message cell: {e}")))?;
    if msg.ext_in_header().is_none() {
        fail!(ApiError::bad_request("BOC is not an external-in message".to_string()))
    }

    msg.normalize_external_inbound().map_err(|e| error!("Failed to normalize message: {e}"))?;
    let norm_cell =
        msg.serialize().map_err(|e| error!("Failed to serialize normalized message: {e}"))?;
    let hash_norm = base64_encode(norm_cell.repr_hash().as_slice());
    ctx.engine.redirect_external_message(&bytes).await?;

    Ok(serde_json::json!({
        "@type": "raw.extMessageInfo",
        "hash": raw_b64,
        "hash_norm": hash_norm,
        "@extra": extra(0),
    }))
}

#[derive(Debug, serde::Deserialize)]
struct EstimateFeeParams {
    address: String,
    body: String,
    #[serde(default)]
    ignore_chksig: bool,
    #[serde(default)]
    init_code: String,
    #[serde(default)]
    init_data: String,
}

async fn estimate_fee(p: EstimateFeeParams, ctx: Ctx) -> JsonResult {
    let acc_ctx = AccountContext::with_address(&ctx, &p.address, None).await?;
    let body = read_single_root_boc_from_base64(&p.body, "body")?;
    let dst = acc_ctx.address.clone();
    let body = SliceData::load_cell(body)?;
    let h = ExternalInboundMessageHeader::new(Default::default(), dst);
    let mut msg = Message::with_ext_in_header_and_body(h, body);
    if !p.init_code.is_empty() || !p.init_data.is_empty() {
        let mut init = StateInit::default();
        if !p.init_code.is_empty() {
            let code_cell = read_single_root_boc_from_base64(&p.body, "init_code")?;
            init.set_code(code_cell);
        };
        if !p.init_data.is_empty() {
            let data_cell = read_single_root_boc_from_base64(&p.body, "init_data")?;
            init.set_data(data_cell);
        };
        msg.set_state_init(init);
    }
    let in_msg_cell = msg.serialize()?;

    let config = acc_ctx.mc_state.state().shard_state_extra()?.config().clone();
    let config = BlockchainConfig::with_config(config)?;
    let limits = config.size_limits_config();
    let mut calc =
        StorageUsageCalc::with_limits(limits.max_msg_cells as u64, limits.max_msg_bits as u64);
    let _max_merkle_depth = calc.append_cell(&in_msg_cell, false, &mut 0)?;
    let fwd_prices = config.get_fwd_prices(acc_ctx.address.is_masterchain());
    let in_fwd_fee = fwd_prices.calc_fwd_fee(calc.bits(), calc.cells());
    let executor = OrdinaryTransactionExecutor::new(config);
    let last_tr_lt = acc_ctx.shard_account.last_trans_lt() + 1;
    let behavior_modifiers = Some(BehaviorModifiers { chksig_always_succeed: p.ignore_chksig });
    let prev_blocks_info = PrevBlocksInfo::Raw(
        KeyExtBlkRef {
            key: acc_ctx.mc_state.state().shard_state_extra()?.after_key_block,
            blk_ref: ExtBlkRef {
                end_lt: acc_ctx.mc_state.state().state()?.gen_lt(),
                seq_no: acc_ctx.mc_block_id.seq_no,
                root_hash: acc_ctx.mc_block_id.root_hash.clone(),
                file_hash: acc_ctx.mc_block_id.file_hash.clone(),
            },
        },
        acc_ctx.mc_state.state().shard_state_extra()?.prev_blocks.clone(),
    );
    let params = ExecuteParams {
        state_libs: acc_ctx.mc_state.state().state()?.libraries().clone().inner(),
        block_unixtime: ctx.engine.now(),
        block_lt: last_tr_lt,
        last_tr_lt,
        behavior_modifiers,
        prev_blocks_info,
        ..Default::default()
    };
    let mut account = acc_ctx.read_account()?;
    let Ok(tr) = executor.execute_with_params(Some(in_msg_cell), &mut account, params) else {
        return Ok(serde_json::json!({
            "@type": "query.fees",
            "source_fees": {
                "@type": "fees",
                "in_fwd_fee": in_fwd_fee,
                "storage_fee": 0,
                "gas_fee": 0,
                "fwd_fee": 0
            },
            "destination_fees": [],
            "@extra": extra(0),
        }));
    };

    let TransactionDescr::Ordinary(descr) = tr.read_description()? else {
        fail!("Only ordinary transactions are supported")
    };
    let storage_fee = if let Some(storage) = descr.storage_ph {
        storage.storage_fees_collected
    } else {
        Coins::zero()
    };
    let gas_fee = descr.compute_ph.gas_fees();
    let mut fwd_fee = Coins::zero();
    if let Some(action) = descr.action {
        if let Some(fee) = action.total_fwd_fees {
            fwd_fee += fee;
        }
    }
    Ok(serde_json::json!({
        "@type": "query.fees",
        "source_fees": {
            "@type": "fees",
            "in_fwd_fee": in_fwd_fee,
            "storage_fee": storage_fee.as_u128(),
            "gas_fee": gas_fee.as_u128(),
            "fwd_fee": fwd_fee.as_u128()
        },
        "destination_fees": [],
        "@extra": extra(0),
    }))
}

#[derive(Debug, serde::Deserialize)]
struct SendQueryParams {
    address: String,
    body: String,
    #[serde(default)]
    init_code: String,
    #[serde(default)]
    init_data: String,
}

async fn send_query(p: SendQueryParams, ctx: Ctx) -> JsonResult {
    let dst: MsgAddressInt =
        p.address.parse().map_err(|e| ApiError::bad_request(format!("Invalid address: {e}")))?;
    let body = read_single_root_boc_from_base64(&p.body, "BODY")?;
    let body = SliceData::load_cell(body)
        .map_err(|e| ApiError::bad_request(format!("Can't make slice from BODY cell: {e}")))?;
    let h = ExternalInboundMessageHeader::new(Default::default(), dst);
    let mut msg = Message::with_ext_in_header_and_body(h, body);

    if !p.init_code.is_empty() || !p.init_data.is_empty() {
        let mut init = StateInit::default();
        if !p.init_code.is_empty() {
            let code_cell = read_single_root_boc_from_base64(&p.body, "init_code")?;
            init.set_code(code_cell);
        };
        if !p.init_data.is_empty() {
            let data_cell = read_single_root_boc_from_base64(&p.body, "init_data")?;
            init.set_data(data_cell);
        };
        msg.set_state_init(init);
    }

    let boc_bytes =
        msg.write_to_bytes().map_err(|e| error!("Failed to serialize external message: {e}"))?;
    ctx.engine.redirect_external_message(&boc_bytes).await?;

    Ok(serde_json::json!({
        "@type": "ok",
        "@extra": extra(0),
    }))
}

#[derive(Debug, serde::Deserialize)]
struct TryLocateResultTxParams {
    source: String,
    destination: String,
    created_lt: u64,
}

// lookup for block by destination and created_lt first
// then find transaction in it by created_lt and source in inbound message
async fn try_locate_result_tx(p: TryLocateResultTxParams, ctx: Ctx) -> JsonResult {
    let address = p.destination.parse()?;
    let prefix = AccountIdPrefixFull::prefix(&address)?;
    let Some((block_id, data)) = ctx.engine.lookup_block_by_lt(&prefix, p.created_lt).await? else {
        fail!("block not found for {prefix}:{}", p.created_lt)
    };
    let block = Block::construct_from_bytes(&data)?;
    let extra = block.read_extra()?;
    let acc_blocks = extra.read_account_blocks()?;
    let Some(acc_block) = acc_blocks.get(address.address())? else {
        fail!("transactions for account {} not found in block {block_id}", p.destination)
    };
    let address = p.source.parse()?;
    let mut result = None;
    acc_block.transactions().iterate_slices_with_keys(|_lt, slice| {
        let tr_cell = slice.reference(0)?;
        let tr = Transaction::construct_from_cell(tr_cell.clone())?;
        if let Some(in_msg) = tr.read_in_msg()? {
            if in_msg.created_lt() == Some(p.created_lt) {
                if in_msg.src_ref() == Some(&address) {
                    result = Some((tr, tr_cell));
                }
            }
        }
        Ok(result.is_none())
    })?;
    let Some((tr, tr_cell)) = result else {
        fail!(
            "transaction with lt {} not found for account {} in block {block_id}",
            p.created_lt,
            p.destination
        )
    };
    serialize_transaction(&tr, tr_cell, &p.destination)
}

// lookup for block by source account and created_lt first
// then find transaction in it by created_lt and destination in outbound message
async fn try_locate_source_tx(p: TryLocateResultTxParams, ctx: Ctx) -> JsonResult {
    let address = p.source.parse()?;
    let prefix = AccountIdPrefixFull::prefix(&address)?;
    let Some((block_id, data)) = ctx.engine.lookup_block_by_lt(&prefix, p.created_lt).await? else {
        fail!("block not found for {prefix}:{}", p.created_lt)
    };
    let block = Block::construct_from_bytes(&data)?;
    let extra = block.read_extra()?;
    let acc_blocks = extra.read_account_blocks()?;
    let Some(acc_block) = acc_blocks.get(address.address())? else {
        fail!("transactions for account {} not found in block {block_id}", p.source)
    };
    let address = p.destination.parse()?;
    let mut result = None;
    acc_block.transactions().iterate_slices_with_keys(|_lt, slice| {
        let tr_cell = slice.reference(0)?;
        let tr = Transaction::construct_from_cell(tr_cell.clone())?;
        let found = !tr.iterate_out_msgs(|out_msg| {
            Ok(out_msg.created_lt() != Some(p.created_lt) || out_msg.dst_ref() != Some(&address))
        })?;
        if found {
            result = Some((tr, tr_cell));
        }
        Ok(result.is_none())
    })?;
    let Some((tr, tr_cell)) = result else {
        fail!(
            "transaction with lt {} not found for account {} in block {block_id}",
            p.created_lt,
            p.source
        )
    };
    serialize_transaction(&tr, tr_cell, &p.destination)
}

#[derive(Debug, serde::Deserialize)]
struct GetBlockTransactionsParams {
    workchain: Option<IntOrStr>,
    shard: Option<IntOrStr>,
    seqno: Option<u32>,
    #[serde(default)]
    root_hash: String,
    #[serde(default)]
    file_hash: String,
    #[serde(default)]
    after_lt: Option<u64>,
    #[serde(default)]
    after_hash: Option<String>,
    #[serde(default)]
    count: usize,
}

struct GetBlockTransactionsResult {
    block_id: BlockIdExt,
    all_txs: Vec<(MsgAddressInt, u64, Cell)>,
}

macro_rules! parse_hash {
    ($value:expr, $msg:expr) => {{
        if !$value.is_empty() {
            match UInt256::from_str($value) {
                Ok(v) => Some(v),
                Err(_) => fail!(ApiError::InvalidParam($msg.into())),
            }
        } else {
            None
        }
    }};
}

async fn get_block_transactions_int(
    p: &GetBlockTransactionsParams,
    ctx: &Ctx,
) -> Result<GetBlockTransactionsResult> {
    let Some(shard) = &p.shard else { fail!(ApiError::MissingParamTC("shard".into())) };
    let prefix = shard.as_i64()? as u64;
    let Some(workchain) = &p.workchain else { fail!(ApiError::MissingParamTC("workchain".into())) };
    let workchain = workchain.as_i32()?;
    let Some(seqno) = p.seqno else { fail!(ApiError::MissingParamTC("seqno".into())) };
    let root_hash = parse_hash!(&p.root_hash, "root_hash: invalid hash");
    let file_hash = parse_hash!(&p.file_hash, "file_hash: invalid hash");

    let prefix = AccountIdPrefixFull::workchain(workchain, prefix);
    let Some((block_id, data)) = ctx.engine.lookup_block_by_seqno(&prefix, seqno).await? else {
        fail!("block not found for {prefix}:{}", seqno)
    };
    if root_hash.is_some() && *block_id.root_hash() != root_hash.unwrap() {
        fail!(ApiError::InvalidParam("root_hash".into()))
    }
    if file_hash.is_some() && *block_id.file_hash() != file_hash.unwrap() {
        fail!(ApiError::InvalidParam("file_hash".into()))
    }
    let block = Block::construct_from_bytes(&data)?;
    let extra = block.read_extra()?;
    let acc_blocks = extra.read_account_blocks()?;

    let mut all_txs = Vec::new();
    acc_blocks.iterate_with_keys(|acc_id, acc_block| {
        let address = MsgAddressInt::standard(prefix.workchain_id() as i8, acc_id.clone());
        acc_block.transactions().iterate_slices_with_keys(|lt, slice| {
            let tr_cell = slice.reference(0)?;
            all_txs.push((address.clone(), lt, tr_cell));
            Ok(true)
        })
    })?;
    all_txs.sort_by(|a, b| a.1.cmp(&b.1));
    if let (Some(after_lt), Some(after_hash)) = (p.after_lt, p.after_hash.as_ref()) {
        let after_hash = after_hash.parse()?;
        let pos = all_txs.iter().position(|a| a.1 == after_lt && a.2.repr_hash() == after_hash);
        if let Some(pos) = pos {
            all_txs = all_txs.split_off(pos);
        }
    }
    Ok(GetBlockTransactionsResult { block_id, all_txs })
}

async fn get_block_transactions(p: GetBlockTransactionsParams, ctx: Ctx) -> JsonResult {
    let addr_mode = if ctx.is_testnet().await {
        ADDR_FORMAT_BOUNCE | ADDR_FORMAT_URL_SAFE | ADDR_FORMAT_TESTNET
    } else {
        ADDR_FORMAT_BOUNCE | ADDR_FORMAT_URL_SAFE
    };
    let result = get_block_transactions_int(&p, &ctx).await?;
    let incomplete = result.all_txs.len() > p.count;
    let mut transactions = Vec::new();
    for (address, lt, tr_cell) in result.all_txs.into_iter().take(p.count) {
        transactions.push(serde_json::json!({
            "@type": "blocks.shortTxId",
            "mode": 7,
            "account": address.to_string_custom(addr_mode)?,
            "lt": lt.to_string(),
            "hash": serialize_uint256(&tr_cell.repr_hash())
        }));
    }
    Ok(serde_json::json!({
        "@type": "blocks.transactions",
        "id": serialize_block_id(&result.block_id),
        "req_count": p.count,
        "incomplete": incomplete,
        "transactions": transactions
    }))
}

async fn get_block_transactions_ext(p: GetBlockTransactionsParams, ctx: Ctx) -> JsonResult {
    let addr_mode = if ctx.is_testnet().await {
        ADDR_FORMAT_URL_SAFE | ADDR_FORMAT_TESTNET
    } else {
        ADDR_FORMAT_URL_SAFE
    };
    let result = get_block_transactions_int(&p, &ctx).await?;
    let incomplete = result.all_txs.len() > p.count;
    let mut transactions = Vec::new();
    for (address, _, tr_cell) in result.all_txs.into_iter().take(p.count) {
        let tr = Transaction::construct_from_cell(tr_cell.clone())?;
        let address = if tr.read_in_msg()?.map_or(true, |msg| msg.is_bouncable()) {
            address.to_string_custom(addr_mode | ADDR_FORMAT_BOUNCE)
        } else {
            address.to_string_custom(addr_mode)
        }?;
        transactions.push(serialize_transaction(&tr, tr_cell, &address)?);
    }
    Ok(serde_json::json!({
        "@type": "blocks.transactionsExt",
        "id": serialize_block_id(&result.block_id),
        "req_count": p.count,
        "incomplete": incomplete,
        "transactions": transactions
    }))
}

#[derive(Debug, serde::Deserialize)]
struct GetBlockParams {
    workchain: Option<IntOrStr>,
    shard: Option<IntOrStr>,
    seqno: Option<u32>,
    #[serde(default)]
    root_hash: String,
    #[serde(default)]
    file_hash: String,
}

async fn get_block_data(p: GetBlockParams, ctx: Ctx) -> Result<(BlockIdExt, Vec<u8>)> {
    let Some(shard) = p.shard else { fail!(ApiError::MissingParamTC("shard".into())) };
    let prefix = shard.as_i64()? as u64;
    let Some(workchain) = p.workchain else { fail!(ApiError::MissingParamTC("workchain".into())) };
    let workchain = workchain.as_i32()?;
    let Some(seqno) = p.seqno else { fail!(ApiError::MissingParamTC("seqno".into())) };
    let root_hash = parse_hash!(&p.root_hash, "root_hash: invalid hash");
    let file_hash = parse_hash!(&p.file_hash, "file_hash: invalid hash");

    let prefix = AccountIdPrefixFull::workchain(workchain, prefix);
    let Some((block_id, data)) = ctx.engine.lookup_block_by_seqno(&prefix, seqno).await? else {
        let msg = if seqno == 0 {
            format!(
                "block not found for {prefix}, seqno {seqno}. If this is a zerostate (seqno=0), \
                its full block BOC may be unavailable in local archive"
            )
        } else {
            format!("block not found for {prefix}, seqno {seqno}")
        };
        fail!(ApiError::NotFound(msg))
    };

    if let Some(root_hash) = &root_hash {
        if block_id.root_hash() != root_hash {
            fail!(ApiError::InvalidParam("root_hash".into()))
        }
    }
    if let Some(file_hash) = &file_hash {
        if block_id.file_hash() != file_hash {
            fail!(ApiError::InvalidParam("file_hash".into()))
        }
    }

    Ok((block_id, data))
}

async fn get_block_header(p: GetBlockParams, ctx: Ctx) -> JsonResult {
    let (block_id, data) = get_block_data(p, ctx).await?;
    let block = Block::construct_from_bytes(&data)?;
    let info = block.read_info()?;
    Ok(serde_json::json!({
        "@type": "blocks.header",
        "id": serialize_block_id(&block_id),
        "global_id": info.gen_software().map_or(0, |v| v.version),
        "version": info.version(),
        "flags": info.flags(),
        "after_merge": info.after_merge(),
        "after_split": info.after_split(),
        "before_split": info.before_split(),
        "want_merge": info.want_merge(),
        "want_split": info.want_split(),
        "validator_list_hash_short": info.gen_validator_list_hash_short() as i32,
        "catchain_seqno": info.gen_catchain_seqno(),
        "min_ref_mc_seqno": info.min_ref_mc_seqno(),
        "is_key_block": info.key_block(),
        "prev_key_block_seqno": info.prev_key_block_seqno(),
        "start_lt": info.start_lt().to_string(),
        "end_lt": info.end_lt().to_string(),
        "gen_utime": info.gen_utime(),
        "prev_blocks": info.read_prev_ids()?.iter().map(|id| serialize_block_id(id)).collect::<Vec<_>>(),
    }))
}

async fn get_block(p: GetBlockParams, ctx: Ctx) -> JsonResult {
    let (block_id, data) = get_block_data(p, ctx).await?;
    Ok(serde_json::json!({
        "id": serialize_block_id(&block_id),
        "boc": base64_encode(&data),
    }))
}

const FILTER_BY_SHARD: i32 = 1;
const SKIP_EXTERNALS_QUEUE_SIZE: i32 = 1000;

#[derive(Debug, serde::Deserialize)]
struct GetOutMsgQueueSizesParams {
    #[serde(default)]
    mode: Option<i32>,
    #[serde(default)]
    wc: Option<i32>,
    #[serde(default)]
    shard: Option<IntOrStr>,
}

async fn get_out_msg_queue_sizes(p: GetOutMsgQueueSizesParams, ctx: Ctx) -> JsonResult {
    let mode = p.mode.unwrap_or(0);
    let mc_id = get_last_liteserver_state_block(&ctx.engine)?;
    let mc_state = ctx.engine.load_state(&mc_id).await?;
    let shard_hashes = mc_state.shard_state_extra()?.shards();

    let filter = if (mode & FILTER_BY_SHARD) != 0 {
        let wc =
            p.wc.ok_or_else(|| error!("wc is required for getOutMsgQueueSizes with mode bit0"))?;
        let shard = p
            .shard
            .as_ref()
            .ok_or_else(|| error!("shard is required for getOutMsgQueueSizes with mode bit0"))?
            .as_i64()?;
        Some(ShardIdent::with_tagged_prefix(wc, shard as u64)?)
    } else {
        None
    };

    let mut shard_ids = Vec::new();
    if filter.as_ref().map_or(true, |f| f.intersect_with(mc_id.shard())) {
        shard_ids.push(mc_id);
    }
    shard_hashes.iterate_shards(|ident, descr| {
        if filter.as_ref().map_or(true, |f| f.intersect_with(&ident)) {
            shard_ids.push(Arc::new(BlockIdExt::with_params(
                ident,
                descr.seq_no,
                descr.root_hash,
                descr.file_hash,
            )));
        }
        Ok(true)
    })?;

    let mut shards = Vec::with_capacity(shard_ids.len());
    for id in shard_ids {
        let state = ctx.engine.load_state(&id).await?;
        let info = state
            .state()?
            .read_out_msg_queue_info()
            .map_err(|e| error!("cannot read out_msg_queue_info: {e}"))?;

        let queue_size = info.extra().out_queue_size();
        let size_usize = if queue_size > 0 { queue_size } else { info.out_queue().len()? };
        let size = i32::try_from(size_usize)
            .map_err(|_| error!("out_msg_queue_size overflow: {size_usize}"))?;
        shards.push(serde_json::json!({
            "id": serialize_block_id(&id),
            "size": size,
        }));
    }

    Ok(serde_json::json!({
        "@type": "liteServer.outMsgQueueSizes",
        "shards": shards,
        "ext_msg_queue_size_limit": SKIP_EXTERNALS_QUEUE_SIZE,
    }))
}

#[derive(Debug, serde::Deserialize)]
struct GetConfigParamParams {
    config_id: u32,
    #[serde(default)]
    seqno: Option<u32>,
}

async fn get_config_param(p: GetConfigParamParams, ctx: Ctx) -> JsonResult {
    let mc_block_id = get_last_liteserver_state_block(&ctx.engine)?;
    let mut mc_state = ctx.engine.load_and_pin_state(&mc_block_id).await?;
    if let Some(mc_seq_no) = p.seqno {
        let mc_block_id = mc_state.state().find_block_id(mc_seq_no)?;
        mc_state = ctx.engine.load_and_pin_state(&mc_block_id).await?;
    }
    let config = &mc_state.state().shard_state_extra()?.config().config_params;
    let key = p.config_id.write_to_bitstring()?;
    let bytes = if let Some(slice) = config.get(key)? {
        let cell = slice
            .reference(0)
            .map_err(|e| error!("Failed to get config param reference cell: {e}"))?;
        base64_encode(write_boc(&cell)?)
    } else {
        "".to_string()
    };
    Ok(serde_json::json!({
        "@type": "configInfo",
        "config": {
            "@type": "tvm.cell",
            "bytes": bytes
        }
    }))
}

#[derive(Debug, serde::Deserialize)]
struct GetLibrariesParams {
    libraries: Vec<String>,
}

async fn get_libraries(p: GetLibrariesParams, ctx: Ctx) -> JsonResult {
    let mc_block_id = get_last_liteserver_state_block(&ctx.engine)?;
    let mc_state = ctx.engine.load_and_pin_state(&mc_block_id).await?;
    let libraries = mc_state.state().state()?.libraries();
    let mut result = Vec::new();
    let mut found = std::collections::HashSet::new();
    for id in p.libraries {
        let key: UInt256 = id.parse()?;
        if !found.contains(&key) {
            if let Some(descr) = libraries.get(&key)? {
                let data = base64_encode(write_boc(descr.lib())?);
                result.push(serde_json::json!({
                    "@type": "smc.libraryEntry",
                    "hash": serialize_uint256(&key),
                    "data": data
                }));
                found.insert(key);
            }
        }
    }
    Ok(serde_json::json!({
        "@type": "smc.libraryResult",
        "result": result,
        "@extra": extra(0),
    }))
}

#[derive(Debug, serde::Deserialize)]
struct DetectAddressParams {
    address: String,
}

async fn detect_address(p: DetectAddressParams, ctx: Ctx) -> JsonResult {
    let input = p.address.trim();
    if input.is_empty() {
        fail!(ApiError::bad_request("address parameter is required"))
    }

    let (address, given_type, testnet) =
        if let Some(FriendlyAddressData { address, bounceable, testnet }) =
            parse_friendly_address(input)?
        {
            let given_type =
                if bounceable { "friendly_bounceable" } else { "friendly_non_bounceable" };
            (address, given_type, testnet)
        } else {
            let testnet = ctx.is_testnet().await;
            let address: MsgAddressInt = input
                .parse()
                .map_err(|e| ApiError::bad_request(format!("Invalid address: {e}")))?;
            (address, "raw_form", testnet)
        };

    let addr_mode = if testnet { ADDR_FORMAT_TESTNET } else { 0 };
    Ok(serde_json::json!({
        "raw_form": address.to_string(),
        "bounceable": {
            "b64": address.to_string_custom(addr_mode | ADDR_FORMAT_BOUNCE)?,
            "b64url": address.to_string_custom(
                addr_mode | ADDR_FORMAT_BOUNCE | ADDR_FORMAT_URL_SAFE
            )?,
        },
        "non_bounceable": {
            "b64": address.to_string_custom(addr_mode | ADDR_FORMAT_URL_SAFE)?,
            "b64url": address.to_string_custom(addr_mode | ADDR_FORMAT_URL_SAFE)?,
        },
        "given_type": given_type,
        "testnet": testnet,
    }))
}

async fn pack_address(p: DetectAddressParams, ctx: Ctx) -> JsonResult {
    let input = p.address.trim();
    if input.is_empty() {
        fail!(ApiError::bad_request("address parameter is required"))
    }

    let (address, testnet) = if let Some(fa) = parse_friendly_address(input)? {
        (fa.address, fa.testnet)
    } else {
        let testnet = ctx.is_testnet().await;
        let address = parse_address(input)?;
        // let address: MsgAddressInt = input.parse().map_err(|e| ApiError {
        //     jsonrpc_http_status: http::StatusCode::RANGE_NOT_SATISFIABLE,
        //     http_status: http::StatusCode::RANGE_NOT_SATISFIABLE,
        //     jsonrpc_code: 416,
        //     message: format!("Invalid address {input}: {e}").into(),
        // })?;
        (address, testnet)
    };

    let addr_mode = if testnet { ADDR_FORMAT_TESTNET } else { 0 };
    Ok(serde_json::json!(
        address.to_string_custom(addr_mode | ADDR_FORMAT_BOUNCE | ADDR_FORMAT_URL_SAFE)?
    ))
}

async fn unpack_address(p: DetectAddressParams, _ctx: Ctx) -> JsonResult {
    let input = p.address.trim();
    if input.is_empty() {
        fail!(ApiError::bad_request("address parameter is required"))
    }

    let address = if let Some(fa) = parse_friendly_address(input)? {
        fa.address
    } else {
        parse_address(input)?
    };

    Ok(serde_json::json!(address.to_string()))
}

struct FriendlyAddressData {
    address: MsgAddressInt,
    bounceable: bool,
    testnet: bool,
}

fn parse_friendly_address(input: &str) -> Result<Option<FriendlyAddressData>> {
    if input.len() != 48 {
        return Ok(None);
    }

    let decoded = match base64_decode_url_safe(input) {
        Ok(bytes) => bytes,
        Err(_) => match base64_decode(input) {
            Ok(bytes) => bytes,
            Err(_) => return Ok(None),
        },
    };
    if decoded.len() != 36 {
        fail!(ApiError::bad_request("Friendly address must decode into 36 bytes"))
    }

    let expected_crc = (u16::from(decoded[34]) << 8) | u16::from(decoded[35]);
    let actual_crc = address_crc(&decoded[..34]);
    if expected_crc != actual_crc {
        fail!(ApiError::bad_request("Friendly address checksum mismatch"))
    }

    let flags = decoded[0];
    let testnet = flags & 0x80 != 0;
    let bounceable = flags & 0x40 == 0;
    let workchain_id = decoded[1] as i8;
    let address = SliceData::from_raw(&decoded[2..34], 256);
    let address = MsgAddressInt::standard(workchain_id, address);

    Ok(Some(FriendlyAddressData { address, bounceable, testnet }))
}

fn parse_address(input: &str) -> Result<MsgAddressInt> {
    input.parse().map_err(|_e| ApiError::NotSatisfiable(format!("Incorrect address")).into())
}

#[derive(Debug, serde::Deserialize)]
struct GetTokenDataParams {
    address: String,
}

async fn get_token_data(p: GetTokenDataParams, ctx: Ctx) -> JsonResult {
    let acc_ctx = AccountContext::with_address(&ctx, &p.address, None).await?;
    let mc_state_cell = acc_ctx.mc_state_root_cell();
    let is_testnet = ctx.is_testnet().await;

    const TYPES_METHODS: [(&str, &str); 4] = [
        ("jetton_master", "get_jetton_data"),
        ("jetton_wallet", "get_wallet_data"),
        ("nft_collection", "get_collection_data"),
        ("nft_item", "get_nft_data"),
    ];

    let mut contract_type: Option<&str> = None;
    let mut stack: Option<Vec<StackEntry>> = None;

    for (ty, method_name) in &TYPES_METHODS {
        let method_id = ton_method_id(&method_name);

        let res = ton_vm::run_smc_method(
            &acc_ctx.shard_account,
            mc_state_cell.clone(),
            method_id,
            Vec::<StackEntry>::new(),
        );

        let Ok(res) = res else {
            continue;
        };

        if res.exit_code == 0 {
            contract_type = Some(*ty);
            stack = Some(res.stack);
            break;
        }
    }
    let contract_type = contract_type.ok_or_else(|| {
        error!(ApiError::Conflict(format!("Smart contract {} is not Jetton or NFT", p.address)))
    })?;
    let stack = stack.expect("stack must be Some when contract_type is Some");

    let result = match contract_type {
        "jetton_master" => parse_jetton_master_data(&stack, is_testnet)?,
        "jetton_wallet" => parse_jetton_wallet_data(&stack, is_testnet)?,
        "nft_collection" => parse_nft_collection(&stack, is_testnet)?,
        "nft_item" => parse_nft_item_data(&stack, is_testnet)?,
        other => fail!("unexpected contract type: {other}"),
    };

    Ok(result)
}

#[derive(Debug, serde::Deserialize)]
struct RunStackParams {
    address: String,
    method_id: String,
    stack: Vec<RPCStackEntry>,
}

async fn run_stack_test(p: RunStackParams, _ctx: Ctx) -> JsonResult {
    let stack = serialize_stack(p.stack)?;
    Ok(serde_json::json!({
        "address": p.address,
        "method": p.method_id,
        "stack": stack,
    }))
}

fn extra(ls_index: u64) -> String {
    let now_secs = UnixTime::now_f64();
    let rand_val: f64 = rand::random();
    // "%s:%s:%s" % (ts, ls_index, rand_val)
    format!("{}:{}:{}", now_secs, ls_index, rand_val)
}

#[cfg(test)]
#[path = "tests/test_handlers.rs"]
mod tests;
/*
│ 0 │ getSeqno            │ 16    │ 1.024  │ 0.222  │ 6.796  │ 1.662  │ 0.469  │ 2.698  │ 2.698  │ 6.796  │
│ 1 │ send                │ 6     │ 0.573  │ 0.203  │ 1.223  │ 0.373  │ 0.613  │ 1.223  │ 1.223  │ 1.223  │
│ 2 │ runMethod           │ 3     │ 1.192  │ 0.219  │ 2.864  │ 1.454  │ 0.493  │ 2.864  │ 2.864  │ 2.864  │
│ 3 │ getTransactions     │ 12    │ 3.448  │ 0.118  │ 8.092  │ 3.372  │ 3.342  │ 7.964  │ 7.964  │ 8.092  │
│ 4 │ getTransaction      │ 4     │ 0.350  │ 0.099  │ 0.491  │ 0.181  │ 0.474  │ 0.491  │ 0.491  │ 0.491  │
│ 5 │ getTransactions_ERR │ 8     │ 10.296 │ 10.254 │ 10.335 │ 0.026  │ 10.295 │ 10.327 │ 10.335 │ 10.335 │
│ 6 │ lookupBlock         │ 10    │ 0.231  │ 0.102  │ 0.482  │ 0.156  │ 0.132  │ 0.434  │ 0.482  │ 0.482  │
│ 7 │ getShardBlockProof  │ 10    │ 0.870  │ 0.413  │ 1.992  │ 0.530  │ 0.939  │ 1.320  │ 1.992  │ 1.992  │
│ 8 │ getBlockHeader      │ 10    │ 1.200  │ 0.427  │ 6.977  │ 2.040  │ 0.484  │ 1.109  │ 6.977  │ 6.977  │
*/
/*
{
  "ok": true,
  "result": {
    "@type": "blocks.shardBlockProof",
    "from": {
      "@type": "ton.blockIdExt",
      "workchain": -1,
      "shard": "-9223372036854775808",
      "seqno": 2519640,
      "root_hash": "FDrQoUzZUbOHc6UNY0mBN03hQN2AUzu+mETAi0WoaaQ=",
      "file_hash": "eRy8Y2e0Yis5VmUtkrEyxwFrLNAVVkOYc28KIKc+uXQ="
    },
    "mc_id": {
      "@type": "ton.blockIdExt",
      "workchain": -1,
      "shard": "-9223372036854775808",
      "seqno": 271885,
      "root_hash": "glKt0B3JgCWAY1ThZnRzncl22/8vkiBGrspMQe0bk0I=",
      "file_hash": "23ZStMEKu4jf6v309fdmKUNB42BErrz5KkbUwjA1vZU="
    },
    "links": [
      {
        "@type": "blocks.shardBlockLink",
        "id": {
          "@type": "ton.blockIdExt",
          "workchain": 0,
          "shard": "2305843009213693952",
          "seqno": 107498,
          "root_hash": "zhQbOhXXLU6MVqQyY7ZsYsE1KeB1jbXaVpLomBSMUgQ=",
          "file_hash": "O3jxSivaj7hQGbC0o6L1+ES5QqQQ55HeTqTVJS56R6s="
        },
        "proof": "te6ccgECIQEABG4ACUYDglKt0B3JgCWAY1ThZnRzncl22/8vkiBGrspMQe0bk0IADQEkEBHvVaoAAABlAgMEBQGgm8ephwAAAAAEAQAEJg0AAAAAAP////8AAAAAAAAAAGkZvxUAAABRJN/VwAAAAFEk39XFnx4pfAAAAYUABCYLAAAP8sQAAAALAAAAAAAAAe4GIhG45I37Sv1W2AQHCCqKBL8i2A1AMfFWF07sDWOFak4I93iF5Y5x8N7NeiICyVDITMgn1MmW9PmGG+VO80fq1ebVhFu8bw9NZBHOLX4HSmcAFgAWCQokiUoz9vu5+i2Mn9NaRjBBb1+pXfgtl7McFQJO+qu+ArYYlGp07BhcdIUBMLoClLwkhIWvzWBQQiIHCfVaHopqVLBhS/gioAsMDQ4AmAAAAFEkoszFAAQmDCSpAFlppUP2MGtnfyXQpPQcEzhdFVheEr33fVYqeG7H7jQt4U6GA4QYNPV9/JfQAng/Bca4QLGcVo+PejQa4sUoSAEBtSWiZTKRbsG1k7r3EUE7rT9yS775BfLDaYpTinjKQZEAAihIAQEoBxjZ+Y2KyzNe/cgDYfVWEun3obZ+Ob8InYAWdvIT0QAAaIwBA78i2A1AMfFWF07sDWOFak4I93iF5Y5x8N7NeiICyVDI4enM1vwlRm6ATqrdML++4YmEKLuOCLfHBIKNXtaEK8EAFgALaIwBA0zIJ9TJlvT5hhvlTvNH6tXm1YRbvG8PTWQRzi1+B0pnpk11Ab6O7L2UPXwKzqt/tcbGNZPUYS7JJZg+/xSYEU0AFgALIQOAIA8AAQIhAYIQIxfMpWiVAvkARKgXyAQREhMoSAEBLGptBz3yCfXqdiaU476gzp6UrGg+hj+JZWOAXj75OasAAyhIAQH4O3gnycuY2pKf+t/OWbEYsL1+u4MHsYWK6xNYIRsjRQAGIQPQQBQiFcgRKgXyAIlQL5AIHB0hAVAeIgHAFRYiAcAXGCIBwBobAdtQAA0fUAAhMGgAAAKJJoScAAAAAokmhJwOcKDZ0K65anRitSGTHbNjFgmpTwOsba7StJdEwKRikCHbx4pRXtR9woDNhaUdF6/CJcoVIIc8jvJ1Jqkpc9I9WIAAAAwpAAAAAAAAAAAAITBbSM34qhkoSAEB6/OLxDWKft11pS2/AqputLaNI6gH4pSiKZUZzCOm8LMAAQATQdzWUAIO5rKAIChIAQEK2CfRVFEz2wQKOB+/TxHZHLrknZahePpSOgbk2IrokwABKEgBARkt5KAjIxOazP2VqJftKkWh/2cdg6LFPjDB8pDT+/9KAAEoSAEBi+bQ8EhFOHj5GqvCpFk7mN5PFW36QLeMHhqyJh9FRQsAAShIAQEILXJtxKSgF76R/SkIExMPeL/iXK2gcZRAuKWlDEbYJAABIgFhHyAoSAEBNE34ScgHcv1wt4qpCs9q6FN5IMoambngv9Jen/UxP0YAAShIAQHc3a1hq0qmG/qHF7xFcsTTkx1hBoKZG9jjYZqnkcX78QAC"
      },
      {
        "@type": "blocks.shardBlockLink",
        "id": {
          "@type": "ton.blockIdExt",
          "workchain": 0,
          "shard": "2305843009213693952",
          "seqno": 107497,
          "root_hash": "JUBQPjvdO9w8qltaSdv4xYRnco0jQMZOyknjkcJ1plU=",
          "file_hash": "1HY15JW2oIqJeWexi4E6nPsYuDMaRWDzbDbixDPihtU="
        },
        "proof": "te6ccgECCAEAAZYACUYDzhQbOhXXLU6MVqQyY7ZsYsE1KeB1jbXaVpLomBSMUgQABgEkEBHvVaoAAABlAgMEBQKgm8ephwAAAACEAQABo+oAAAAAAgAAAAAAAAAAAAAAAGkZvxUAAABRJNCTgAAAAFEk0JOB1muMAAAAAYUABCYLAAAP8sQAAAALAAAAAAAAAe4GByhIAQEYmYcEkaBtzwRI98Oc3LHbwCE15Dj1Ce1eKfi1MPN+WwABKEgBAeYoNjUrLD4IDr7CCWVL8gMDBRlAq1hyJvyJmnmjJusMAAUoSAEB7guH4rQRtqGozPaTd4K/fY0LviCtYxHGpMKjmLDgvEsABACYAAAAUSRlw8UABCYL0aXdNBlOH4hh88tv0O637T05kgQ6DsoWBUUIfqUhhcltHm6EE7da+9QvOPZrl/HI+3mflpiWGYLYHbisG+wx7gCYAAAAUSSizMEAAaPpJUBQPjvdO9w8qltaSdv4xYRnco0jQMZOyknjkcJ1plXUdjXklbagiol5Z7GLgTqc+xi4MxpFYPNsNuLEM+KG1Q=="
      }
    ],
    "mc_proof": [
      {
        "@type": "blocks.blockLinkBack",
        "to_key_block": false,
        "from": {
          "@type": "ton.blockIdExt",
          "workchain": -1,
          "shard": "-9223372036854775808",
          "seqno": 2519640,
          "root_hash": "FDrQoUzZUbOHc6UNY0mBN03hQN2AUzu+mETAi0WoaaQ=",
          "file_hash": "eRy8Y2e0Yis5VmUtkrEyxwFrLNAVVkOYc28KIKc+uXQ="
        },
        "to": {
          "@type": "ton.blockIdExt",
          "workchain": -1,
          "shard": "-9223372036854775808",
          "seqno": 271885,
          "root_hash": "glKt0B3JgCWAY1ThZnRzncl22/8vkiBGrspMQe0bk0I=",
          "file_hash": "23ZStMEKu4jf6v309fdmKUNB42BErrz5KkbUwjA1vZU="
        },
        "dest_proof": "te6ccgECFgEAA0YACUYDglKt0B3JgCWAY1ThZnRzncl22/8vkiBGrspMQe0bk0IADQEkEBHvVaoAAABlAgMEBQGgm8ephwAAAAAEAQAEJg0AAAAAAP////8AAAAAAAAAAGkZvxUAAABRJN/VwAAAAFEk39XFnx4pfAAAAYUABCYLAAAP8sQAAAALAAAAAAAAAe4GAhG45I37Sv1W2AQHCCqKBL8i2A1AMfFWF07sDWOFak4I93iF5Y5x8N7NeiICyVDITMgn1MmW9PmGG+VO80fq1ebVhFu8bw9NZBHOLX4HSmcAFgAWDA0kiUoz9vu5+i2Mn9NaRjBBb1+pXfgtl7McFQJO+qu+ArYYlGp07BhcdIUBMLoClLwkhIWvzWBQQiIHCfVaHopqVLBhS/gioA4PEBEAmAAAAFEkoszFAAQmDCSpAFlppUP2MGtnfyXQpPQcEzhdFVheEr33fVYqeG7H7jQt4U6GA4QYNPV9/JfQAng/Bca4QLGcVo+PejQa4sUCJYAuwXxBVnooHAF2C+JinogAwAgJCQAdRKgXyAJX6rbAEZVPxAAIAgEgCgsAFb4AAAO8s2cNwVVQABW/////vL0alKIAEGiMAQO/ItgNQDHxVhdO7A1jhWpOCPd4heWOcfDezXoiAslQyOHpzNb8JUZugE6q3TC/vuGJhCi7jgi3xwSCjV7WhCvBABYAC2iMAQNMyCfUyZb0+YYb5U7zR+rV5tWEW7xvD01kEc4tfgdKZ6ZNdQG+juy9lD18Cs6rf7XGxjWT1GEuySWYPv8UmBFNABYACyhIAQHlhX6S8Op02o427gaNIqS1xvxoPgackj2ouHC16ZrdTQAEKEgBAa5LMoDlbi+vg/QUpuPavp1fvhiXZUTAX+0SGsy4W1P8AAAoSAEBh6T9UhI07SdUAHBBKvrHhGugwOCSmm3PFhKPNOZt4+4AByMXzKVolQL5AESoF8gEEhMUKEgBAewwpjDCG84JZT/4kslo3M3Kak3944JUcEucX0oVuy+uAAQoSAEB+886x9zeP8vWTChOeZffp+t9iFOtfZbweZ50T3IHGVEAAiEBUBUoSAEBDSqSvTLtW+dEacxvrWJ5gQqITCoJrIv2oPFNCiNaDtsAAw==",
        "proof": "te6ccgECDwEAAuoACUYDFDrQoUzZUbOHc6UNY0mBN03hQN2AUzu+mETAi0WoaaQAEQEkEBHvVaoAAABlAgMEBQGgm8ephwAAAAAEAQAmclgAAAAAAP////8AAAAAAAAAAGknJzQAAAM/2VQ/AAAAAz/ZVD8FmIHi3wAADqgAJnJTAAbGbsQAAAALAAAAAAAAAe4GIhG45I37R0OqOAQHCCqKBPJXls3n2nMBoSAVpm161PvUldOZGxfUnUNW+a3ZsJCcxNmyfGMJgDj94n9sOGymXqpusTNIYRovSvmER9xixlMAIQAhCQokiUoz9vsktbAj6EZp7YYwMNisQgOwznozBqTietChb3Mlo4qwVMOF3ncpeUBT+xgaArv7OKBYHvK2gXOiqdAl5zTUZbqIoAsMDQ4AmAAAAz/ZRPzFACZyVxTnzFbeDkQLxDZ2r/h6vsHRYb83SLKZaKqjeotGWva9goHb0GSk8z6vpq1wankIkszZiOJh+pbqGW7yKQ2OtucoSAEBhVHXtJGEnTwQl/symAlc9s4Z+yAkuPw3+lKM5+I6BugAAihIAQG+AH4shQPwYnHEpGBkQ/pX69Wbv1jcq4xyYLSOSQfQFAAAaIwBA/JXls3n2nMBoSAVpm161PvUldOZGxfUnUNW+a3ZsJCcKGFheM2s4v9Ue2LRW+ECYb9tqESIgHlgxlfNi4BZJMEAIQAOaIwBA8TZsnxjCYA4/eJ/bDhspl6qbrEzSGEaL0r5hEfcYsZTfQ+PzFy6TKEJ8mR36QXmG6setGyufYrHrLfeF2c5x5IAIQAPKEgBAQu5XRVKsCYGnFpRKLse17BdJFiznkLSyXNYg3wsR3PTAAQoSAEBrksygOVuL6+D9BSm49q+nV++GJdlRMBf7RIazLhbU/wAAChIAQH6qVHjZ2YmL88zDlM9WMRJ74yjePe9FZ+xgUZmhUj9xwAHKEgBAeXt1Jeb5NgwH1irWsl5C1wBkXxHySnRUDh7Rd65xKjzAAU=",
        "state_proof": "te6ccgECoAEAEQ0ACUYDxNmyfGMJgDj94n9sOGymXqpusTNIYRovSvmER9xixlMAIQEkW5Ajr9QAAABlAP////8AAAAAAAAAAAAmclgAAAAAaScnNAAAAz/ZVD8FACZyU1ACAwQFKEgBAWZFLfLbSX3XmeM2J8M5uRmj83hcoLacesXLxdx8jmllAAEoSAEBeUnlK9M+FsWfP/BwRZn6hb9szcTNrD0vH4TOKQyY4nIAICIzAAAAAAAAAAD//////////4AvyfsneDiWGCgKBiRVzCaqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqsAX6Q14HjW8BgcICQooSAEB+EGaqGfGMj4qCC0h305U+Wt7UAq0Rp94wnRV7yFs+gQAGChIAQE3+OUWAEPhOEkXmRlAlFawLU4sIoGAGarKNV336Y8tLQAEIgPNQAsMIr8AATsu1moAAA6oYAAAZ/son5ioAAAEIgu35DAANjN2ey1eJe0dgm+1mW6atw4XTd13P6/GoysJNl9gwCm1Hay/m0TeZw803/fVA1ICI8ORAtGX0JwKINXEVCR53yzHgL4YGShIAQGyDjajs2pM3uYBEGxkLpBxiwpY2vIAdT27MYn5VrSUtgABIgEgDQ4iASAiIyhIAQGCQAimMXsnQUhSI8obOGjjYOlPgDr2n7B4yMCw1mK5HQAMIgEgDxAoSAEBfZAbb6t+KPMBaSCs5webS5GnwgSP5B6x0II77Onis9QABCIBIBESKEgBAe0pwxyisuee67BTz37EHTKaNlEzOpHnmDg2ZkawGes9AAIiASATFCIBIBUWKEgBARhp9cTOdKq+TOoyrrGKr1Lcs1ZM6K3sXYcMYsrbU5eVAAYBASAXKEgBAU5nty5f15xjEK13bHbt/rxK60HEHztBg5V8zLAze36KAAEAJMIBAAAA+gAAAPoAAAPoAAAAByITxUAAAM/2UT8xYBobKEgBAQouWqIObQmIt02ChRunih2rkNUpie+DsMB959w21uJXAAgiESAAAFYsU8V4sBwdKEgBAb7NIVIii1GvzBa0hAfB1ZtEvifIYhtIPWbn8KcIpvpTABMiESAAAChszeZIsB4fKEgBAYJEMHXne0VNuKixKea8oWG0jsU980rVOzBvBZ9O83QNABQiESAAABOsLsWIsCAhKEgBAfyUv1vwfbkvGfiaYicYfmhQsIpmSQ5DC/8NIDNhgsd5ABMiESAAAAnKDhBosFhZIhEgAAATrC7FiLB8fQEDoUAkKEgBASQRYdt5RQVZBxGAuETmjWCv71WaGVPk2+vtyplOvEGBAAEBKxJpEbqOaRK6jgAaABoAAAAAACesWsAlAgLLJicCASAoKQIBIEZHAgEgKisCASA4OQIBICwtAgEgMjMCASAuLwIBIDAxAFsU46BJ4oTXV/zejBYe/yWDmKz5xEpNiy9eLAVhqq+ZWMHsXOd/AAAAAAAAYahgAFsU46BJ4okSYsaF+LzCs03DtlxzTIqvjE81wyCpfJiqmckYH2P3wAAAAAAAYahgAFsU46BJ4oZ5VYaUw998DEvlV3uWm63OkM8hyKfPLsZ9UcmjOF7WAAAAAAAAYahgAFsU46BJ4pzK1yo5hWaXjs5JPQLUXXDh46FbZoWgMai/OQcVLOTvQAAAAAAAYahgAgEgNDUCASA2NwBbFOOgSeKygyl+YIm5pmczP3UsnLC1wvEyebEpJYZGaKcisR2E9EAAAAAAAGGoYABbFOOgSeKITIVriyBU3XRRkOPAtoFzRg4VCIw6NqgSM7sqVQawfUAAAAAAAGGoYABbFOOgSeKUQqAzI6L+KrL0Rrhk7zH57C6AdStRSFVSMTs4vf4p9AAAAAAAAGGoYABbFOOgSeKB1LUzCnV/CKEyRCf4f+KfO/vCt0n2fh8gDxabtVB6a4AAAAAAAGGoYAIBIDo7AgEgQEECASA8PQIBID4/AFsU46BJ4pdINZGVGisJ+p7IuOeOqQUlH3013VOp0jfUG0vU7IBxwAAAAAAAYahgAFsU46BJ4rDhd53KXlAU/sYGgK7+zigWB7ytoFzoqnQJec01GW6iAAAAAAAAYahgAFsU46BJ4obnoGqrF53Fny0IjyL61xvHLY+7Ezv79j5tKrl/QUJrwAAAAAAAYahgAFsU46BJ4oTwiv3Kcn0pxSCinmvAh+Cbk9+ltX83T6/ZQF79UnAOgAAAAAAAYahgAgEgQkMCASBERQBbFOOgSeKtPtQYBgXlx3SsCgd67ESbbNNDA8Xt/i6wyzdmyEBNC8AAAAAAAGGoYABbFOOgSeKEYUykV0/W2fylQr3MDjTFv4z2bcWgV6KvfSFNrSdh28AAAAAAAGGoYABbFOOgSeKY5lnGbsO4ZzPbb4QiXLxH3UgA/nC7zuDwK1IcKGWwZUAAAAAAAGGoYABbFOOgSeKGFx0hQEwugKUvCSEha/NYFBCIgcJ9VoeimpUsGFL+CIAAAAAAAGGoYAIBIEhJAgHUVlcCASBKSwIBIFBRAgEgTE0CASBOTwBbFOOgSeKaieFnpNoXGnKMY1yRqCZ/G3tPBqNOUTvwX6Yg2MWPT0AAAAAAAGGoYABbFOOgSeKs9sesxDsLJMzuwsXdv+TaKiARlNvcaksbvEStUr5r6cAAAAAAAGGoYABbFOOgSeK6ng488ooVv4mCpBukyEJHiMKzfXusQZUunadXtA98IEAAAAAAAGGoYABbFOOgSeKHesk8mW2bcfkisFSVOzfh5agbR00AE/FNrSpFvyj7DkAAAAAAAGGoYAIBIFJTAgEgVFUAWxTjoEnijnQcy/847CQaNtGTvcyd0T7sp3LU4PhAJjVEtrL25cGAAAAAAABhqGAAWxTjoEnijoWGANVK9HGeKzYgjGuUamgDRDOnjm7THwgcfCYXYy7AAAAAAABhqGAAWxTjoEnigC821mGxzDCPyySrJSp1Xb8T9a+3A3iwYO0XcC+pDKlAAAAAAABhqGAAWxTjoEniribfAQKLur2HjhKZxnW4GNwj9YAdQHC/ra/bis1WNHXAAAAAAABhqGAAWxTjoEnihwvK1AQOmYiS5JQn/fBMPjoOgXUzfodEUYk7Qn/drNoAAAAAAABhqGAAWxTjoEnipziRGoJqutdtvJ4t0DMoV9em+3yX3TblsAsSihS4uJQAAAAAAABhqGAiESAAAAThIIj4sFpbKEgBAZr+FkuZHu8L8sH4jQlXSRV/aR7SYdlkxKfT/U5c7Eq3ABEiESAAAAJwkTigsFxdKEgBAVsY//rn3CI6znoNHiH/azyIhBHO+lP0/9FYFa7owzoVABAiESAAAAE0nEmosF5fKEgBARebJ1bH5/dzuhH+LeRjfOuWJ3aaBk3Z/hxp1kiNtEIEAA8iESAAAACYJ9+wsGBhKEgBASI932jwoGojESK91QlUCuX1S5LYHQUqGxQvTvfvzvJCAA4iESAAAABKqYposGJjKEgBAQdqlbLXYCl15iVAQ/yqxnXyyxB14usu686Hlp+xEeikAA0iESAAAAAh+2rIsGRlKEgBAQtG3AbS4g+MTHrv7k0Rq4CgaIVxjvHtg5dhVmM2M3WUAAwiESAAAAAQCoWKEGZnKEgBAWVFcd1MSqs2E0eWatyYnKkseKhRXvCuOuA/DZ0tSx7dAAsiESAAAAAIGUm6EGhpKEgBAdbnu7RDvSn8eSNiP7+5bTfFw+SEJK5OSWjXKDY0fgIwAAoiESAAAAAESLm6EGprKEgBAdsvuCUQp8SSTNmeyriKpcxUB5IX7zg5epOZ7CATZplBAAkiESAAAAACYHG6EGxtKEgBAXxe/cRi6KK5IQRyiD/pTG4XdMsGT4SG1LtcJHZHLjsSAAgiESAAAAABRD/SEG5vKEgBAdtYw4asSbYJLFDg1tEqQCbGXS2ax3ZJuPQwBVv+1zm/AAciESAAAAAAlq46EHBxKEgBAdDx0HwPl1EZEUwvAj4xpOONSZHMFxlgCiGIe1Y7U5vWAAYiESAAAAAAOyC6EHJzKEgBAcvnaiYaAxQ3B98bLUVDgKEx7l3M8cVjNXMv6+CCIOEWAAUiESAAAAAAHJw6EHR1KEgBAVUKOhlv8+ZKJvcIWtxvZnjmF7FLkvSjA5VyBipuJat8AAQiESAAAAAADVn6EHZ3KEgBAcEWG68YFWqLZtFl++1oKDlluzF8DJaulDC9/wknbzaKAAMiESAAAAAABbjaEHh5KEgBAR9ZOQph8tghaeSQMQxXfLXTSnDcgFjxiXMWJK+6WPZFAAIiESAAAAAAAehJ0Hp7KEgBAXCndoI9woETrqwApipnFH+Tmxf09r8hSDu7RQ/BSzuKAAEAqSAAAAAAAAAAEAAAAAAAAAAAAAAABkRfrZ2cd6sIDmgTj74U3FyZ13Nycu6u5F+T2YDPRCHBKBnNEldXTmcA/RfAf9OT76ZgNwARPMGTNXWSdQLmcggoSAEBAO6uC4vaWnhKeWD0rStSCo/EfBxa62VcGOm2NX72xPYAACIRAAAADqkJSGCwfn8oSAEBoLtLYRdm5jftjLa4H3lTGQGQwnUwNAN0Ri6+xkPutLEAESIRAAAADDcuFxiwgIEoSAEB90knDZ2dp9paMCWTYDWWvZONY60Qn4aTq2plewsTZYAAECIRAAAACvuxUdiwgoMoSAEBNtvdrrI+C9Z0dr0A2nzHkKNPlUDVumDqTj8+LD9X0xEADyIRAAAACmNnHRiwhIUoSAEBtjzggBWsqY3p7d8yFsGgR7ggdaRVfmPaQ8frvWiIQZYADihIAQE6mZFFJFRHWGWFqJXa4OJVPgAcMyhbWWqUIBtxQYfxJQANIhEAAAAKY2cdGLCGhyIRAAAACj3fzHCwiIkoSAEBGBpB1DP8ZCjVHGd/fmGVFCIchH/Aea4UF+JXi32+YnUADCIRAAAACimrEbCwiosoSAEBrrAfMXZ4iP31NIAFH6Vhxh8LgeB6oL4omff2NcDDBlQACyhIAQE9uMVeslxIkC0meYiEhiPlGtEf8+WqljbVJBBiFCSniwAKIhEAAAAKKasRsLCMjShIAQHkAz4bDNDceu55l8dYl3NdkRbk9sF/Qx0lqNo7hyJAvAAJIhEAAAAKKasRsLCOjyIRAAAACicTT9CwkJEoSAEBREEQrI+FV91OTWCpYrCfjOM+0ee7w8OkM8PVCLoOxbYACCIRAAAACiXHbuCwkpMoSAEBAhRHjn1rW/lYpKF5WjkH4vxqz5DJj+QT5rDqgaClXCIAByIRAAAACiUjZrCwlJUoSAEB1mVEmuiJ//4EW/od4DTIDNv5PDJK8qh2ElXjYsyUb/AABiIRAAAACiTRYpiwlpcoSAEBvq+rF6aGan4t4XVmoi4/xuZrB9sHUtgW5n/9CJ62QqUABSIRAAAACiSnbGiwmJkoSAEBU5uo48P20IH7K02FnZ+NJzEBm812g6Z/ioUm1+qhJR8ABChIAQGH9y88pmC2h2dy/gmJrp8GX/h094itV/xdRiMeHUaa1QADIhEAAAAKJKdsaLCamyhIAQFsfNF/DMc16E682MOpNK9d4x2hi9xFa3elgbl1FcfhcwACIhEAAAAKJKdsaLCcnSIRAAAACiSb+riwnp8oSAEBy3aAL2jJLbRmUPhK6ceY2mGZN5DEw66bTSoxW5AnkvEAAShIAQHiUKS0BoP9z1FfEUiK9mh7gk9PKBAh6rY8zFE40XVTjQAAAKkAAAAKJJv6uKAAAAUSTf1cUABCYNglKt0B3JgCWAY1ThZnRzncl22/8vkiBGrspMQe0bk0LbdlK0wQq7iN/q/fT192YpQ0HjYESuvPkqRtTCMDW9lY"
      }
    ],
    "@extra": "1764173631.164459:1:0.7102170528257488"
  }
}
*/
