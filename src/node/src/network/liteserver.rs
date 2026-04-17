/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    block::{
        create_prevblocks_proof, create_shard_proof, get_block_proof,
        get_last_liteserver_state_block, get_shard_block_proof, header_proof,
        proof_mc_to_shard_top, proof_shard_prev_link, visit_prev_blocks_info, BlockIdExtExtention,
        BlockStuff, HeaderProofKind,
    },
    engine_traits::{EngineOperations, Stoppable},
    error::NodeError,
    shard_states_keeper::PinnedShardStateGuard,
    types::awaiters_pool::AwaitersPool,
    validating_utils::UNREGISTERED_CHAIN_MAX_LEN,
};
use adnl::{
    common::{AdnlPeers, Answer, QueryAnswer, QueryResult, Subscriber, TaggedObject, TimedAnswer},
    server::{AdnlServer, AdnlServerConfig, AdnlServerConfigJson},
};
use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap, HashSet},
    fmt::{Display, Formatter},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
    vec,
};
use ton_api::{
    deserialize_boxed,
    ton::{
        lite_server::{
            accountdispatchqueueinfo::AccountDispatchQueueInfo,
            accountid::AccountId as AccountIdTl, accountstate::AccountState,
            allshardsinfo::AllShardsInfo, blockdata::BlockData, blockheader::BlockHeader,
            blockoutmsgqueuesize::BlockOutMsgQueueSize, blockstate::BlockState,
            blocktransactions::BlockTransactions, blocktransactionsext::BlockTransactionsExt,
            configinfo::ConfigInfo, currenttime::CurrentTime, dispatchqueueinfo::DispatchQueueInfo,
            dispatchqueuemessage::DispatchQueueMessage,
            dispatchqueuemessages::DispatchQueueMessages, error::Error as LSError,
            libraryentry::LibraryEntry, libraryresult::LibraryResult,
            libraryresultwithproof::LibraryResultWithProof, lookupblockresult::LookupBlockResult,
            masterchaininfo::MasterchainInfo, masterchaininfoext::MasterchainInfoExt,
            outmsgqueuesize::OutMsgQueueSize, outmsgqueuesizes::OutMsgQueueSizes,
            partialblockproof::PartialBlockProof, runmethodresult::RunMethodResult,
            sendmsgstatus::SendMsgStatus, shardblocklink::ShardBlockLink,
            shardblockproof::ShardBlockProof, shardinfo::ShardInfo, transactionid::TransactionId,
            transactionid3::TransactionId3, transactioninfo::TransactionInfo,
            transactionlist::TransactionList, transactionmetadata::TransactionMetadata,
            validatorstats::ValidatorStats, version::Version, BlockTransactions as BlockTxEnum,
            BlockTransactionsExt as BlockTxExtEnum, Error as ErrorEnum,
        },
        rpc::lite_server::{
            GetAccountState, GetAccountStatePrunned, GetAllShardsInfo, GetBlock, GetBlockHeader,
            GetBlockOutMsgQueueSize, GetBlockProof, GetConfigAll, GetConfigParams,
            GetDispatchQueueInfo, GetDispatchQueueMessages, GetLibraries, GetLibrariesWithProof,
            GetMasterchainInfo, GetMasterchainInfoExt, GetOneTransaction, GetOutMsgQueueSizes,
            GetShardBlockProof, GetShardInfo, GetState, GetTime, GetTransactions,
            GetValidatorStats, GetVersion, ListBlockTransactions, ListBlockTransactionsExt,
            LookupBlock, LookupBlockWithProof, Query, QueryPrefix, RunSmcMethod, SendMessage,
            WaitMasterchainSeqno,
        },
        ton_node::{blockid::BlockId, zerostateidext::ZeroStateIdExt},
    },
    AnyBoxedSerialize, Constructor, IntoBoxed, TLObject,
};
use ton_block::{
    error, fail, find_leaf, read_single_root_boc, write_boc, write_boc_multi, Account, AccountId,
    AccountIdPrefixFull, Block, BlockError, BlockIdExt, BlockInfo, BocWriter, BuilderData, Cell,
    CurrencyCollection, Deserializable, HashmapAugType, HashmapType, IBitstring, MerkleProof,
    MsgAddrStd, MsgAddressInt, Result, Serializable, ShardIdent, ShardStateUnsplit, SliceData,
    Transaction, Transactions, UInt256, UsageTree, INVALID_WORKCHAIN_ID, MASTERCHAIN_ID,
};
use ton_vm::{smart_contract_info::convert_stack, stack::StackItem};

const SEQNO_ANY: u32 = u32::MAX;

const RUN_SMC_METHOD_PROOFS: i32 = 0x1;
const RUN_SMC_METHOD_STATE_PROOF: i32 = 0x2;
const RUN_SMC_METHOD_RESULT: i32 = 0x4;
const RUN_SMC_METHOD_INIT_C7: i32 = 0x8;
const RUN_SMC_METHOD_LIB_EXTRAS: i32 = 0x10;
const RUN_SMC_METHOD_FULL_C7: i32 = 0x20;
const RUN_SMC_METHOD_SUPPORTED: i32 = 0x3f;
const RUN_SMC_METHOD_ERROR_CODE: i32 = -0x100;

pub type LookupMode = i32;

pub const LOOKUP_BY_SEQNO: i32 = 0x1;
pub const LOOKUP_BY_LT: i32 = 0x2;
pub const LOOKUP_BY_UTIME: i32 = 0x4;
pub const LOOKUP_INCLUDE_PREV: i32 = 0x8;
pub const LOOKUP_BY_MASK: i32 =
    LOOKUP_BY_SEQNO | LOOKUP_BY_LT | LOOKUP_BY_UTIME | LOOKUP_INCLUDE_PREV;

#[inline]
fn lkp_check(mode: LookupMode) -> Result<()> {
    let by = mode & LOOKUP_BY_MASK;
    if by == 0 || (by & (by - 1)) != 0 {
        fail!("exactly one of LookupBySeqno, LookupByLt, LookupByUtime must be set");
    }
    Ok(())
}

#[inline]
fn lkp_has(mode: LookupMode, flag: i32) -> bool {
    (mode & flag) != 0
}

pub const MAX_TRANSACTION_COUNT: usize = 16;

const FILTER_BY_SHARD: i32 = 1;

const WANT_PROOF: i32 = 0x20;
const REVERSE_ORDER: i32 = 0x40;
const AFTER_PRESENT: i32 = 0x80;
const LS_VERSION: i32 = 0x101;
const LS_CAPABILITIES: i64 = 7;
const SKIP_EXTERNALS_QUEUE_SIZE: i32 = 1000;

const CFG_NEED_PREV_BLOCKS: i32 = 0x80;
const CFG_FROM_PREV_KEY_BLOCK: i32 = 0x8000; // read the config from the previous key block
const CFG_VISIT_PARAMS: i32 = 0x10000; // enable the listed parameters (list)
const CFG_VISIT_ROOT: i32 = 0x20000; // enable the config root (all at once)
const CFG_MODE_MASK_RET: i32 = 0xFFFF; // the mask of the lower 16 bits to be returned

const TID_ACCOUNT: i32 = 1 << 0;
const TID_LT: i32 = 1 << 1;
const TID_HASH: i32 = 1 << 2;

const LIB_MODE_SKIP_DATA: i32 = 1 << 1; // 2 — if set, we do not return the cell data.

const WANT_PROOF_BIT: i32 = 0x1;
const ONE_ACCOUNT_BIT: i32 = 0x2;
const MESSAGES_BOC_BIT: i32 = 0x4;

fn make_liteserver_error(code: i32, message: String) -> TLObject {
    ErrorEnum::LiteServer_Error(LSError { code, message }).into_tl_object()
}

fn format_block_for_error(id: &BlockId) -> String {
    let shard_u64 = id.shard as u64;
    let shard_no_tag = ShardIdent::with_tagged_prefix(id.workchain, shard_u64)
        .map(|s| s.shard_prefix_without_tag())
        .unwrap_or(shard_u64);
    format!("({},{:016x})", id.workchain, shard_no_tag)
}

fn map_liteserver_error(err: &anyhow::Error) -> (i32, String) {
    const CODE: i32 = 500;
    const PREFIX: &str = "LITE_SERVER_";

    let msg = err.to_string();
    if msg.starts_with(PREFIX) {
        return (CODE, msg);
    }

    if let Some(block_err) = err.downcast_ref::<BlockError>() {
        let mapped = match block_err {
            BlockError::InvalidArg(text) => format!("LITE_SERVER_INVALID_PARAMS: {text}"),
            BlockError::InvalidData(text) => format!("LITE_SERVER_INVALID_DATA: {text}"),
            BlockError::NotFound(text) => format!("LITE_SERVER_NOTFOUND: {text}"),
            _ => format!("LITE_SERVER_UNKNOWN: {msg}"),
        };
        return (CODE, mapped);
    }

    if let Some(node_err) = err.downcast_ref::<NodeError>() {
        let mapped = match node_err {
            NodeError::Timeout(_) => "LITE_SERVER_TIMEOUT".to_string(),
            NodeError::InvalidArg(text) => format!("LITE_SERVER_INVALID_PARAMS: {text}"),
            NodeError::InvalidData(text) => format!("LITE_SERVER_INVALID_DATA: {text}"),
            NodeError::InvalidOperation(text) => format!("LITE_SERVER_INVALID_OPERATION: {text}"),
            NodeError::ValidatorReject(text) => format!("LITE_SERVER_REJECTED: {text}"),
            NodeError::ValidatorSoftReject(text) => format!("LITE_SERVER_SOFT_REJECTED: {text}"),
        };
        return (CODE, mapped);
    }

    (CODE, format!("LITE_SERVER_UNKNOWN: {msg}"))
}

fn make_liteserver_error_from(err: &anyhow::Error) -> TLObject {
    let (code, message) = map_liteserver_error(err);
    make_liteserver_error(code, message)
}

fn make_notready_block_msg(
    id: &BlockId,
    kind: &str,
    value: impl std::fmt::Display,
    last_mc_seqno: u32,
) -> String {
    format!(
        "LITE_SERVER_NOTREADY: cannot find block {} {}={}: ltdb: block not found (last known masterchain block: {})",
        format_block_for_error(id),
        kind,
        value,
        last_mc_seqno,
    )
}

async fn load_block_by_id(
    engine: &Arc<dyn EngineOperations>,
    id: &BlockIdExt,
) -> Result<BlockStuff> {
    let handle = engine.load_block_handle(id)?.ok_or_else(|| error!("no handle for {}", id))?;
    engine.load_block(&handle).await
}

fn make_shard_descr_proof(
    mc_state_root: &Cell,
    shard_ident: &ShardIdent,
    expected_descr: &ton_block::ShardDescr,
) -> Result<MerkleProof> {
    let usage = UsageTree::with_root(mc_state_root.clone());

    let state: ShardStateUnsplit = ShardStateUnsplit::construct_from_cell(usage.root_cell())
        .map_err(|e| error!("construct_from_cell(mc_state) failed: {e}"))?;

    let custom = state
        .read_custom()
        .map_err(|e| error!("read_custom(mc_state) failed: {e}"))?
        .ok_or_else(|| error!("No custom in masterchain state"))?;

    let shards = custom.shards();

    let rec = shards
        .get_shard(shard_ident)?
        .ok_or_else(|| error!("shard {shard_ident} not found in mc state"))?;

    let got = rec.descr();
    if got.seq_no != expected_descr.seq_no || got.root_hash != expected_descr.root_hash {
        log::warn!(
            "make_shard_descr_proof: descr mismatch; expected (seqno={}, rh={:x}) got (seqno={}, rh={:x})",
            expected_descr.seq_no,
            expected_descr.root_hash,
            got.seq_no,
            got.root_hash,
        );
    }

    MerkleProof::create_by_usage_tree(mc_state_root, &usage)
        .map_err(|e| error!("create_by_usage_tree failed: {e}"))
}

fn create_block_shards_proof(block_root: &Cell) -> Result<Vec<u8>> {
    let usage = UsageTree::with_root(block_root.clone());

    let block = Block::construct_from_cell(usage.root_cell())
        .map_err(|e| error!("Block::construct_from_cell failed: {e}"))?;

    let extra = block.read_extra().map_err(|e| error!("read_extra failed: {e}"))?;
    let mc_extra = extra
        .read_custom()
        .map_err(|e| error!("read_custom failed: {e}"))?
        .ok_or_else(|| error!("no mc_extra in block.extra.custom"))?;

    let _shards = mc_extra.shards();

    let proof = MerkleProof::create_by_usage_tree(block_root, &usage)
        .map_err(|e| error!("create_by_usage_tree(block_root) failed: {e}"))?;

    proof.write_to_bytes().map_err(|e| error!("write_to_bytes failed: {e}"))
}

#[inline]
async fn state_header_proof(engine: &Arc<dyn EngineOperations>, id: &BlockIdExt) -> Result<Cell> {
    let handle = engine.load_block_handle(&id)?.ok_or_else(|| error!("no handle for {}", id))?;
    let bp = engine.load_block_proof(&handle, !id.is_masterchain()).await?;
    let (_blk, virt_root) = bp.virtualize_block()?;
    // This proof is so lightweight that we can build it without spawn_blocking
    let proof = create_state_proof(&virt_root)?;
    Ok(proof.serialize()?)
}

struct ResolvedAccount {
    id: BlockIdExt,
    shardblk: BlockIdExt,
    mc_state: PinnedShardStateGuard,
    _shard_state: PinnedShardStateGuard,
    shard_proof: Option<Vec<u8>>,
    proof: Option<Vec<u8>>,
    account_cell: Option<Cell>,
    gen_utime: u32,
    gen_lt: u64,
}

async fn resolve_account_state(
    engine: &Arc<dyn EngineOperations>,
    block_id: BlockIdExt,
    account_address: &MsgAddressInt,
    workchain: i32,
    need_proofs: bool,
) -> Result<ResolvedAccount> {
    let id;
    let shardblk;
    let shard_state_guard: PinnedShardStateGuard;
    let mc_state_guard: PinnedShardStateGuard;
    let mut shard_proof = None;

    if !block_id.shard_id.is_masterchain() {
        // Reference block is not from masterchain — it must be fully specified
        // and exactly this block must contain the account state
        shardblk = block_id.clone();
        shard_state_guard = engine.load_and_pin_state(&block_id).await?;
        let master_ref = shard_state_guard
            .state()
            .state()?
            .master_ref()
            .ok_or_else(|| error!("shard state has no master_ref"))?;
        let mc_block_id = BlockIdExt::from_ext_blk(master_ref.master.clone());
        id = mc_block_id.clone();
        mc_state_guard = engine.load_and_pin_state(&mc_block_id).await?;
    } else {
        id = if block_id.seq_no != SEQNO_ANY {
            // Reference block is specified
            block_id.clone()
        } else {
            // Reference block is not specified, use last known mc state
            (*get_last_liteserver_state_block(engine)?).clone()
        };
        mc_state_guard = engine.load_and_pin_state(&id).await?;

        if workchain == MASTERCHAIN_ID {
            // Account is in masterchain — read directly from the reference block state
            shard_state_guard = mc_state_guard.clone();
            shardblk = id.clone();
        } else {
            // Find the shard containing the account
            let mut sb_id = None;
            for top_id in mc_state_guard.state().top_blocks(account_address.workchain_id())? {
                if top_id.shard().contains_address(account_address)? {
                    sb_id = Some(top_id);
                    break;
                }
            }
            shardblk =
                sb_id.ok_or_else(|| error!("No shard found for address {account_address}"))?;
            shard_state_guard = engine.load_and_pin_state(&shardblk).await?;

            // Build shard proofs (mc block→state + mc state→shard)
            if need_proofs {
                let mc_state_proof_cell = state_header_proof(engine, &id).await?;
                let shard_id = shardblk.shard_id.clone();
                let mc_state_root = mc_state_guard.state().root_cell().clone();
                shard_proof = Some(
                    tokio::task::spawn_blocking(move || {
                        let shard_header = create_shard_proof(&mc_state_root, &shard_id)?;
                        BocWriter::with_roots([mc_state_proof_cell, shard_header.serialize()?])?
                            .write_to_vec()
                    })
                    .await??,
                );
            }
        }
    }

    let state_proof_cell =
        if need_proofs { Some(state_header_proof(engine, &shardblk).await?) } else { None };

    // Lookup account in the shard state and build its proof
    let shard_root = shard_state_guard.state().root_cell().clone();
    let addr = account_address.clone();
    tokio::task::spawn_blocking(move || {
        let usage_tree = UsageTree::with_params(shard_root.clone(), true);
        let state = ShardStateUnsplit::construct_from_cell(usage_tree.root_cell())?;
        let shard_account = state.read_accounts()?.account(&addr.address())?;
        let account_cell = shard_account.map(|acc| acc.account_cell());

        // proof: BOC with 2 roots (shard block→state proof + state→account proof)
        let proof = state_proof_cell
            .map(|spc| {
                let acc_proof = MerkleProof::create_by_usage_tree(&shard_root, &usage_tree)?;
                BocWriter::with_roots([spc, acc_proof.serialize()?])?.write_to_vec()
            })
            .transpose()?;

        let gen_utime = state.gen_time();
        let gen_lt = state.gen_lt();

        Ok(ResolvedAccount {
            id,
            shardblk,
            mc_state: mc_state_guard,
            _shard_state: shard_state_guard,
            shard_proof,
            proof,
            account_cell,
            gen_utime,
            gen_lt,
        })
    })
    .await?
}

fn visit_block_info(block: &Block) -> Result<BlockInfo> {
    let info = block.read_info()?;
    let _prev = info.read_prev_ref()?;
    let _vprev = info.read_prev_vert_ref()?;
    let _mc_ref = info.read_master_ref()?;
    let _su = block.read_state_update()?;
    Ok(info)
}

/// Find nearest transaction cell using directed dict traversal (no full deserialization).
/// Returns (lt, transaction_cell) or None if no matching entry found.
fn find_nearest_tx_cell(
    transactions: &Transactions,
    lt: u64,
    forward: bool,
) -> Result<Option<(u64, Cell)>> {
    let root = match transactions.data() {
        Some(r) => r.clone(),
        None => return Ok(None),
    };
    let key = lt.write_to_bitstring()?;
    let mut path = BuilderData::new();
    let next_index = if forward { 0 } else { 1 };
    let result =
        find_leaf::<Transactions>(root, &mut path, 64, key, next_index, false, false, &mut 0usize)?;
    match result {
        Some(mut slice) => {
            let found_lt = u64::construct_from_cell(path.into_cell()?)?;
            CurrencyCollection::skip(&mut slice)?;
            let tx_cell = slice.reference(0)?;
            Ok(Some((found_lt, tx_cell)))
        }
        None => Ok(None),
    }
}

fn create_state_proof(block_root: &Cell) -> Result<MerkleProof> {
    let usage_tree = UsageTree::with_params(block_root.clone(), true);
    let block = Block::construct_from_cell(usage_tree.root_cell())?;
    let _info = visit_block_info(&block)?;
    MerkleProof::create_by_usage_tree(block_root, &usage_tree)
}

async fn spawn_and_read_boc(data: Vec<u8>) -> Result<Cell> {
    tokio::task::spawn_blocking(move || read_single_root_boc(&data)).await?
}

#[derive(serde::Deserialize, serde::Serialize)]
pub struct LiteServerConfigJson {
    #[serde(flatten)]
    adnl: AdnlServerConfigJson,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_parallel_fast_queries: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_parallel_slow_queries: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account_state_cache_size_mb: Option<u64>,
}

impl LiteServerConfigJson {
    pub fn from_server_config(server_config: AdnlServerConfigJson) -> Self {
        Self {
            adnl: server_config,
            max_parallel_fast_queries: None,
            max_parallel_slow_queries: None,
            account_state_cache_size_mb: None,
        }
    }
}

pub struct LiteServerConfig {
    adnl: AdnlServerConfig,
    max_parallel_fast_queries: u64,
    max_parallel_slow_queries: u64,
    account_state_cache_size_mb: u64,
}

impl LiteServerConfig {
    const DEFAULT_MAX_PARALLEL_FAST_QUERIES: u64 = 256;
    const DEFAULT_MAX_PARALLEL_SLOW_QUERIES: u64 = 16;
    const DEFAULT_ACCOUNT_STATE_CACHE_SIZE_MB: u64 = 256;

    pub fn from_json_config(json_config: &LiteServerConfigJson) -> Result<Self> {
        Ok(Self {
            adnl: AdnlServerConfig::from_json_config(&json_config.adnl)?,
            max_parallel_fast_queries: json_config
                .max_parallel_fast_queries
                .unwrap_or(Self::DEFAULT_MAX_PARALLEL_FAST_QUERIES),
            max_parallel_slow_queries: json_config
                .max_parallel_slow_queries
                .unwrap_or(Self::DEFAULT_MAX_PARALLEL_SLOW_QUERIES),
            account_state_cache_size_mb: json_config
                .account_state_cache_size_mb
                .unwrap_or(Self::DEFAULT_ACCOUNT_STATE_CACHE_SIZE_MB),
        })
    }
}

pub struct LiteServer {
    adnl: AdnlServer,
}

impl LiteServer {
    pub async fn with_params(
        config: LiteServerConfig,
        runtime: tokio::runtime::Handle,
        engine: Arc<dyn EngineOperations>,
    ) -> Result<Self> {
        let subscriber = LiteServerQuerySubscriber::new(
            runtime,
            engine,
            config.max_parallel_fast_queries,
            config.max_parallel_slow_queries,
            config.account_state_cache_size_mb,
        )?;
        let ret = Self {
            adnl: AdnlServer::listen(
                config.adnl,
                vec![Arc::new(subscriber)],
                #[cfg(feature = "telemetry")]
                "LiteServer",
            )
            .await?,
        };
        Ok(ret)
    }
    pub async fn shutdown(self) {
        self.adnl.shutdown().await
    }
}

#[async_trait::async_trait]
impl Stoppable for LiteServer {
    fn name(&self) -> &'static str {
        "liteserver"
    }
    async fn shutdown(self: Box<Self>) {
        (*self).shutdown().await;
    }
}

macro_rules! route {
    // q - TL-object that we are working with (the variable must be `mut`)
    // s - &LiteServerQueryImpl
    ($q:ident,
        $( $ty:ty => |$req:ident| $handler:expr ),* $(,)?
    ) => {{
        let start = Instant::now();
        log::trace!("Processing query {:?}", $q);
        let mut $q = $q;
        loop {
            $(
                match $q.downcast::<$ty>() {
                    // we have "caught" the desired type - we execute the handler
                    Ok($req) => {
                        let obj = match $handler.await {
                            Ok(reply) => {
                                log::trace!(
                                    "liteserver: {} done in {}ms",
                                    stringify!($ty),
                                    start.elapsed().as_millis()
                                );
                                reply.into_boxed().into_tl_object()
                            }
                            Err(e) => {
                                log::error!(
                                    "liteserver: {} failed in {}ms: {e:#}",
                                    stringify!($ty),
                                    start.elapsed().as_millis()
                                );
                                make_liteserver_error_from(&e)
                            }
                        };
                        break Some(Answer::Object(TaggedObject::from(obj)));
                    }
                    // if it doesn't fit, we're trying further
                    Err(rest) => $q = rest,
                }
            )*
            log::warn!("Unsupported LiteServerQuery: {:?}", $q);
            break None
        }
    }};
}

// Request coalescing key for GetAccountState.
// When multiple identical requests arrive in parallel, only one computes the result —
// the rest await via AwaitersPool.
#[derive(Clone, Hash, Eq, PartialEq, Ord, PartialOrd)]
struct AccountCacheKey {
    block_root_hash: UInt256,
    account_hash: UInt256,
    workchain: i32,
    prune: bool,
}

impl Display for AccountCacheKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "acct:{}@{}",
            self.account_hash.to_hex_string(),
            self.block_root_hash.to_hex_string()
        )
    }
}

pub(crate) fn account_state_byte_size(state: &AccountState) -> u64 {
    (state.shard_proof.len() + state.proof.len() + state.state.len()) as u64
}

// Shared wait registry for waitMasterchainSeqno queries.
//
// Problem: clients wrap queries with waitMasterchainSeqno(seqno, timeout).
// Many such tasks spawned in paraller are doing similar redundant work
// (load_block_handle,seqno comparison, scheduler pressure).
//
// Solution: a single watcher task monitors mc block progression via
// wait_next_applied_mc_block. Incoming wait-queries register in a BTreeMap
// keyed by target seqno. When a new block arrives, all queries with
// seqno <= new_block are drained and dispatched for processing in bulk.
// Per-query timeouts are tracked via deadlines and checked on each block.
//
// Combined with the early seqno check (satisfied waits are dropped before
// registration), this reduces spawns from N-per-query to 1 watcher + 1
// processing task per dispatched query.

struct PendingWaitQuery {
    query: TLObject,
    slow: bool,
    deadline: Instant,
    tx: tokio::sync::oneshot::Sender<Result<TimedAnswer<Answer>>>,
}

impl PendingWaitQuery {
    fn reject(self) {
        let timeout_answer = Ok(TimedAnswer {
            answer: Some(Answer::Object(TaggedObject::from(make_liteserver_error(
                500,
                "LITE_SERVER_TIMEOUT".to_string(),
            )))),
            #[cfg(feature = "telemetry")]
            actual_start_at: Some(Instant::now()),
        });
        self.tx.send(timeout_answer).ok();
    }
}

struct WaitRegistry {
    pending: Mutex<BTreeMap<u32, Vec<PendingWaitQuery>>>,
    watcher_active: AtomicBool,
    // Wakes the watcher when a new query is registered,
    // so it can recalculate the earliest deadline
    notify: tokio::sync::Notify,
}

// Byte-limited LRU. Evicts LRU entries when total_bytes exceeds max_bytes.
struct Lru {
    max_bytes: u64,
    total_bytes: u64,
    cache: lru::LruCache<AccountCacheKey, AccountState>,
}

struct SubscriberContext {
    account_state_awaiters: Arc<AwaitersPool<AccountCacheKey, AccountState>>,
    account_state_lru: Arc<Mutex<Lru>>,
    engine: Arc<dyn EngineOperations>,
    semaphore_fast: Arc<tokio::sync::Semaphore>,
    semaphore_slow: Arc<tokio::sync::Semaphore>,
    wait_registry: Arc<WaitRegistry>,
}

pub struct LiteServerQuerySubscriber {
    context: Arc<SubscriberContext>,
    runtime: tokio::runtime::Handle,
}

impl LiteServerQuerySubscriber {
    pub fn new(
        runtime: tokio::runtime::Handle,
        engine: Arc<dyn EngineOperations>,
        max_parallel_fast_queries: u64,
        max_parallel_slow_queries: u64,
        account_state_cache_size_mb: u64,
    ) -> Result<Self> {
        let lru = Lru {
            cache: lru::LruCache::unbounded(),
            max_bytes: account_state_cache_size_mb.max(1) * 1024 * 1024,
            total_bytes: 0,
        };
        let context = Arc::new(SubscriberContext {
            account_state_awaiters: Arc::new(AwaitersPool::new(
                "account_state_coalesce",
                #[cfg(feature = "telemetry")]
                engine.engine_telemetry().clone(),
                engine.engine_allocated().clone(),
            )),
            account_state_lru: Arc::new(Mutex::new(lru)),
            engine,
            semaphore_fast: Arc::new(tokio::sync::Semaphore::new(
                max_parallel_fast_queries as usize,
            )),
            semaphore_slow: Arc::new(tokio::sync::Semaphore::new(
                max_parallel_slow_queries as usize,
            )),
            wait_registry: Arc::new(WaitRegistry {
                pending: Mutex::new(BTreeMap::new()),
                watcher_active: AtomicBool::new(false),
                notify: tokio::sync::Notify::new(),
            }),
        });
        Ok(Self { context, runtime })
    }

    async fn get_account_state(
        engine: &Arc<dyn EngineOperations>,
        block_id: BlockIdExt,
        account_id: AccountIdTl,
        prune: bool,
        is_slow: Arc<AtomicBool>,
    ) -> Result<AccountState> {
        if !block_id.shard_id.is_masterchain()
            && block_id.shard_id.workchain_id() != account_id.workchain
        {
            fail!(
                "Reference block workchain id must be masterchain \
                or match the account's workchain id"
            );
        }
        if block_id.shard().workchain_id() == INVALID_WORKCHAIN_ID {
            fail!("Reference block id for a getAccountState() is invalid");
        }

        let account_address = MsgAddressInt::AddrStd(MsgAddrStd {
            anycast: None,
            workchain_id: account_id.workchain as i8,
            address: account_id.id.clone().into(),
        });
        if block_id.shard_id.workchain_id() == account_id.workchain
            && !block_id.shard_id.contains_address(&account_address)?
        {
            fail!("Requested account id is not contained in the shard of the reference block");
        }

        let resolved =
            resolve_account_state(engine, block_id, &account_address, account_id.workchain, true)
                .await?;
        let ResolvedAccount { id, shardblk, shard_proof, proof, account_cell, .. } = resolved;
        let proof = proof.expect("always built with need_proofs=true");
        let shard_proof = shard_proof.unwrap_or_default();

        if !is_slow.load(Ordering::Relaxed) {
            if let Some(ref ac) = account_cell {
                let acc = Account::construct_from_cell(ac.clone())?;
                if let Some(si) = acc.storage_info() {
                    if si.used().bits() > 30 * 1024 * 8 || si.used().cells() > 300 {
                        log::info!(
                            "Account {} is big: storage stat: {} bits, {} cells, processing is postponed",
                            account_id.id,
                            si.used().bits(),
                            si.used().cells(),
                        );
                        is_slow.store(true, Ordering::Relaxed);
                        return Ok(AccountState::default());
                    }
                }
            }
        }

        let state = if let Some(account_cell) = account_cell {
            tokio::task::spawn_blocking(move || {
                if !prune {
                    write_boc(&account_cell)
                } else {
                    let usage_tree = UsageTree::with_root(account_cell.clone());
                    let acc = Account::construct_from_cell(usage_tree.root_cell())?;
                    let balance_root = acc
                        .balance()
                        .map(|cc| cc.other.root().map(|c| c.repr_hash()))
                        .flatten()
                        .unwrap_or_default();
                    MerkleProof::create_with_subtrees(
                        &account_cell,
                        |hash| usage_tree.contains(hash),
                        |hash| hash == &balance_root,
                    )?
                    .write_to_bytes()
                }
            })
            .await??
        } else {
            Vec::new()
        };

        Ok(AccountState { id, shardblk, shard_proof, proof, state })
    }

    async fn run_smc_method(
        engine: &Arc<dyn EngineOperations>,
        mode: i32,
        block_id: BlockIdExt,
        account_id: AccountIdTl,
        method_id: i64,
        params: Vec<u8>,
    ) -> Result<RunMethodResult> {
        if block_id.shard().workchain_id() == INVALID_WORKCHAIN_ID {
            fail!("Reference block id for runSmcMethod is invalid");
        }
        if params.len() >= 65536 {
            fail!("more than 64k parameter bytes passed");
        }
        if mode & !RUN_SMC_METHOD_SUPPORTED != 0 {
            fail!("unsupported mode in runSmcMethod");
        }

        let account_address = MsgAddressInt::AddrStd(MsgAddrStd {
            anycast: None,
            workchain_id: account_id.workchain as i8,
            address: account_id.id.clone().into(),
        });

        let resolved = resolve_account_state(
            engine,
            block_id,
            &account_address,
            account_id.workchain,
            mode & RUN_SMC_METHOD_PROOFS != 0,
        )
        .await?;
        let mc_state_root = resolved.mc_state.state().root_cell().clone();

        let lib_extras =
            if mode & RUN_SMC_METHOD_LIB_EXTRAS != 0 { Some(Vec::new()) } else { None };

        let make_result = move |exit_code, result, state_proof, init_c7| RunMethodResult {
            mode,
            id: resolved.id.into(),
            shardblk: resolved.shardblk.into(),
            shard_proof: resolved.shard_proof,
            proof: resolved.proof,
            state_proof,
            init_c7,
            lib_extras,
            exit_code,
            result,
        };

        let empty_result = if mode & RUN_SMC_METHOD_RESULT != 0 { Some(Vec::new()) } else { None };
        let empty_init_c7 =
            if mode & RUN_SMC_METHOD_INIT_C7 != 0 { Some(Vec::new()) } else { None };

        let account_cell = match resolved.account_cell {
            Some(cell) => cell,
            None => {
                let empty_state_proof =
                    if mode & RUN_SMC_METHOD_STATE_PROOF != 0 { Some(Vec::new()) } else { None };
                return Ok(make_result(
                    RUN_SMC_METHOD_ERROR_CODE,
                    empty_result,
                    empty_state_proof,
                    empty_init_c7,
                ));
            }
        };

        tokio::task::spawn_blocking(move || -> Result<RunMethodResult> {
            let account_usage = if mode & RUN_SMC_METHOD_STATE_PROOF != 0 {
                Some(UsageTree::with_params(account_cell.clone(), true))
            } else {
                None
            };
            let account = if let Some(ref usage) = account_usage {
                Account::construct_from_cell(usage.root_cell())?
            } else {
                Account::construct_from_cell(account_cell.clone())?
            };

            if account.get_code().is_none() {
                let state_proof = account_usage
                    .map(|usage| {
                        MerkleProof::create_by_usage_tree(&account_cell, &usage)?.write_to_bytes()
                    })
                    .transpose()?;
                return Ok(make_result(
                    RUN_SMC_METHOD_ERROR_CODE,
                    empty_result,
                    state_proof,
                    empty_init_c7,
                ));
            }

            let input_stack = deserialize_vm_stack_boc(&params)?;
            let input_entries = convert_stack(&input_stack)?;

            let run = ton_vm::run_smc_method(
                &account,
                mc_state_root.clone(),
                method_id as u32,
                input_entries,
                resolved.gen_utime,
                resolved.gen_lt,
            )?;

            // Always serialize stack when state_proof is requested — serialization
            // visits data cells referenced from the result, capturing them in the usage tree
            let result = if mode & (RUN_SMC_METHOD_RESULT | RUN_SMC_METHOD_STATE_PROOF) != 0 {
                let cell = serialize_vm_stack(&run.stack)?;
                if mode & RUN_SMC_METHOD_RESULT != 0 {
                    Some(write_boc(&cell)?)
                } else {
                    None
                }
            } else {
                None
            };

            let state_proof = account_usage
                .map(|usage| {
                    MerkleProof::create_by_usage_tree(&account_cell, &usage)?.write_to_bytes()
                })
                .transpose()?;

            let init_c7 = if mode & RUN_SMC_METHOD_INIT_C7 != 0 {
                let mut smc_info = run.smc_info;
                if mode & RUN_SMC_METHOD_FULL_C7 == 0 {
                    smc_info.config_params = Default::default();
                }
                Some(serialize_vm_stack_value_boc(&smc_info.as_temp_data_item())?)
            } else {
                None
            };

            Ok(make_result(run.exit_code, result, state_proof, init_c7))
        })
        .await?
    }

    async fn get_account_state_coalesced(
        context: &SubscriberContext,
        block_id: BlockIdExt,
        account_id: AccountIdTl,
        prune: bool,
        is_slow: Arc<AtomicBool>,
    ) -> Result<AccountState> {
        // Wraps get_account_state through LRU cache + AwaitersPool so that:
        // 1. Sequential duplicate requests hit the LRU cache instantly.
        // 2. Parallel duplicate requests share a single computation via coalescing.

        // Resolve "latest" block_id to actual id for a stable cache key.
        let resolved_root_hash = if block_id.seq_no == SEQNO_ANY {
            get_last_liteserver_state_block(&context.engine)?.root_hash.clone()
        } else {
            block_id.root_hash.clone()
        };

        let key = AccountCacheKey {
            block_root_hash: resolved_root_hash,
            account_hash: account_id.id.clone(),
            workchain: account_id.workchain,
            prune,
        };

        // 1. Check LRU cache for a previously computed result.
        if let Some(state) = context.account_state_lru.lock().unwrap().cache.get(&key).cloned() {
            log::info!("liteserver: GetAccountState LRU hit for {}", key);
            // We may already have empty value in slot after previous slow query. So confirm it
            if state.proof.is_empty() && state.state.is_empty() && !is_slow.load(Ordering::Relaxed)
            {
                is_slow.store(true, Ordering::Relaxed);
            }
            return Ok(state);
        }

        // 2. Coalescing: first caller computes (runs closure), others wait.
        let computed = Arc::new(AtomicBool::new(false));
        let start = Instant::now();
        let result = context
            .account_state_awaiters
            .do_or_wait(&key, None, {
                let engine = context.engine.clone();
                let block_id = block_id.clone();
                let account_id = account_id.clone();
                let is_slow = is_slow.clone();
                let computed = computed.clone();
                async move {
                    computed.store(true, Ordering::Relaxed);
                    Self::get_account_state(&engine, block_id, account_id, prune, is_slow).await
                }
            })
            .await?;

        match result {
            Some(state) => {
                let is_empty = state.proof.is_empty() && state.state.is_empty();
                if computed.load(Ordering::Relaxed) && !is_empty {
                    // 3. Insert into LRU with byte tracking (only the computing task).
                    //    Skip empty results (big accounts that bailed out with is_slow).
                    let entry_size = account_state_byte_size(&state);
                    let mut lru = context.account_state_lru.lock().unwrap();
                    if lru.cache.push(key.clone(), state.clone()).is_none() {
                        lru.total_bytes += entry_size;
                    }
                    while lru.total_bytes > lru.max_bytes {
                        if let Some((_, evicted)) = lru.cache.pop_lru() {
                            lru.total_bytes -= account_state_byte_size(&evicted);
                        } else {
                            break;
                        }
                    }
                } else {
                    log::info!(
                        "liteserver: GetAccountState coalesced for {key} in {}ms",
                        start.elapsed().as_millis()
                    );
                }
                // Propagate is_slow: if coalesced result is default (empty),
                // it means the first caller bailed out due to big account.
                if state.proof.is_empty()
                    && state.state.is_empty()
                    && !is_slow.load(Ordering::Relaxed)
                {
                    is_slow.store(true, Ordering::Relaxed);
                }
                Ok(state)
            }
            None => Err(error!("account state coalescing: unexpected None")),
        }
    }

    async fn get_all_shards_info(
        engine: &Arc<dyn EngineOperations>,
        block_id: BlockIdExt,
    ) -> Result<AllShardsInfo> {
        let handle = engine
            .load_block_handle(&block_id)?
            .ok_or_else(|| error!("no handle for {}", block_id))?;

        let raw = engine.load_block_raw(&handle).await?;
        tokio::task::spawn_blocking(move || {
            let raw_root = read_single_root_boc(&raw)?;
            let proof = create_block_shards_proof(&raw_root)?;
            let block = Block::construct_from_cell(raw_root)?;

            let mc_extra = block
                .read_extra()?
                .read_custom()?
                .ok_or_else(|| error!("no mc_extra in masterchain block"))?;

            let shards_cell = mc_extra
                .shards()
                .serialize()
                .map_err(|e| error!("shards.serialize() failed: {e}"))?;

            let data = write_boc(&shards_cell)
                .map_err(|e| error!("write_boc(shards_cell) failed: {e}"))?;
            Ok(AllShardsInfo { id: block_id, proof, data })
        })
        .await?
    }

    async fn get_block(engine: &Arc<dyn EngineOperations>, id: BlockIdExt) -> Result<BlockData> {
        if id.root_hash().is_zero() || id.file_hash().is_zero() {
            let prefix = id.shard().account_id_prefix();
            let (full_id, data) = engine
                .lookup_block_by_seqno(&prefix, id.seq_no())
                .await?
                .ok_or_else(|| error!("block with mc seqno {} not found in index", id.seq_no()))?;
            return Ok(BlockData { id: full_id, data });
        }
        let handle = engine
            .load_block_handle(&id)?
            .ok_or_else(|| error!("Cannot load handle for block {}", id))?;

        let data = engine.load_block_raw(&handle).await?;
        let id = handle.id().clone();
        Ok(BlockData { id, data })
    }

    async fn get_block_header(
        engine: &Arc<dyn EngineOperations>,
        block_id: BlockIdExt,
        mode: i32,
    ) -> Result<BlockHeader> {
        if block_id.seq_no == 0 || block_id.root_hash.is_zero() || block_id.file_hash.is_zero() {
            fail!("invalid BlockIdExt for getBlockHeader: {}", block_id);
        }
        let handle = engine
            .load_block_handle(&block_id)?
            .ok_or_else(|| error!("no handle for {}", block_id))?;
        let bp = engine.load_block_proof(&handle, !block_id.is_masterchain()).await?;
        let header_proof =
            tokio::task::spawn_blocking(move || write_boc(bp.merkle_proof_root_cell())).await??;
        Ok(BlockHeader { id: block_id, mode, header_proof })
    }

    async fn get_block_out_msg_queue_size(
        engine: &Arc<dyn EngineOperations>,
        mode: i32,
        id: BlockIdExt,
        _want_proof: bool,
    ) -> Result<BlockOutMsgQueueSize> {
        let with_proof = (mode & 1) != 0;
        let state = engine.load_state(&id).await?;
        let state_root = state.root_cell().clone();

        let (size, state_path_cell_opt) =
            tokio::task::spawn_blocking(move || -> Result<(usize, Option<Cell>)> {
                if with_proof {
                    let usage = UsageTree::with_root(state_root.clone());
                    let virt_root = usage.root_cell();

                    let sstate: ShardStateUnsplit =
                        ShardStateUnsplit::construct_from_cell(virt_root)?;
                    let info = sstate
                        .read_out_msg_queue_info()
                        .map_err(|e| error!("cannot read out_msg_queue_info: {e}"))?;

                    let size = info.extra().out_queue_size();
                    let state_path_proof = MerkleProof::create_by_usage_tree(&state_root, &usage)?;
                    let state_path_cell = {
                        let mut b = BuilderData::new();
                        state_path_proof.write_to(&mut b)?;
                        b.into_cell()?
                    };
                    Ok((size, Some(state_path_cell)))
                } else {
                    let sstate: ShardStateUnsplit =
                        ShardStateUnsplit::construct_from_cell(state_root.clone())?;
                    let info = sstate.read_out_msg_queue_info()?;
                    let size = info.extra().out_queue_size();
                    Ok((size, None))
                }
            })
            .await??;

        let proof_bytes = if !with_proof {
            None
        } else {
            let block_stuff = load_block_by_id(engine, &id).await?;
            tokio::task::spawn_blocking(move || -> Result<Option<Vec<u8>>> {
                let block_root = block_stuff.root_cell();
                let usage_block = UsageTree::with_root(block_root.clone());
                let _su =
                    Block::construct_from_cell(usage_block.root_cell())?.read_state_update()?;

                let state_root_proof =
                    MerkleProof::create_by_usage_tree(&block_root, &usage_block)?;
                let state_root_cell = {
                    let mut b = BuilderData::new();
                    state_root_proof.write_to(&mut b)?;
                    b.into_cell()?
                };

                let state_path_cell =
                    state_path_cell_opt.ok_or_else(|| error!("state-path proof missing"))?;
                let bytes = BocWriter::with_roots(vec![state_root_cell, state_path_cell])?
                    .write_to_vec()?;
                Ok(Some(bytes))
            })
            .await??
        };
        Ok(BlockOutMsgQueueSize { mode, id, size: size as i64, proof: proof_bytes })
    }

    async fn get_block_proof(
        engine: &Arc<dyn EngineOperations>,
        mode_raw: i32,
        known_block: BlockIdExt,
        target_block_opt: Option<BlockIdExt>,
    ) -> Result<PartialBlockProof> {
        get_block_proof(engine, mode_raw, known_block, target_block_opt).await
    }

    async fn get_config_params(
        engine: &Arc<dyn EngineOperations>,
        mode: i32,
        id: BlockIdExt,
        mut param_list: Vec<i32>,
    ) -> Result<ConfigInfo> {
        if !id.is_masterchain() {
            fail!("id must be a full masterchain block");
        }
        if param_list.is_empty() && (mode & CFG_VISIT_ROOT) == 0 {
            fail!("empty param_list: pass ids or call GetConfigAll");
        }

        let from_key_block = (mode & CFG_FROM_PREV_KEY_BLOCK) != 0;
        let mut subtrees = HashSet::new();

        let (id, state_proof, cfg, usage) = if from_key_block {
            let key_block_id = Self::get_prev_key_block(engine, &id).await?;
            let block_handle = engine
                .load_block_handle(&key_block_id)?
                .ok_or_else(|| error!("no handle for {}", key_block_id))?;
            let block_data = engine.load_block_raw(&block_handle).await?;
            let block_root = read_single_root_boc(&block_data)?;
            let usage = UsageTree::with_params(block_root.clone(), true);
            let block = Block::construct_from_cell(usage.root_cell())?;
            let extra = block
                .read_extra()?
                .read_custom()?
                .ok_or_else(|| error!("no custom in key block {}", key_block_id))?;
            let cfg = extra
                .config()
                .cloned()
                .ok_or_else(|| error!("no config in key block {}", key_block_id))?;
            (key_block_id, Vec::new(), cfg, usage)
        } else {
            let state_proof = Self::state_proof_for_config(engine, &id).await?;
            let state = engine.load_state(&id).await?;
            let usage = UsageTree::with_params(state.root_cell().clone(), true);
            let ss: ShardStateUnsplit = ShardStateUnsplit::construct_from_cell(usage.root_cell())?;
            let custom = ss
                .read_custom()
                .map_err(|e| error!("read_custom({id}) failed: {e}"))?
                .ok_or_else(|| error!("No custom in masterchain state {id}"))?;
            if mode & CFG_NEED_PREV_BLOCKS != 0 {
                visit_prev_blocks_info(&custom, &ss)?;
                param_list.push(8);
            }
            if (mode & CFG_VISIT_ROOT) != 0 {
                let stats_hash = custom
                    .block_create_stats
                    .as_ref()
                    .map(|s| s.counters.root().map(|c| c.repr_hash()))
                    .flatten()
                    .unwrap_or_default();
                subtrees.insert(stats_hash);
            }
            (id, state_proof, custom.config, usage)
        };

        if (mode & CFG_VISIT_ROOT) != 0 {
            subtrees.insert(
                cfg.root().ok_or_else(|| error!("No config root in state {id}"))?.repr_hash(),
            );
        } else {
            param_list.sort_unstable();
            param_list.dedup();
            for &pid in &param_list {
                let key_bits: SliceData = pid.write_to_bitstring()?;
                if let Some(leaf) = cfg.config_params.get(key_bits)? {
                    subtrees.insert(leaf.cell()?.repr_hash());
                }
            }
        }

        let config_proof = tokio::task::spawn_blocking(move || {
            let proof = MerkleProof::create_with_subtrees(
                &usage.original_root(),
                |h| usage.contains(h),
                |h| subtrees.contains(h),
            )?;
            proof.write_to_bytes()
        })
        .await??;

        Ok(ConfigInfo { mode: (mode & CFG_MODE_MASK_RET), id, state_proof, config_proof })
    }

    async fn get_dispatch_queue_info(
        engine: &Arc<dyn EngineOperations>,
        mode: i32,
        id: BlockIdExt,
        after_addr: Option<UInt256>,
        max_accounts: i32,
        want_proof: bool,
    ) -> Result<DispatchQueueInfo> {
        let shard_state = engine.load_state(&id).await?;
        tokio::task::spawn_blocking(move || {
            let out_msg_queue_info = shard_state.state()?.read_out_msg_queue_info()?;
            let out_queue = out_msg_queue_info.out_queue();
            let mut acc: HashMap<UInt256, (i64, u64, u64)> = HashMap::new();
            let mut complete = true;

            out_queue.iterate_with_keys(|key, env| {
                if let Some(after) = &after_addr {
                    if key.hash <= *after {
                        return Ok(true);
                    }
                }

                if !acc.contains_key(&key.hash) && acc.len() == max_accounts as usize {
                    complete = false;
                    return Ok(false);
                }

                let lt = env.enqueued_lt();
                acc.entry(key.hash.clone())
                    .and_modify(|e| {
                        e.0 += 1; // size
                        e.1 = e.1.min(lt); // min_lt
                        e.2 = e.2.max(lt); // max_lt
                    })
                    .or_insert((1, lt, lt));

                Ok(true)
            })?;

            let mut account_dispatch_queues = Vec::with_capacity(acc.len());
            for (addr, (size, min_lt, max_lt)) in acc {
                account_dispatch_queues.push(AccountDispatchQueueInfo {
                    addr,
                    size,
                    min_lt: min_lt as i64,
                    max_lt: max_lt as i64,
                });
            }

            let proof = if want_proof {
                let queue_cell = out_queue.serialize()?;
                let mut bytes = Vec::new();
                BocWriter::with_root(&queue_cell)?.write(&mut bytes)?;
                Some(bytes)
            } else {
                None
            };

            Ok(DispatchQueueInfo {
                mode,
                id,
                account_dispatch_queues: account_dispatch_queues.into(),
                complete: complete.into(),
                proof,
            })
        })
        .await?
    }

    async fn get_dispatch_queue_messages(
        engine: &Arc<dyn EngineOperations>,
        mode: i32,
        id: BlockIdExt,
        addr: UInt256,
        after_lt: i64,
        max_messages: i32,
    ) -> Result<DispatchQueueMessages> {
        if id.root_hash.is_zero() || id.file_hash.is_zero() {
            fail!("invalid BlockIdExt");
        }
        if max_messages <= 0 {
            fail!("invalid max_messages");
        }

        let want_proof = (mode & WANT_PROOF_BIT) != 0;
        let one_account = (mode & ONE_ACCOUNT_BIT) != 0;
        let messages_boc_flag = (mode & MESSAGES_BOC_BIT) != 0;

        let after_lt = after_lt.max(0) as u64;
        let limit = if messages_boc_flag {
            max_messages.min(16) as usize
        } else {
            max_messages.min(64) as usize
        };

        // Get state_header_proof cell before spawn_blocking
        let state_header_proof_cell =
            if want_proof { Some(state_header_proof(engine, &id).await?) } else { None };

        let shard_state = engine.load_state(&id).await?;
        let state_root = shard_state.root_cell().clone();

        tokio::task::spawn_blocking(move || {
            let state_usage = UsageTree::with_params(state_root.clone(), true);
            let ss = ShardStateUnsplit::construct_from_cell(state_usage.root_cell())?;
            let out_msg_queue_info = ss.read_out_msg_queue_info()?;
            let dispatch_queue = out_msg_queue_info.extra().dispatch_queue();

            let mut messages: Vec<DispatchQueueMessage> = Vec::with_capacity(limit);
            let mut message_roots: Vec<Cell> = Vec::new();
            let mut complete = true;

            let orig_addr = addr.clone();
            let mut current_addr = addr;
            let current_lt = after_lt;
            let mut first_account = true;

            loop {
                if messages.len() >= limit {
                    complete = false;
                    break;
                }

                // Find account >= current_addr (eq=true for first, eq=false after)
                let search_key: AccountId = (&current_addr).into();
                let Some((mut account_id, account_queue)) =
                    dispatch_queue.find_leaf(&search_key, true, first_account, false)?
                else {
                    break;
                };

                let account_addr = account_id.get_next_hash()?;
                if one_account && account_addr != orig_addr {
                    complete = true;
                    break;
                }

                // Process messages in this account
                let account_messages = account_queue.messages();
                let start_lt =
                    if account_addr == orig_addr && first_account { current_lt } else { 0 };

                // Find messages with lt > start_lt
                let mut msg_lt = start_lt;
                loop {
                    if messages.len() >= limit {
                        complete = false;
                        break;
                    }

                    // Find next message with lt > msg_lt
                    let Some((found_lt, enqueued_msg)) =
                        account_messages.find_leaf(&msg_lt, true, false, false)?
                    else {
                        break;
                    };

                    let envelope = enqueued_msg.read_envelope_msg()?;
                    let msg_cell = envelope.message_cell();
                    let msg_hash = msg_cell.repr_hash();

                    // Build metadata with safe casts
                    let metadata = if let Some(meta) = envelope.metadata() {
                        let initiator_addr = meta.initiator_addr();
                        let initiator_id = match initiator_addr {
                            MsgAddressInt::AddrStd(std_addr) => {
                                UInt256::from_slice(&std_addr.address.get_bytestring(0))
                            }
                            MsgAddressInt::AddrVar(ref var_addr) => {
                                if var_addr.address.remaining_bits() != 256 {
                                    fail!("initiator address is not 256-bit");
                                }
                                UInt256::from_slice(&var_addr.address.get_bytestring(0))
                            }
                        };
                        let depth =
                            i32::try_from(meta.depth()).map_err(|_| error!("depth overflow"))?;
                        let initiator_lt = i64::try_from(meta.initiator_lt())
                            .map_err(|_| error!("initiator_lt overflow"))?;
                        TransactionMetadata {
                            mode: 0,
                            depth,
                            initiator: AccountIdTl {
                                workchain: initiator_addr.workchain_id(),
                                id: initiator_id,
                            },
                            initiator_lt,
                        }
                    } else {
                        TransactionMetadata {
                            mode: 0,
                            depth: -1,
                            initiator: AccountIdTl {
                                workchain: INVALID_WORKCHAIN_ID,
                                id: UInt256::default(),
                            },
                            initiator_lt: -1,
                        }
                    };

                    let lt = i64::try_from(found_lt).map_err(|_| error!("lt overflow"))?;

                    messages.push(DispatchQueueMessage {
                        addr: account_addr.clone(),
                        lt,
                        hash: msg_hash,
                        metadata,
                    });

                    if messages_boc_flag {
                        message_roots.push(msg_cell);
                    }

                    msg_lt = found_lt;
                }

                current_addr = account_addr;
                first_account = false;
            }

            let proof = if want_proof {
                let state_header_cell =
                    state_header_proof_cell.ok_or_else(|| error!("state_header_proof missing"))?;
                let usage_tree_cell =
                    MerkleProof::create_by_usage_tree(&state_root, &state_usage)?.serialize()?;
                Some(write_boc_multi(vec![state_header_cell, usage_tree_cell])?)
            } else {
                None
            };

            let messages_boc_bytes =
                if messages_boc_flag { Some(write_boc_multi(message_roots)?) } else { None };

            Ok(DispatchQueueMessages {
                mode,
                id,
                messages: messages.into(),
                complete: complete.into(),
                proof,
                messages_boc: messages_boc_bytes,
            })
        })
        .await?
    }

    async fn get_libraries_with_proof(
        engine: &Arc<dyn EngineOperations>,
        id: BlockIdExt,
        mode: i32,
        mut library_list: Vec<UInt256>,
    ) -> Result<LibraryResultWithProof> {
        if library_list.is_empty() {
            return Ok(LibraryResultWithProof {
                id,
                mode,
                result: Vec::new(),
                state_proof: Vec::new(),
                data_proof: Vec::new(),
            });
        }
        if library_list.len() > 16 {
            library_list.truncate(16);
        }
        let mut seen = HashSet::new();
        library_list.retain(|h| seen.insert(h.clone()));

        let state_stuff = engine.load_state(&id).await?;
        let (result, data_proof) =
            tokio::task::spawn_blocking(move || -> Result<(Vec<LibraryEntry>, Vec<u8>)> {
                let state_root = state_stuff.root_cell();
                let usage = UsageTree::with_root(state_root.clone());
                let ss_u = ShardStateUnsplit::construct_from_cell(usage.root_cell())?;
                let libs_u = ss_u.libraries();
                let include_data = (mode & LIB_MODE_SKIP_DATA) == 0;

                let mut result = Vec::new();
                for hash in library_list {
                    if let Ok(Some(lib_descr)) = libs_u.get(&hash) {
                        let cell = lib_descr.lib();
                        if cell.repr_hash() != hash {
                            continue;
                        }
                        let data = if include_data { write_boc(&cell)? } else { Vec::new() };
                        result.push(LibraryEntry { hash, data });
                    }
                }
                let proof =
                    MerkleProof::create_by_usage_tree(&state_root, &usage)?.write_to_bytes()?;
                Ok((result, proof))
            })
            .await??;

        let blk = load_block_by_id(engine, &id).await?;
        let state_proof = tokio::task::spawn_blocking(move || {
            header_proof(blk.root_cell(), HeaderProofKind::Full)?.write_to_bytes()
        })
        .await??;
        Ok(LibraryResultWithProof { id, mode, result, state_proof, data_proof })
    }

    async fn get_masterchain_info(engine: &Arc<dyn EngineOperations>) -> Result<MasterchainInfo> {
        let mc_block_id = get_last_liteserver_state_block(engine)?;
        let mc_state = engine.load_state(&mc_block_id).await?;
        let state_root_hash = mc_state.root_cell().repr_hash();

        let zerostate_block_id = engine.zerostate_id()?.clone();

        let init = ZeroStateIdExt {
            workchain: zerostate_block_id.shard().workchain_id(),
            root_hash: zerostate_block_id.root_hash,
            file_hash: zerostate_block_id.file_hash,
        };
        Ok(MasterchainInfo { last: (*mc_block_id).clone(), state_root_hash, init })
    }

    async fn get_masterchain_info_ext(
        engine: &Arc<dyn EngineOperations>,
        mode: i32,
    ) -> Result<MasterchainInfoExt> {
        let mc_block_id = get_last_liteserver_state_block(engine)?;

        let mc_state = engine.load_state(&mc_block_id).await?;
        let state_root_hash = mc_state.root_cell().repr_hash();
        let zerostate_block_id = engine.zerostate_id()?.clone();
        let init = ZeroStateIdExt {
            workchain: zerostate_block_id.shard().workchain_id(),
            root_hash: zerostate_block_id.root_hash,
            file_hash: zerostate_block_id.file_hash,
        };

        let last_utime = mc_state.state()?.gen_time() as i32;
        let now = engine.now() as i32;
        let gv = mc_state.shard_state_extra()?.config().get_global_version()?;

        Ok(MasterchainInfoExt {
            mode,
            version: gv.version as i32,
            capabilities: gv.capabilities as i64,
            last: (*mc_block_id).clone(),
            last_utime,
            now,
            state_root_hash,
            init,
        })
    }

    async fn get_one_transaction(
        engine: &Arc<dyn EngineOperations>,
        block_id: BlockIdExt,
        account_id: AccountIdTl,
        lt: i64,
    ) -> Result<TransactionInfo> {
        if lt < 0 {
            fail!("lt must be positive");
        }

        let id_bytes = account_id.id.as_slice();
        if id_bytes.len() < 8 {
            fail!(BlockError::InvalidArg("account id too short for prefix".into()));
        }
        let prefix64 = account_id.id.prefix64();
        if !block_id.shard().contains_prefix(account_id.workchain, prefix64) {
            fail!(
                "block {block_id} cannot contain account {}:{:x}",
                account_id.workchain,
                account_id.id,
            );
        }

        let handle = engine
            .load_block_handle(&block_id)?
            .ok_or_else(|| error!("no handle for {}", block_id))?;
        let raw = engine.load_block_raw(&handle).await?;
        tokio::task::spawn_blocking(move || {
            let root = read_single_root_boc(&raw)?;
            let usage = UsageTree::with_params(root.clone(), true);
            let blk = Block::construct_from_cell(usage.root_cell())?;

            let info = visit_block_info(&blk)?;
            let lt_u = lt as u64;
            if lt_u < info.start_lt() || lt_u > info.end_lt() {
                fail!(
                    "lt {lt} is outside block {block_id} range [{}..={}]",
                    info.start_lt(),
                    info.end_lt()
                );
            }

            let acc_key = SliceData::from_raw(account_id.id.as_slice().to_vec(), 256);
            let account_blocks = blk.read_extra()?.read_account_blocks()?;

            let acc_block = account_blocks
                .get(&acc_key)?
                .ok_or_else(|| error!("account not found in block {}", block_id))?;

            let slice = acc_block.transactions().get_as_slice(&lt_u)?.ok_or_else(|| {
                error!("transaction with lt {} not found in block {}", lt, block_id)
            })?;

            let tx_cell = slice.reference(0)?;
            let tx_boc = write_boc(&tx_cell)?;

            let proof_bytes = MerkleProof::create_by_usage_tree(&root, &usage)?.write_to_bytes()?;

            Ok(TransactionInfo { id: block_id, proof: proof_bytes, transaction: tx_boc })
        })
        .await?
    }

    async fn get_out_msg_queue_sizes(
        engine: &Arc<dyn EngineOperations>,
        mode: i32,
        wc: Option<i32>,
        shard: Option<i64>,
    ) -> Result<OutMsgQueueSizes> {
        let mc_block_id = get_last_liteserver_state_block(engine)?;
        let mc_state = engine.load_state(&mc_block_id).await?;
        let shard_hashes = mc_state.shard_state_extra()?.shards();

        let filter = if (mode & FILTER_BY_SHARD) != 0 {
            let wc =
                wc.ok_or_else(|| error!("wc is required for getOutMsgQueueSizes with mode bit0"))?;
            let shard = shard.ok_or_else(|| {
                error!("shard is required for getOutMsgQueueSizes with mode bit0")
            })?;
            Some(ShardIdent::with_tagged_prefix(wc, shard as u64)?)
        } else {
            None
        };

        let mut shard_ids = Vec::new();
        let mc_id = mc_state.block_id().clone();
        if filter.as_ref().map_or(true, |f| f.intersect_with(mc_id.shard())) {
            shard_ids.push(mc_id);
        }
        shard_hashes.iterate_shards(|ident, descr| {
            if filter.as_ref().map_or(true, |f| f.intersect_with(&ident)) {
                shard_ids.push(BlockIdExt::with_params(
                    ident,
                    descr.seq_no,
                    descr.root_hash,
                    descr.file_hash,
                ));
            }
            Ok(true)
        })?;

        let mut shards = Vec::with_capacity(shard_ids.len());
        for id in shard_ids {
            let state = engine.load_state(&id).await?;
            let info = state
                .state()?
                .read_out_msg_queue_info()
                .map_err(|e| error!("cannot read out_msg_queue_info: {e}"))?;

            // Use cached out_queue_size if available, otherwise count via len()
            let queue_size = info.extra().out_queue_size();
            let size_usize = if queue_size > 0 { queue_size } else { info.out_queue().len()? };
            let size = i32::try_from(size_usize)
                .map_err(|_| error!("out_msg_queue_size overflow: {size_usize}"))?;
            shards.push(OutMsgQueueSize { id, size });
        }

        let limit = SKIP_EXTERNALS_QUEUE_SIZE;

        Ok(OutMsgQueueSizes { shards: shards.into(), ext_msg_queue_size_limit: limit })
    }

    async fn get_libraries(
        engine: &Arc<dyn EngineOperations>,
        mut library_list: Vec<UInt256>,
    ) -> Result<LibraryResult> {
        if library_list.is_empty() {
            return Ok(LibraryResult { result: Vec::new() });
        }
        if library_list.len() > 16 {
            log::trace!(
                "get_libraries: too many libraries requested ({}), returning only first 16",
                library_list.len()
            );
            library_list.truncate(16);
        }
        let mut seen = HashSet::new();
        library_list.retain(|h| seen.insert(h.clone()));

        let mc_block_id = get_last_liteserver_state_block(engine)?;
        let mc_state = engine.load_state(&mc_block_id).await?;
        let state = mc_state.state()?;
        let libraries = state.libraries();

        let mut results = Vec::new();
        for lib_hash in library_list {
            if let Ok(Some(lib_cell)) = libraries.get(&lib_hash) {
                results.push(LibraryEntry { hash: lib_hash, data: write_boc(lib_cell.lib())? })
            }
        }
        Ok(LibraryResult { result: results })
    }

    async fn get_shard_block_proof(
        engine: &Arc<dyn EngineOperations>,
        id: BlockIdExt,
    ) -> Result<ShardBlockProof> {
        get_shard_block_proof(engine, id).await
    }

    async fn get_shard_info(
        engine: &Arc<dyn EngineOperations>,
        block_id: BlockIdExt,
        workchain: i32,
        shard: i64,
        exact: bool,
    ) -> Result<ShardInfo> {
        if !block_id.shard().is_masterchain() {
            fail!("Block {block_id} is not masterchain block");
        }
        let target_shard = ShardIdent::with_tagged_prefix(workchain, shard as u64)?;
        let wc = target_shard.workchain_id();
        if wc != 0 && wc != -1 {
            fail!("Invalid workchain {workchain}, only 0 and -1 are supported");
        }

        let mc_state = engine.load_state(&block_id).await?;

        tokio::task::spawn_blocking(move || {
            let state_root = mc_state.root_cell();

            let extra = mc_state
                .shard_state_extra()
                .map_err(|e| error!("shard_state_extra({block_id}) failed: {e}"))?;

            let shard_hashes = extra.shards();
            let found_record = if exact {
                shard_hashes.get_shard(&target_shard)?
            } else {
                shard_hashes.find_shard(&target_shard)?
            };

            let (found_shard, shard_descr, found) = match found_record {
                Some(record) => (record.shard().clone(), Some(record.descr().clone()), true),
                None => (target_shard.clone(), None, false),
            };

            let combined_proof = if let Some(ref descr) = shard_descr {
                make_shard_descr_proof(state_root, &found_shard, descr)?.write_to_bytes()?
            } else {
                Vec::new()
            };
            let (shardblk, shard_descr_bytes) = if found {
                let top_block = if let Some(ref descr) = shard_descr {
                    BlockIdExt {
                        shard_id: found_shard,
                        seq_no: descr.seq_no,
                        root_hash: descr.root_hash.clone(),
                        file_hash: descr.file_hash.clone(),
                    }
                } else {
                    fail!("No shard descriptor found");
                };
                let descr_bytes = if let Some(ref descr) = shard_descr {
                    descr.write_to_bytes()?
                } else {
                    vec![]
                };
                (top_block, descr_bytes)
            } else {
                (BlockIdExt::default(), vec![])
            };

            Ok(ShardInfo {
                id: block_id,
                shardblk,
                shard_proof: combined_proof,
                shard_descr: shard_descr_bytes,
            })
        })
        .await?
    }

    async fn get_state(
        engine: &Arc<dyn EngineOperations>,
        block_id: BlockIdExt,
    ) -> Result<BlockState> {
        if block_id.root_hash.is_zero() || block_id.file_hash.is_zero() {
            fail!("invalid BlockIdExt: {block_id}");
        }
        if !block_id.is_masterchain() {
            fail!("getState works only for masterchain blocks: {block_id}");
        }
        if block_id.seq_no > 1000 {
            fail!("cannot request total state: possibly too large");
        }

        let state = engine.load_state(&block_id).await?;
        let data = write_boc(state.root_cell())?;

        let result = BlockState {
            id: block_id,
            root_hash: state.root_cell().repr_hash(),
            file_hash: UInt256::calc_file_hash(&data),
            data,
        };
        Ok(result)
    }

    async fn get_transactions(
        engine: &Arc<dyn EngineOperations>,
        count_i32: i32,
        account_id: AccountIdTl,
        lt_i64: i64,
        mut hash: UInt256,
    ) -> Result<TransactionList> {
        if count_i32 <= 0 {
            fail!("count must be > 0");
        }
        let count = usize::try_from(count_i32)
            .map_err(|_| error!("count is too large for this platform"))?;
        let mut lt = u64::try_from(lt_i64).map_err(|_| error!("lt must be >= 0"))?;
        let workchain_id = account_id.workchain;
        let mut remaining = count.min(MAX_TRANSACTION_COUNT);

        let mut roots = Vec::new();
        let mut block_ids = Vec::new();

        // prefix for index by lt
        let prefix = AccountIdPrefixFull { workchain_id, prefix: account_id.id.prefix64() };
        let account_id = account_id.id.write_to_bitstring()?;

        'main: while remaining != 0 && lt != 0 {
            // abort_getTransactions: if you haven't found anything yet,
            // it's an error; otherwise, we finish with a partial result
            let Some((block_id, data)) = engine.lookup_block_by_lt(&prefix, lt).await? else {
                break;
            };
            // sanity: the block must cover our address
            if !block_id.shard().contains_full_prefix(&prefix) {
                fail!(
                    "obtained a block {block_id} that cannot contain \
                    specified account {account_id:x}"
                );
            }

            // deserialize strictly with the correct id — otherwise there will be a `wrong root hash`
            let id = block_id.clone();
            let block_stuff = tokio::task::spawn_blocking(move || {
                BlockStuff::deserialize_block(id, Arc::new(data))
            })
            .await??;

            let Some(acc_block) = block_stuff.get_account(&account_id)? else {
                fail!("block with id: {block_id} does not contain account: {account_id:x}")
            };
            let mut found_any_in_this_block = false;
            // search for a transaction with exactly the right lt
            while let Some(slice) = acc_block.transactions().get_as_slice(&lt)? {
                let cell = slice.reference(0)?;
                // check hash if exist
                if !hash.is_zero() && cell.repr_hash() != hash {
                    fail!(
                        "transaction hash mismatch: prev_trans_lt/hash invalid \
                        for wc={workchain_id}, lt={lt}"
                    );
                }

                // unpacking and monotony prev_trans_lt
                let tr = Transaction::construct_from_cell(cell.clone())?;
                if tr.prev_trans_lt() >= lt {
                    fail!("previous transaction time is not less than the current one");
                }
                found_any_in_this_block = true;

                // saving the found transaction
                roots.push(cell);
                block_ids.push(block_id.clone());
                remaining -= 1;
                if remaining == 0 {
                    break 'main;
                }

                // Step back up the chain
                lt = tr.prev_trans_lt();
                if lt == 0 {
                    break 'main;
                }
                hash = tr.prev_trans_hash().clone();
            }
            // exact-behaivor: block by lt, by tx not
            if !found_any_in_this_block {
                break;
            }
            // continue cycle: new lt already set prev_trans_lt
        }

        if roots.is_empty() {
            fail!("cannot compute block with specified transaction: no block by lt={lt}");
        }
        let transactions =
            tokio::task::spawn_blocking(move || BocWriter::with_roots(roots)?.write_to_vec())
                .await??;
        Ok(TransactionList { ids: block_ids, transactions })
    }

    async fn get_time(engine: &Arc<dyn EngineOperations>) -> Result<CurrentTime> {
        let now = engine.now() as i32;
        Ok(CurrentTime { now })
    }

    async fn get_validator_stats(
        engine: &Arc<dyn EngineOperations>,
        mode: i32,
        id: BlockIdExt,
        limit: i32,
        start_after: Option<UInt256>,
        _modified_after: Option<i32>,
    ) -> Result<ValidatorStats> {
        let state_handle = engine.load_state(&id).await?;
        tokio::task::spawn_blocking(move || {
            let mc_state = state_handle.shard_state_extra()?;
            let config = mc_state.config();
            let usage_tree = UsageTree::with_root(state_handle.root_cell().clone());

            let validator_set = config.validator_set()?;
            let mut count = 0;
            for validator in validator_set.list() {
                if count >= limit {
                    break;
                }

                let pubkey = UInt256::from_slice(validator.public_key.as_slice());
                if let Some(start) = &start_after {
                    if &pubkey <= start {
                        continue;
                    }
                }
                count += 1;
            }

            let (state_proof, data_proof) = if (mode & 1) != 0 {
                let state_proof_cell =
                    MerkleProof::create_by_usage_tree(&state_handle.root_cell(), &usage_tree)?
                        .serialize()?;
                let state_proof = write_boc(&state_proof_cell)?;
                (state_proof, Vec::new())
            } else {
                (Vec::new(), Vec::new())
            };

            Ok(ValidatorStats {
                mode,
                id,
                count,
                complete: (count < limit).into(),
                state_proof,
                data_proof,
            })
        })
        .await?
    }

    async fn get_version(engine: &Arc<dyn EngineOperations>) -> Result<Version> {
        let now = engine.now() as i32;
        Ok(Version { mode: 0, version: LS_VERSION, capabilities: LS_CAPABILITIES, now })
    }

    async fn list_block_transactions_internal(
        engine: &Arc<dyn EngineOperations>,
        id: BlockIdExt,
        mode: i32,
        count: i32,
        after: Option<TransactionId3>,
        ext: bool,
    ) -> Result<TLObject> {
        let handle = engine.load_block_handle(&id)?.ok_or_else(|| error!("no handle for {id}"))?;
        let raw = engine.load_block_raw(&handle).await?;

        tokio::task::spawn_blocking(move || {
            let root = read_single_root_boc(&raw)?;
            let usage = UsageTree::with_params(root.clone(), false);
            let block = Block::construct_from_cell(usage.root_cell())?;
            let _info = visit_block_info(&block)?;
            let extra = block.read_extra()?;
            let acc_blocks = extra.read_account_blocks()?;

            let reverse = mode & REVERSE_ORDER != 0;
            let forward = !reverse;
            let need = count as usize;
            let boundary_lt: u64 = if reverse { u64::MAX } else { 0 };

            // Determine starting point
            let (mut cur_addr, mut cur_lt): (AccountId, u64) = if mode & AFTER_PRESENT != 0 {
                let after_id =
                    after.ok_or_else(|| error!("AFTER_PRESENT flag is set but `after` is None"))?;
                (AccountId::from(&after_id.account), after_id.lt as u64)
            } else if reverse {
                (AccountId::from([0xFF; 32]), u64::MAX)
            } else {
                (AccountId::from([0x00; 32]), 0u64)
            };

            let mut result_txs: Vec<(UInt256, u64, Cell)> = Vec::new();
            let mut allow_same = true;
            let mut incomplete = true;

            while result_txs.len() < need {
                // Find nearest account block via directed traversal
                let (acc_id, acc_block) =
                    match acc_blocks.find_leaf(&cur_addr, forward, allow_same, false)? {
                        Some(pair) => pair,
                        None => {
                            incomplete = false;
                            break;
                        }
                    };

                allow_same = false;

                // If moved to a different account, reset transaction lt to boundary
                if acc_id != cur_addr {
                    cur_lt = boundary_lt;
                }
                cur_addr = acc_id;

                let acc_uint = cur_addr.clone().get_next_hash()?;

                // Traverse transactions in this account using directed lookup
                let transactions = acc_block.transactions();
                loop {
                    if result_txs.len() >= need {
                        break;
                    }

                    match find_nearest_tx_cell(transactions, cur_lt, forward)? {
                        Some((lt, tx_cell)) => {
                            result_txs.push((acc_uint.clone(), lt, tx_cell));
                            cur_lt = lt;
                        }
                        None => {
                            cur_lt = boundary_lt;
                            break;
                        }
                    }
                }
            }

            let proof_bytes = if mode & WANT_PROOF != 0 {
                MerkleProof::create_by_usage_tree(&root, &usage)?.write_to_bytes()?
            } else {
                Vec::new()
            };

            let ids: Vec<TransactionId> = result_txs
                .iter()
                .map(|(account, lt, cell)| {
                    let flags = TID_ACCOUNT | TID_LT | TID_HASH;
                    TransactionId {
                        mode: flags,
                        account: Some(account.clone()),
                        lt: Some(*lt as i64),
                        hash: Some(cell.repr_hash()),
                        metadata: None,
                    }
                })
                .collect();

            if ext {
                let tx_roots: Vec<Cell> = result_txs.iter().map(|(_, _, c)| c.clone()).collect();
                let tx_boc = write_boc_multi(tx_roots)?;
                Ok(BlockTransactionsExt {
                    id,
                    req_count: count,
                    incomplete: incomplete.into(),
                    transactions: tx_boc,
                    proof: proof_bytes.into(),
                }
                .into_boxed()
                .into_tl_object())
            } else {
                Ok(BlockTransactions {
                    id,
                    req_count: count,
                    incomplete: incomplete.into(),
                    ids,
                    proof: proof_bytes.into(),
                }
                .into_boxed()
                .into_tl_object())
            }
        })
        .await?
    }

    pub async fn list_block_transactions(
        engine: &Arc<dyn EngineOperations>,
        id: BlockIdExt,
        mode: i32,
        count: i32,
        after: Option<TransactionId3>,
    ) -> Result<BlockTransactions> {
        let mode = mode & (WANT_PROOF | REVERSE_ORDER | AFTER_PRESENT);
        let tl =
            Self::list_block_transactions_internal(engine, id, mode, count, after, false).await?;
        let enum_obj =
            tl.downcast::<BlockTxEnum>().map_err(|_| error!("unexpected TL-object type"))?;
        let payload = match enum_obj {
            BlockTxEnum::LiteServer_BlockTransactions(p) => p,
        };
        Ok(payload)
    }

    pub async fn list_block_transactions_ext(
        engine: &Arc<dyn EngineOperations>,
        id: BlockIdExt,
        mode: i32,
        count: i32,
        after: Option<TransactionId3>,
    ) -> Result<BlockTransactionsExt> {
        let mode = mode & (REVERSE_ORDER | AFTER_PRESENT | WANT_PROOF);
        let tl =
            Self::list_block_transactions_internal(engine, id, mode, count, after, true).await?;
        let enum_obj =
            tl.downcast::<BlockTxExtEnum>().map_err(|_| error!("unexpected TL-object type"))?;
        let payload = match enum_obj {
            BlockTxExtEnum::LiteServer_BlockTransactionsExt(p) => p,
        };
        Ok(payload)
    }

    async fn lookup_block(
        engine: &Arc<dyn EngineOperations>,
        mode: i32,
        id: BlockId,
        lt: Option<i64>,
        utime: Option<i32>,
    ) -> Result<BlockHeader> {
        let block_stuff = Self::lookup_block_stuff(engine, mode, id, lt, utime).await?;
        let root = block_stuff.root_cell().clone();
        let header_proof = tokio::task::spawn_blocking(move || {
            header_proof(&root, HeaderProofKind::Minimal)?.write_to_bytes()
        })
        .await??;
        Ok(BlockHeader { id: block_stuff.id().clone(), mode, header_proof })
    }

    async fn lookup_block_stuff(
        engine: &Arc<dyn EngineOperations>,
        mode: i32,
        id: BlockId,
        lt_opt: Option<i64>,
        utime_opt: Option<i32>,
    ) -> Result<BlockStuff> {
        let by_seq = (mode & 0x1) != 0;
        let by_lt = (mode & 0x2) != 0;
        let by_utime = (mode & 0x4) != 0;
        let bits = (by_seq as u8) + (by_lt as u8) + (by_utime as u8);
        if bits > 1 {
            fail!(BlockError::InvalidArg(
                "lookup_block: incompatible mode flags (seq/lt/utime)".into()
            ))
        }
        let shard_ident = ShardIdent::with_tagged_prefix(id.workchain, id.shard as u64)
            .map_err(|e| error!("invalid shard ident: {e}"))?;
        let prefix: AccountIdPrefixFull = shard_ident.account_id_prefix();
        // find by seqno
        if by_seq || (!by_lt && !by_utime) {
            if id.workchain == MASTERCHAIN_ID {
                if let Some(last) = engine.load_last_applied_mc_block_id()? {
                    let last = last.as_ref();
                    if id.seqno as u32 > last.seq_no {
                        let msg = make_notready_block_msg(&id, "seqno", id.seqno, last.seq_no);
                        fail!("{}", msg);
                    }
                }
            }

            if let Some((bid, data)) =
                engine.lookup_block_by_seqno(&prefix, id.seqno as u32).await?
            {
                return BlockStuff::deserialize_block(bid, Arc::new(data));
            }

            let fallback_id = BlockIdExt {
                shard_id: shard_ident,
                seq_no: id.seqno as u32,
                root_hash: UInt256::ZERO,
                file_hash: UInt256::ZERO,
            };
            return load_block_by_id(engine, &fallback_id).await;
        }

        // search by lt
        if by_lt {
            let lt = lt_opt.ok_or_else(|| error!("lookup_block(mode=lt): `lt` is required"))?;
            if lt < 0 {
                fail!(BlockError::InvalidArg("lt must be non-negative".into()));
            }
            let found = engine.lookup_block_by_lt(&prefix, lt as u64).await?;
            if let Some((bid, data)) = found {
                let res = BlockStuff::deserialize_block(bid, Arc::new(data));
                if let Err(ref err) = res {
                    let is_pkg_missing = err.to_string().contains("Package not found for seq_no");
                    if id.workchain == MASTERCHAIN_ID && is_pkg_missing {
                        let last = engine.load_last_applied_mc_block_id()?;
                        let last_seqno = last.map(|id| id.as_ref().seq_no).unwrap_or_default();
                        let msg = make_notready_block_msg(&id, "lt", lt, last_seqno);
                        fail!("{}", msg);
                    }
                }
                return res;
            }
            if id.workchain == MASTERCHAIN_ID {
                let last = engine.load_last_applied_mc_block_id()?;
                let last_seqno = last.map(|id| id.as_ref().seq_no).unwrap_or_default();
                let msg = make_notready_block_msg(&id, "lt", lt, last_seqno);
                fail!("{}", msg);
            }
            fail!(BlockError::NotFound(format!("no block found for lt={lt}")));
        }

        if by_utime {
            let utime =
                utime_opt.ok_or_else(|| error!("lookup_block(mode=utime): `utime` is required"))?;
            if utime < 0 {
                fail!(BlockError::InvalidArg("utime must be non-negative".into()));
            }

            let mut found: Option<(BlockIdExt, Vec<u8>)> = None;
            engine
                .lookup_blocks_by_utime(
                    &prefix,
                    utime as u32,
                    Box::new(|bid, data| {
                        found = Some((bid, data));
                        Ok(false)
                    }),
                )
                .await?;

            if let Some((bid, data)) = found {
                let res = BlockStuff::deserialize_block(bid, Arc::new(data));
                if let Err(ref err) = res {
                    let is_pkg_missing = err.to_string().contains("Package not found for seq_no");
                    if id.workchain == MASTERCHAIN_ID && is_pkg_missing {
                        let last = engine.load_last_applied_mc_block_id()?;
                        let last_seqno = last.map(|id| id.as_ref().seq_no).unwrap_or_default();
                        let msg = make_notready_block_msg(&id, "utime", utime, last_seqno);
                        fail!("{}", msg);
                    }
                }
                return res;
            }
            if id.workchain == MASTERCHAIN_ID {
                let last = engine.load_last_applied_mc_block_id()?;
                let last_seqno = last.map(|id| id.as_ref().seq_no).unwrap_or_default();
                let msg = make_notready_block_msg(&id, "utime", utime, last_seqno);
                fail!("{}", msg);
            }
            fail!(BlockError::NotFound(format!("no block found for utime={utime}")));
        }
        fail!(BlockError::InvalidArg("lookup_block: unknown mode".into()))
    }

    async fn lookup_block_with_proof(
        engine: &Arc<dyn EngineOperations>,
        mode_raw: LookupMode,
        id: BlockId,
        mc_block_id: BlockIdExt,
        lt_opt: Option<i64>,
        utime_opt: Option<i32>,
    ) -> Result<LookupBlockResult> {
        let mode = mode_raw & LOOKUP_BY_MASK;
        lkp_check(mode)?;

        if !mc_block_id.is_masterchain() || !mc_block_id.shard().is_full() {
            fail!("mc_block_id must be masterchain ID (optionally without hashes)");
        }
        let mc_prefix = AccountIdPrefixFull::any_masterchain();

        let base_mc_id = if mc_block_id.root_hash().is_zero() || mc_block_id.file_hash().is_zero() {
            let (bid, _) = engine
                .lookup_block_by_seqno(&mc_prefix, mc_block_id.seq_no())
                .await?
                .ok_or_else(|| error!("MC block seqno {} not found", mc_block_id.seq_no()))?;
            bid
        } else {
            mc_block_id.clone()
        };
        let mc_block_handle = engine
            .load_block_handle(&mc_block_id)?
            .ok_or_else(|| error!("Cannot load handle for mc_block_id {}", mc_block_id))?;

        let target_shard = ShardIdent::with_tagged_prefix(id.workchain, id.shard as u64)
            .map_err(|e| error!("invalid shard ident: {e}"))?;
        let prefix: AccountIdPrefixFull = target_shard.account_id_prefix();
        // Lookup target block
        let (target_id, target_root) = if lkp_has(mode, LOOKUP_BY_LT) {
            let lt = lt_opt.ok_or_else(|| error!("mode=LookupByLt, but `lt` missing"))?;
            let (bid, data) = engine
                .lookup_block_by_lt(&prefix, lt as u64)
                .await?
                .ok_or_else(|| error!("no block found by lt={lt}"))?;
            (bid, spawn_and_read_boc(data).await?)
        } else if lkp_has(mode, LOOKUP_BY_UTIME) {
            let ut = utime_opt.ok_or_else(|| error!("mode=LookupByUtime, but `utime` missing"))?;
            let mut found: Option<(BlockIdExt, Vec<u8>)> = None;
            engine
                .lookup_blocks_by_utime(
                    &prefix,
                    ut as u32,
                    Box::new(|bid, data| {
                        found = Some((bid, data));
                        Ok(false)
                    }),
                )
                .await?;
            let (bid, data) = found.ok_or_else(|| error!("no block found by utime={ut}"))?;
            (bid, spawn_and_read_boc(data).await?)
        } else {
            // LookupBySeqno
            if id.workchain == MASTERCHAIN_ID {
                let (bid, data) = engine
                    .lookup_block_by_seqno(&mc_prefix, id.seqno as u32)
                    .await?
                    .ok_or_else(|| error!("mc seqno {} not found", id.seqno))?;
                (bid, spawn_and_read_boc(data).await?)
            } else if let Some((bid, data)) =
                engine.lookup_block_by_seqno(&prefix, id.seqno as u32).await?
            {
                (bid, spawn_and_read_boc(data).await?)
            } else {
                let fallback = BlockIdExt {
                    shard_id: target_shard.clone(),
                    seq_no: id.seqno as u32,
                    root_hash: UInt256::ZERO,
                    file_hash: UInt256::ZERO,
                };
                let h = engine
                    .load_block_handle(&fallback)?
                    .ok_or_else(|| error!("no handle for {}", fallback))?;
                let raw = engine.load_block_raw(&h).await?;
                (h.id().clone(), spawn_and_read_boc(raw).await?)
            }
        };
        let blk_handle = engine
            .load_block_handle(&target_id)?
            .ok_or_else(|| error!("no handle for {}", target_id))?;
        let mc_ref_seqno = blk_handle.masterchain_ref_seq_no();
        if mc_block_id.seq_no() < mc_ref_seqno {
            fail!(
                "mc_block_id (trusted) {} older than target's mc_ref {}",
                mc_block_id.seq_no(),
                mc_ref_seqno
            );
        }

        let (mc_ref_bid, mc_ref_data) = engine
            .lookup_block_by_seqno(&mc_prefix, mc_ref_seqno)
            .await?
            .ok_or_else(|| error!("MC ref seqno {} not found", mc_ref_seqno))?;
        let mc_ref_root = spawn_and_read_boc(mc_ref_data).await?;

        // Build proofchain for shard block
        let mut shard_links: Vec<ShardBlockLink> = Vec::new();
        if id.workchain != MASTERCHAIN_ID {
            // maybe target_id
            let (top_id, first_link_proof) = proof_mc_to_shard_top(&mc_ref_root, &target_shard)?;
            // MC -> top_of_shard
            shard_links.push(ShardBlockLink { id: top_id.clone(), proof: first_link_proof });

            let mut cur_id = top_id.clone();
            // top -> ... -> target
            while cur_id != target_id {
                if shard_links.len() >= UNREGISTERED_CHAIN_MAX_LEN as usize {
                    fail!(
                        "proof chain is too long: walked {} steps (cap {}) from {} to {}",
                        shard_links.len(),
                        UNREGISTERED_CHAIN_MAX_LEN,
                        top_id.seq_no(),
                        target_id.seq_no()
                    );
                }
                let cur_h = engine
                    .load_block_handle(&cur_id)?
                    .ok_or_else(|| error!("no handle for {}", cur_id))?;
                let cur_raw = engine.load_block_raw(&cur_h).await?;
                let cur_root = spawn_and_read_boc(cur_raw).await?;
                let (prev_id, link_proof) = proof_shard_prev_link(&cur_root, &cur_id, &top_id)?;
                shard_links.push(ShardBlockLink { id: prev_id.clone(), proof: link_proof });
                cur_id = prev_id;
            }
        }

        // Build proof for base mc block.
        // (if mc block is looked, it is base block)
        let mut mc_block_proof = vec![];
        let mut client_mc_state_proof = vec![];
        if mc_ref_bid != base_mc_id {
            let base_mc_raw = engine.load_block_raw(&mc_block_handle).await?;
            let mc_state = engine.load_state(&mc_block_id).await?;

            (client_mc_state_proof, mc_block_proof) =
                tokio::task::spawn_blocking(move || -> Result<(Vec<u8>, Vec<u8>)> {
                    let base_mc_root = read_single_root_boc(&base_mc_raw)?;
                    Ok((
                        create_state_proof(&base_mc_root)?.write_to_bytes()?,
                        create_prevblocks_proof(mc_state.root_cell().clone(), mc_ref_bid.seq_no)?.0,
                    ))
                })
                .await??;
        }

        let want_prev = (mode & (LOOKUP_INCLUDE_PREV | LOOKUP_BY_LT | LOOKUP_BY_UTIME)) != 0;
        let mut prev_header = Vec::new();
        if want_prev {
            let p1 = engine.load_block_prev1(&target_id).ok();
            let p2 = engine.load_block_prev2(&target_id).ok().flatten();

            let choose = |cand: &BlockIdExt| -> bool {
                let cand_shard = cand.shard();
                cand_shard.intersect_with(&target_shard)
            };

            let chosen = match (p1.as_ref(), p2.as_ref()) {
                (Some(a), Some(b)) => match (choose(a), choose(b)) {
                    (true, false) => Some(a.clone()),
                    (false, true) => Some(b.clone()),
                    _ => Some(a.clone()),
                },
                (Some(a), None) => Some(a.clone()),
                (None, Some(b)) => Some(b.clone()),
                (None, None) => None,
            };
            if let Some(prev_id) = chosen {
                let prev_blk = load_block_by_id(engine, &prev_id).await?;
                prev_header = tokio::task::spawn_blocking(move || {
                    header_proof(prev_blk.root_cell(), HeaderProofKind::Minimal)?.write_to_bytes()
                })
                .await??;
            }
        }
        let ret_mc_id =
            if target_id.is_masterchain() { target_id.clone() } else { mc_ref_bid.clone() };

        let header = tokio::task::spawn_blocking(move || {
            header_proof(&target_root, HeaderProofKind::Minimal)?.write_to_bytes()
        })
        .await??;
        Ok(LookupBlockResult {
            id: target_id,
            mode: mode,
            mc_block_id: ret_mc_id,
            client_mc_state_proof,
            mc_block_proof,
            shard_links,
            header,
            prev_header,
        })
    }

    async fn send_message(
        engine: &Arc<dyn EngineOperations>,
        body: Vec<u8>,
    ) -> Result<SendMsgStatus> {
        match engine.redirect_external_message(&body).await {
            Ok(_) => Ok(SendMsgStatus { status: 1 }),
            Err(e) => {
                log::error!("SendMessage failed: {}", e);
                Ok(SendMsgStatus { status: -1 })
            }
        }
    }

    /// Returns the previous key block for the given block ID
    async fn get_prev_key_block(
        engine: &Arc<dyn EngineOperations>,
        id: &BlockIdExt,
    ) -> Result<BlockIdExt> {
        let last_block_id = get_last_liteserver_state_block(engine)?;
        if id.seq_no > last_block_id.seq_no {
            fail!(
                "Requested block seq_no {} is greater than last known block seq_no {}",
                id.seq_no,
                last_block_id.seq_no
            );
        }
        let state = engine.load_state(&last_block_id).await?;
        let extra = state.shard_state_extra()?;
        if id != last_block_id.as_ref() {
            extra.prev_blocks.check_block(id)?;
        } else if state.shard_state_extra()?.after_key_block {
            return Ok(last_block_id.as_ref().clone());
        }

        if let Some(prev_key) = extra.prev_blocks.get_prev_key_block(id.seq_no)? {
            return Ok(prev_key.master_block_id().1);
        } else {
            fail!("No previous key block found for seq_no {}", id.seq_no);
        }
    }

    async fn state_proof_for_config(
        engine: &Arc<dyn EngineOperations>,
        id: &BlockIdExt,
    ) -> Result<Vec<u8>> {
        if id.seq_no() == 0 {
            Ok(Vec::new())
        } else {
            let cell = state_header_proof(engine, id).await?;
            write_boc(&cell)
        }
    }

    fn get_prefix(data: &[u8], want: u32) -> bool {
        if data.len() >= 4 {
            if let Ok(data) = data[0..4].try_into() {
                let got = u32::from_le_bytes(data);
                return got == want;
            }
        }
        false
    }

    /// If `liteServer.query data:bytes` came on top, we take out `data` (internal payload).
    /// Returns None if it is not a wrapper.
    #[inline]
    fn peel_query_wrapper(data: &[u8]) -> Result<Option<Vec<u8>>> {
        if !Self::get_prefix(data, Query::constructor_const()) {
            return Ok(None);
        }
        let obj = deserialize_boxed(data)?;
        if let Ok(q) = obj.downcast::<Query>() {
            return Ok(Some(q.data));
        }
        fail!("liteServer.query: decode failed");
    }

    /// If the first TL ID is `liteServer.queryPrefix`, we discard 4 bytes of it.
    /// Returns a slice after the prefix or None if there is no prefix.
    #[inline]
    fn peel_query_prefix<'a>(data: &'a [u8]) -> Option<&'a [u8]> {
        if !Self::get_prefix(data, QueryPrefix::constructor_const()) {
            return None;
        }
        data.get(4..)
    }

    /// If it is followed by `liteServer.waitMasterchainSeqno{seqno, timeout_ms}`, we parse it
    /// and return (remainder after prefix, Some(seqno, timeout_ms)).
    /// If not— just (source_data, None).
    fn try_peel_wait_prefix<'a>(data: &'a [u8]) -> Result<(&'a [u8], Option<(u32, u32)>)> {
        if !Self::get_prefix(data, WaitMasterchainSeqno::constructor_const()) {
            return Ok((data, None));
        }
        // [id:4][seqno:int32][timeout_ms:int32]
        if data.len() < 12 {
            fail!("waitMasterchainSeqno: truncated payload");
        }
        let seqno = i32::from_le_bytes(data[4..8].try_into()?);
        if seqno < 0 {
            fail!("waitMasterchainSeqno: negative seqno={}", seqno);
        }
        let mut timeout_ms = i32::from_le_bytes(data[8..12].try_into()?);
        if timeout_ms < 0 {
            timeout_ms = 0;
        }
        let timeout_ms = (timeout_ms as u32).min(10_000);
        Ok((&data[12..], Some((seqno as u32, timeout_ms))))
    }

    async fn process(
        context: &SubscriberContext,
        query: TLObject,
        is_slow: bool,
    ) -> Result<(Option<Answer>, bool)> {
        let is_slow = Arc::new(AtomicBool::new(is_slow));
        let engine = &context.engine;
        let answer = route!(query,
            GetAccountState =>
                |q| Self::get_account_state_coalesced(
                    context,
                    q.id,
                    q.account,
                    false,
                    is_slow.clone()
                ),
            GetAccountStatePrunned =>
                |q| Self::get_account_state_coalesced(
                    context,
                    q.id,
                    q.account,
                    true,
                    is_slow.clone()
                ),
            GetAllShardsInfo =>
                |q| Self::get_all_shards_info(engine, q.id),
            GetBlock =>
                |q| Self::get_block(engine, q.id),
            GetBlockHeader =>
                |q| Self::get_block_header(engine, q.id, q.mode),
            GetBlockOutMsgQueueSize =>
                |q| Self::get_block_out_msg_queue_size(engine, q.mode, q.id, q.want_proof),
            GetBlockProof =>
                |q| Self::get_block_proof(engine, q.mode, q.known_block, q.target_block),
            GetConfigAll =>
                |q| Self::get_config_params(
                    engine,
                    (q.mode & CFG_MODE_MASK_RET) | CFG_VISIT_ROOT,
                    q.id,
                    Vec::new(),
                ),
            GetConfigParams =>
                |q| Self::get_config_params(
                    engine,
                    (q.mode & CFG_MODE_MASK_RET) | CFG_VISIT_PARAMS,
                    q.id,
                    q.param_list
                ),
            GetDispatchQueueInfo =>
                |q| Self::get_dispatch_queue_info(
                    engine,
                    q.mode,
                    q.id,
                    q.after_addr,
                    q.max_accounts,
                    q.want_proof
                ),
            GetDispatchQueueMessages =>
                |q| Self::get_dispatch_queue_messages(
                    engine,
                    q.mode,
                    q.id,
                    q.addr,
                    q.after_lt,
                    q.max_messages
                ),
            GetLibraries =>
                |q| Self::get_libraries(engine, q.library_list),
            GetLibrariesWithProof =>
                |q| Self::get_libraries_with_proof(engine, q.id, q.mode, q.library_list),
            GetMasterchainInfo =>
                |_q| Self::get_masterchain_info(engine),
            GetMasterchainInfoExt =>
                |q| Self::get_masterchain_info_ext(engine, q.mode & 0x7fff_ffff),
            GetOneTransaction =>
                |q| Self::get_one_transaction(engine, q.id, q.account, q.lt),
            GetOutMsgQueueSizes =>
                |q| Self::get_out_msg_queue_sizes(engine, q.mode, q.wc, q.shard),
            GetShardBlockProof =>
                |q| Self::get_shard_block_proof(engine, q.id),
            GetShardInfo =>
                |q| Self::get_shard_info(engine, q.id, q.workchain, q.shard, q.exact.into()),
            GetState =>
                |q| Self::get_state(engine, q.id),
            GetTime =>
                |_q| Self::get_time(engine),
            GetTransactions =>
                |q| Self::get_transactions(engine, q.count, q.account, q.lt, q.hash),
            GetValidatorStats =>
                |q| Self::get_validator_stats(
                    engine,
                    q.mode,
                    q.id,
                    q.limit,
                    q.start_after,
                    q.modified_after
                ),
            GetVersion =>
                |_q| Self::get_version(engine),
            ListBlockTransactions =>
                |q| Self::list_block_transactions(engine, q.id, q.mode, q.count, q.after),
            ListBlockTransactionsExt =>
                |q| Self::list_block_transactions_ext(engine, q.id, q.mode, q.count, q.after),
            LookupBlock =>
                |q| Self::lookup_block(engine, q.mode, q.id, q.lt, q.utime),
            LookupBlockWithProof =>
                |q| Self::lookup_block_with_proof(
                    engine,
                    q.mode,
                    q.id,
                    q.mc_block_id,
                    q.lt,
                    q.utime
                ),
            RunSmcMethod =>
                |q| Self::run_smc_method(engine, q.mode, q.id, q.account, q.method_id, q.params),
            SendMessage =>
                |q| Self::send_message(engine, q.body),
        );
        Ok((answer, is_slow.load(Ordering::Relaxed)))
    }

    fn maybe_spawn_watcher(&self) {
        if self
            .context
            .wait_registry
            .watcher_active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        self.runtime.spawn({
            let context = self.context.clone();
            async move {
                loop {
                    if Self::watcher_loop(&context).await.is_err() {
                        // Don't retry on engine errors — fail pending queries and exit.
                        // New incoming queries will spawn a fresh watcher.
                        Self::fail_all_pending(&context.wait_registry, "watcher error");
                        context.wait_registry.watcher_active.store(false, Ordering::Release);
                        break;
                    }
                    context.wait_registry.watcher_active.store(false, Ordering::Release);
                    // Race: a query may have been inserted after watcher_loop saw
                    // an empty registry but before we cleared the flag. Re-check
                    // and re-acquire if needed, otherwise that query would hang.
                    if context.wait_registry.pending.lock().unwrap().is_empty() {
                        break;
                    }
                    if context
                        .wait_registry
                        .watcher_active
                        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                        .is_err()
                    {
                        // Another watcher was already spawned — let it handle
                        break;
                    }
                }
            }
        });
    }

    fn dispatch_and_expire(context: &Arc<SubscriberContext>, current_seqno: u32) {
        let now = Instant::now();
        let satisfied;
        {
            let mut map = context.wait_registry.pending.lock().unwrap();

            // Split_off returns entries >= key, leaving < key in the original map.
            // We want entries with seqno <= current_seqno (satisfied).
            let remaining = map.split_off(&(current_seqno + 1));
            satisfied = std::mem::replace(&mut *map, remaining);

            // Expire timed-out queries from remaining entries
            for queries in map.values_mut() {
                let mut i = 0;
                while i < queries.len() {
                    if queries[i].deadline <= now {
                        queries.swap_remove(i).reject();
                    } else {
                        i += 1;
                    }
                }
            }
            map.retain(|_, v| !v.is_empty());
        }

        // Dispatch satisfied queries — spawn processing tasks
        for (_seqno, queries) in satisfied {
            for pq in queries {
                if pq.deadline <= now {
                    pq.reject();
                    continue;
                }
                #[cfg(feature = "telemetry")]
                let actual_start_at = Some(Instant::now());
                let context = context.clone();
                tokio::spawn(async move {
                    let result = Self::run_query_queued(
                        context.as_ref(),
                        pq.query,
                        pq.slow,
                        #[cfg(feature = "telemetry")]
                        actual_start_at,
                    )
                    .await;
                    pq.tx.send(result).ok();
                });
            }
        }
    }

    fn fail_all_pending(registry: &Arc<WaitRegistry>, error_msg: &str) {
        let mut map = registry.pending.lock().unwrap();
        let all = std::mem::take(&mut *map);
        drop(map);
        for (_seqno, queries) in all {
            for pq in queries {
                pq.tx.send(Err(error!("wait registry failed: {error_msg}"))).ok();
            }
        }
    }

    async fn run_query_queued(
        context: &SubscriberContext,
        query: TLObject,
        is_slow: bool,
        #[cfg(feature = "telemetry")] actual_start_at: Option<Instant>,
    ) -> Result<TimedAnswer<Answer>> {
        let permit = if is_slow { &context.semaphore_slow } else { &context.semaphore_fast }
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| error!("Semaphore closed: {e}"))?;
        let answer = Self::process(&context, query, true).await;
        drop(permit);
        let (answer, _) = answer?;
        Ok(TimedAnswer {
            answer,
            #[cfg(feature = "telemetry")]
            actual_start_at,
        })
    }

    async fn watcher_loop(context: &Arc<SubscriberContext>) -> Result<()> {
        let tip_id = get_last_liteserver_state_block(&context.engine)?;
        let mut handle = context
            .engine
            .load_block_handle(&tip_id)?
            .ok_or_else(|| error!("no handle for {}", tip_id))?;

        loop {
            Self::dispatch_and_expire(context, handle.id().seq_no);

            // Check if registry is empty — if so, watcher can exit
            let earliest_deadline = {
                let map = context.wait_registry.pending.lock().unwrap();
                if map.is_empty() {
                    return Ok(());
                }
                // Find earliest deadline across all pending queries
                map.values()
                    .flat_map(|queries| queries.iter().map(|q| q.deadline))
                    .min()
                    .unwrap_or_else(|| Instant::now() + Duration::from_secs(10))
            };

            let now = Instant::now();
            let timeout_ms = if earliest_deadline > now {
                (earliest_deadline - now).as_millis() as u64
            } else {
                // Already expired — run dispatch_and_expire again to clean up
                continue;
            };

            tokio::select! {
                result = context.engine.wait_next_applied_mc_block(&handle, Some(timeout_ms)) => {
                    // a new masterblock arrived
                    match result {
                        Ok((next_handle, _block)) => {
                            handle = next_handle;
                        }
                        Err(e) => {
                            let is_timeout = e
                                .downcast_ref::<NodeError>()
                                .map_or(false, |ne| matches!(ne, NodeError::Timeout(..)));
                            if is_timeout {
                                continue;
                            } else {
                                return Err(e);
                            }
                        }
                    }
                }
                _ = context.wait_registry.notify.notified() => {
                    // New query registered — loop back to recalculate deadline
                    continue;
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl Subscriber for LiteServerQuerySubscriber {
    async fn try_consume_query(&self, object: TLObject, _peers: &AdnlPeers) -> Result<QueryResult> {
        let query = match object.downcast::<Query>() {
            Ok(query) => {
                log::trace!("received query: {query:?}");
                query
            }
            Err(object) => {
                log::warn!("rejected TL object {object:?} (downcast<Query> failed)");
                return Ok(QueryResult::Rejected(object));
            }
        };

        // liteServer.query data:bytes = Object;
        let data: Cow<'_, [u8]> = if let Some(data) = Self::peel_query_wrapper(&query.data)? {
            Cow::Owned(data)
        // liteServer.queryPrefix = Object;
        } else if let Some(data) = Self::peel_query_prefix(&query.data) {
            Cow::Borrowed(data)
        } else {
            Cow::Borrowed(&query.data)
        };
        // liteServer.waitMasterchainSeqno seqno:int timeout_ms:int = Object;
        // Early seqno check: if the target seqno is already reached
        let (data, maybe_wait) = Self::try_peel_wait_prefix(&data)?;
        let maybe_wait = if let Some((want_seqno, timeout_ms)) = maybe_wait {
            if get_last_liteserver_state_block(&self.context.engine)?.seq_no >= want_seqno {
                None
            } else {
                Some((want_seqno, timeout_ms))
            }
        } else {
            None
        };
        let query = deserialize_boxed(data)?;

        // Three query categories:
        // 1. immediate — lightweight, processed inline without semaphore
        // 2. maybe_immediate — GetAccountState: try inline, falls back to slow
        //    if account is large (is_slow flag from process())
        // 3. slow — goes through semaphore_slow; everything else uses semaphore_fast
        let immediate = maybe_wait.is_none()
            && (query.is::<GetBlock>()
                || query.is::<GetLibraries>()
                || query.is::<GetMasterchainInfo>()
                || query.is::<GetMasterchainInfoExt>()
                || query.is::<GetTime>()
                || query.is::<GetVersion>()
                || query.is::<LookupBlock>()
                || query.is::<SendMessage>());
        if immediate {
            let (result, _) = Self::process(self.context.as_ref(), query, false).await?;
            return Ok(QueryResult::Consumed(QueryAnswer::Ready(result)));
        }

        let maybe_immediate = query.is::<GetAccountState>() || query.is::<GetAccountStatePrunned>();
        if maybe_immediate && maybe_wait.is_none() {
            let (result, is_slow) =
                Self::process(self.context.as_ref(), query.clone(), false).await?;
            if !is_slow {
                return Ok(QueryResult::Consumed(QueryAnswer::Ready(result)));
            }
        }

        // maybe_immediate that fell through (big account) is also slow
        let slow = maybe_immediate
            || query.is::<GetBlockProof>()
            || query.is::<GetShardBlockProof>()
            || query.is::<ListBlockTransactions>()
            || query.is::<ListBlockTransactionsExt>()
            || query.is::<LookupBlockWithProof>()
            || query.is::<RunSmcMethod>();

        // Wait-prefixed queries go through the shared wait registry
        if let Some((want_seqno, timeout_ms)) = maybe_wait {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let deadline = Instant::now() + Duration::from_millis(timeout_ms.min(10_000) as u64);
            let pending = PendingWaitQuery { query, slow, deadline, tx };
            {
                let mut map = self.context.wait_registry.pending.lock().unwrap();
                map.entry(want_seqno).or_default().push(pending);
            }
            self.context.wait_registry.notify.notify_one();
            self.maybe_spawn_watcher();
            let handle = self.runtime.spawn(async move {
                rx.await.map_err(|_| anyhow::anyhow!("wait registry dropped"))?
            });
            return Ok(QueryResult::Consumed(QueryAnswer::Pending(handle)));
        }

        // Non-immediate queries without wait prefix — spawn with semaphore
        let handle = self.runtime.spawn({
            let context = self.context.clone();
            async move {
                Self::run_query_queued(
                    context.as_ref(),
                    query,
                    true,
                    #[cfg(feature = "telemetry")]
                    None,
                )
                .await
            }
        });
        Ok(QueryResult::Consumed(QueryAnswer::Pending(handle)))
    }
}

// VmStack BOC serialization helpers (C++ compatible format)
// Format: vm_stack#_ depth:(## 24) stack:(VmStackList depth) = VmStack;
//  vm_stk_nil#_ = VmStackList 0;
//  vm_stk_cons#_ {n:#} rest:^(VmStackList n) tos:VmStackValue = VmStackList (n + 1);

fn deserialize_vm_stack_boc(boc: &[u8]) -> Result<Vec<StackItem>> {
    if boc.is_empty() {
        return Ok(Vec::new());
    }
    let root = read_single_root_boc(boc)?;
    let mut cs = SliceData::load_cell(root)?;
    let depth = cs.get_next_int(24)? as usize;
    if depth == 0 {
        return Ok(Vec::new());
    }
    if depth > 1024 {
        fail!("VmStack depth {} exceeds limit 1024", depth);
    }
    let rest = cs.checked_drain_reference()?;
    let top = read_vm_stack_value(&mut cs)?;
    let mut items = vec![top];

    let mut rest_cell = rest;
    for _ in 0..depth - 1 {
        let mut rest_cs = SliceData::load_cell(rest_cell)?;
        let next_rest = rest_cs.checked_drain_reference()?;
        let item = read_vm_stack_value(&mut rest_cs)?;
        items.push(item);
        rest_cell = next_rest;
    }
    items.reverse();
    Ok(items)
}

fn serialize_vm_stack(items: &[StackItem]) -> Result<Cell> {
    let n = items.len();
    let mut cb = BuilderData::new();
    cb.append_bits(n, 24)?;
    if n == 0 {
        return cb.into_cell();
    }
    // vm_stk_nil = empty cell
    let mut rest = BuilderData::new().into_cell()?;
    for item in items.iter().take(n - 1) {
        let mut cons = BuilderData::new();
        cons.checked_append_reference(rest)?;
        write_vm_stack_value(item, &mut cons)?;
        rest = cons.into_cell()?;
    }
    cb.checked_append_reference(rest)?;
    write_vm_stack_value(&items[n - 1], &mut cb)?;
    cb.into_cell()
}

fn serialize_vm_stack_value_boc(item: &StackItem) -> Result<Vec<u8>> {
    let mut cb = BuilderData::new();
    write_vm_stack_value(item, &mut cb)?;
    write_boc(&cb.into_cell()?)
}

fn read_vm_stack_value(cs: &mut SliceData) -> Result<StackItem> {
    let tag = cs.get_next_byte()?;
    match tag {
        0x00 => Ok(StackItem::None),
        0x01 => {
            let val = cs.get_next_i64()?;
            Ok(StackItem::int(val))
        }
        0x02 => {
            let next = cs.get_next_byte()?;
            match next {
                0xFF => Ok(StackItem::integer(ton_vm::stack::integer::IntegerData::nan())),
                0x00 => {
                    let bytes = cs.get_next_u256()?;
                    Ok(StackItem::integer(ton_vm::stack::integer::IntegerData::from_bytes(
                        bytes, 256, false, true,
                    )?))
                }
                0x01 => {
                    let bytes_256 = cs.get_next_u256()?;
                    let mut bytes = vec![0xff];
                    bytes.extend_from_slice(&bytes_256);
                    Ok(StackItem::integer(ton_vm::stack::integer::IntegerData::from_bytes(
                        &bytes, 264, true, true,
                    )?))
                }
                _ => fail!("VmStackValue: invalid big int subtag 0x{:02x}", next),
            }
        }
        0x03 => {
            let cell = cs.checked_drain_reference()?;
            Ok(StackItem::cell(cell))
        }
        0x04 => {
            let cell = cs.checked_drain_reference()?;
            let st_bits = cs.get_next_int(10)? as usize;
            let end_bits = cs.get_next_int(10)? as usize;
            let st_ref = cs.get_next_int(3)? as usize;
            let end_ref = cs.get_next_int(3)? as usize;
            if st_bits > end_bits || st_ref > end_ref {
                fail!("VmStackValue: invalid slice window");
            }
            let slice = SliceData::load_cell_with_window(cell, st_bits..end_bits, st_ref..end_ref)?;
            Ok(StackItem::slice(slice))
        }
        0x05 => {
            let cell = cs.checked_drain_reference()?;
            let cell_slice = SliceData::load_cell(cell)?;
            let mut builder = BuilderData::new();
            builder.checked_append_references_and_data(&cell_slice)?;
            Ok(StackItem::builder(builder))
        }
        0x07 => {
            let n = cs.get_next_u16()? as usize;
            read_vm_tuple(cs, n)
        }
        _ => fail!("VmStackValue: unknown tag 0x{:02x}", tag),
    }
}

fn read_vm_tuple(cs: &mut SliceData, n: usize) -> Result<StackItem> {
    if n == 0 {
        return Ok(StackItem::tuple(Vec::new()));
    }
    if n == 1 {
        let cell = cs.checked_drain_reference()?;
        let mut item_cs = SliceData::load_cell(cell)?;
        let item = read_vm_stack_value(&mut item_cs)?;
        return Ok(StackItem::tuple(vec![item]));
    }
    let mut head = cs.checked_drain_reference()?;
    let tail = cs.checked_drain_reference()?;
    let mut tail_cs = SliceData::load_cell(tail)?;
    let last = read_vm_stack_value(&mut tail_cs)?;

    let mut remaining = n - 1;
    let mut items = Vec::with_capacity(n);
    while remaining > 1 {
        let mut head_cs = SliceData::load_cell(head)?;
        let next_head = head_cs.checked_drain_reference()?;
        let item_cell = head_cs.checked_drain_reference()?;
        let mut item_cs = SliceData::load_cell(item_cell)?;
        items.push(read_vm_stack_value(&mut item_cs)?);
        head = next_head;
        remaining -= 1;
    }
    let mut head_cs = SliceData::load_cell(head)?;
    let first = read_vm_stack_value(&mut head_cs)?;
    items.push(first);
    items.reverse();
    items.push(last);
    Ok(StackItem::tuple(items))
}

fn write_vm_stack_value(item: &StackItem, cb: &mut BuilderData) -> Result<()> {
    match item {
        StackItem::None => {
            cb.append_bits(0x00, 8)?;
        }
        StackItem::Integer(value) => {
            if value.is_nan() {
                cb.append_bits(0x02FF, 16)?;
            } else if value.fits_in(64)? {
                cb.append_bits(0x01, 8)?;
                let v: i64 = value.as_integer_value(i64::MIN..=i64::MAX)?;
                cb.append_i64(v)?;
            } else {
                cb.append_bits(0x0100, 15)?;
                let int_builder = value.as_builder(257, true, true)?;
                cb.append_builder(&int_builder)?;
            }
        }
        StackItem::Cell(cell) => {
            cb.append_bits(0x03, 8)?;
            cb.checked_append_reference(cell.clone())?;
        }
        StackItem::Slice(slice) => {
            cb.append_bits(0x04, 8)?;
            cb.checked_append_reference(slice.cell()?)?;
            let refs = slice.get_references();
            cb.append_bits(slice.pos(), 10)?;
            cb.append_bits(slice.pos() + slice.remaining_bits(), 10)?;
            cb.append_bits(refs.start, 3)?;
            cb.append_bits(refs.end, 3)?;
        }
        StackItem::Builder(bd) => {
            cb.append_bits(0x05, 8)?;
            cb.checked_append_reference(bd.as_ref().clone().into_cell()?)?;
        }
        StackItem::Tuple(items) => {
            write_vm_tuple(items, cb)?;
        }
        StackItem::Continuation(_) => {
            fail!("Cannot serialize continuation in liteserver response");
        }
    }
    Ok(())
}

fn write_vm_tuple(items: &[StackItem], cb: &mut BuilderData) -> Result<()> {
    let n = items.len();
    cb.append_bits(0x07, 8)?;
    cb.append_u16(n as u16)?;
    if n == 0 {
        return Ok(());
    }
    if n == 1 {
        let cell = serialize_single_vm_value(&items[0])?;
        cb.checked_append_reference(cell)?;
        return Ok(());
    }
    let mut head: Option<Cell> = None;
    let mut tail: Option<Cell> = None;
    for (i, item) in items.iter().enumerate() {
        std::mem::swap(&mut head, &mut tail);
        if i > 1 {
            let mut chain = BuilderData::new();
            chain.checked_append_reference(tail.take().unwrap())?;
            chain.checked_append_reference(head.take().unwrap())?;
            head = Some(chain.into_cell()?);
        }
        tail = Some(serialize_single_vm_value(item)?);
    }
    if let Some(h) = head {
        cb.checked_append_reference(h)?;
    }
    if let Some(t) = tail {
        cb.checked_append_reference(t)?;
    }
    Ok(())
}

fn serialize_single_vm_value(item: &StackItem) -> Result<Cell> {
    let mut cb = BuilderData::new();
    write_vm_stack_value(item, &mut cb)?;
    cb.into_cell()
}

#[cfg(test)]
#[path = "../tests/test_liteserver.rs"]
mod tests;
