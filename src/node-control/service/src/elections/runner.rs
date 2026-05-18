/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::{
    adaptive_strategy,
    election_emulator::ParticipantStake,
    providers::{ElectionsProvider, ValidatorConfig, ValidatorEntry},
};
use anyhow::Context as _;
use common::{
    app_config::{BindingStatus, ElectionsConfig, NodeBinding, StakePolicy},
    snapshot::{
        ElectionsParticipantSnapshot, ElectionsSnapshot, ElectionsStatus, OurElectionParticipant,
        ParticipationStatus, SnapshotStore, StakeSubmission, TimeRange, ValidatorNodeSnapshot,
        ValidatorsSnapshot,
    },
    task_cancellation::CancellationCtx,
    time_format,
    ton_utils::{
        MAX_STAKE_FACTOR_SCALE, display_tons, max_stake_factor_raw_to_multiplier,
        nanotons_to_dec_string, nanotons_to_tons_f64,
    },
};
use contracts::{
    ElectionsInfo, ElectorWrapper, NominatorWrapper, Participant, PoolKind, TonWallet,
    elector::PastElections, nominator,
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};
use ton_block::{
    Cell, ConfigParam15, MsgAddressInt, UnixTime, ValidatorDescr, ValidatorSet,
    config_params::{ConfigParam16, ConfigParam17},
    write_boc,
};

#[cfg(test)]
#[path = "runner_tests.rs"]
mod runner_tests;

const EXPIRED_LAG: u64 = 300; // 5 minutes
/// Value in nanotons required by elector to execute stake or recover operations.
const ELECTOR_STAKE_FEE: u64 = 1_000_000_000;
/// Value in nanotons to send from the wallet to elector to recover stake.
const RECOVER_FEE: u64 = 200_000_000;
/// Gas fee consumed by nominator pool
const NPOOL_COMPUTE_FEE: u64 = 200_000_000;
/// Gas fee consumed by validator wallet
const WALLET_COMPUTE_FEE: u64 = 200_000_000;
/// Storage reserve kept in the validator wallet when staking directly (no nominator pool).
const WALLET_STORAGE_RESERVE: u64 = 1_000_000_000;
/// Extra storage fees reservation to correctly calculate free pool balance:
/// it's a storage fees accumulated between two stake operations.
/// It's an approximation to avoid error when staking all available funds.
/// The line from single-nominator contract:
/// ```ignore
/// throw_unless(ERROR::INSUFFICIENT_BALANCE, stake_amount <= my_balance - msg_value - MIN_TON_FOR_STORAGE);
/// ```
/// where `my_balance` is already decreased by storage fees which we want to cover.
const EXTRA_STORAGE_FEES: u64 = 5_000_000;
/// Gas attached to `process_withdraw_requests` (TONCore op = 2). Mirrors the masterchain pool-op
/// budget used by `update_validator_set` (see `contracts_task::POOL_OP_GAS`); 0.1 TON is not
/// enough for the pool's `load_data` + dict iteration + payouts + `save_data` cycle.
const WITHDRAW_PROCESS_GAS: u64 = 500_000_000; // 0.5 TON
/// Maximum number of withdraw requests processed per `process_withdraw_requests` call.
/// `pool.fc` already caps work by the pool's `max_nominators` (≤40); 100 is a safety ceiling
/// that fits in a single transaction without ever throttling normal operation.
const WITHDRAW_PROCESS_LIMIT: u8 = 100;

type OnStatusChange = Arc<dyn Fn(HashMap<String, BindingStatus>) + Send + Sync>;

/// Persists a batch of freshly generated static ADNL addresses (node_id → base64) into the
/// runtime config under `elections.static_adnls`. Called at most once per tick.
pub(crate) type PersistStaticAdnls =
    Arc<dyn Fn(HashMap<String, String>) -> anyhow::Result<()> + Send + Sync>;

/// Record of a single stake submission (internal).
#[derive(Clone, Debug)]
struct StakeSubmissionRecord {
    stake: u64,
    max_factor: u32,
    submission_time: u64,
}

/// Maximum number of stake submissions to keep per election cycle.
const MAX_STAKE_SUBMISSIONS: usize = 10;

struct Node {
    api: Box<dyn ElectionsProvider>,
    /// Current validator key id.
    /// Changed every time when new elections started.
    key_id: Vec<u8>,
    /// Current participant info.
    /// Set when new election bid is generated. Reset after new elections started.
    participant: Option<Participant>,
    /// Last successful stake submission timestamp.
    submission_time: Option<u64>,
    /// True if stake was accepted by the elector. Reset after new elections started.
    stake_accepted: bool,
    /// Stake amount accepted by the elector (nanotons).
    accepted_stake_amount: Option<u64>,
    /// History of stake submissions for current election cycle (capped to MAX_STAKE_SUBMISSIONS).
    stake_submissions: Vec<StakeSubmissionRecord>,
    /// True if node is in current validator set (p34).
    /// Computed in build_validators_snapshot, used by build_our_participants_snapshot.
    is_validator: bool,
    /// True if node is elected in next validator set (p36).
    /// Computed in build_validators_snapshot, used by build_our_participants_snapshot.
    is_next_validator: bool,
    wallet: Arc<dyn TonWallet>,
    /// Nominator pool for this node (`TonCoreNominatorRouter` when TONCore nominator, two pools). `None` = direct staking.
    pool: Option<Arc<dyn NominatorWrapper>>,
    /// Cached address of the pool contract (`pool.address()`), refreshed when election ID changes.
    /// `None` when there is no pool (direct staking).
    pool_addr_cache: Option<MsgAddressInt>,
    /// Last error observed for this node during the current/previous tick (stringified).
    last_error: Option<String>,
    /// Last `has_withdraw_requests` probe result this tick (`true` = pool still reports a non-empty
    /// `withdraw_requests` queue). Cleared at the start of each tick; set in
    /// [`ElectionRunner::process_pending_withdraw_requests`]. Drives `ProcessingWithdrawRequests` in
    /// snapshots — matches on-chain state without a separate "already sent op = 2" flag.
    withdraw_requests_pending: bool,
    /// Pre-generated static ADNL address (32-byte key hash).
    /// When set, this address is re-registered each election instead of generating a fresh one.
    static_adnl_addr: Option<Vec<u8>>,
    /// Opt-out: when true, the runner generates a fresh ephemeral ADNL address every cycle
    /// for this node instead of using a static one.
    static_adnl_disabled: bool,
    /// Excluded from elections (enable = false).
    excluded: bool,
    /// Effective stake policy for this node.
    stake_policy: StakePolicy,
    /// Last validator config.
    validator_config: ValidatorConfig,
    /// Current binding lifecycle status, computed each tick.
    binding_status: BindingStatus,
    /// Amount to recover from elector, computed each tick.
    last_recover_amount: u64,
}

impl Node {
    /// Resolved pool target for this node.
    /// - `Ok(None)` — direct staking (no pool configured).
    /// - `Ok(Some(addr))` — staking via pool at `addr`.
    /// - `Err` — pool is configured but its address is not cached yet. This is transient:
    ///   the next tick's resolve loop will retry. Callers should propagate the error so
    ///   the node is not silently downgraded to wallet-based staking.
    fn pool_target(&self) -> anyhow::Result<Option<&MsgAddressInt>> {
        match (&self.pool, &self.pool_addr_cache) {
            (None, _) => Ok(None),
            (Some(_), Some(addr)) => Ok(Some(addr)),
            (Some(_), None) => {
                Err(anyhow::anyhow!("pool address not resolved; will retry next tick"))
            }
        }
    }

    /// Get the address from which the stake will be sent to elector: pool or wallet.
    /// Errors if pool is configured but its address has not been resolved yet — preventing
    /// a misroute to wallet-based staking when the pool address cache is transiently empty.
    /// Note: only raw address bytes are returned (without workchain ID).
    async fn stake_addr(&self) -> anyhow::Result<Vec<u8>> {
        Ok(match self.pool_target()? {
            Some(addr) => addr.address().storage().to_vec(),
            None => self.wallet.address().await?.address().storage().to_vec(),
        })
    }

    /// Resolve the ADNL address for this election cycle.
    /// Precedence:
    /// 1. Opt-out (`static_adnl_disabled = true`) → fresh ephemeral ADNL.
    /// 2. Static ADNL is set (either pre-configured or auto-generated by the
    ///    runner's ensure-step earlier this tick) → re-register it.
    /// 3. Neither — the ensure-step failed to generate one this tick. Abort:
    ///    the next tick will retry generation.
    async fn resolve_adnl_addr(
        &mut self,
        perm_key_id: Vec<u8>,
        until: u64,
    ) -> anyhow::Result<Vec<u8>> {
        if self.static_adnl_disabled {
            return self.api.new_adnl_addr(perm_key_id, until).await;
        }
        match &self.static_adnl_addr {
            Some(key_id) => {
                self.api.register_adnl_addr(key_id.clone(), perm_key_id, until).await?;
                Ok(key_id.clone())
            }
            None => anyhow::bail!("static ADNL address not yet generated; will retry next tick"),
        }
    }

    fn reset_participation(&mut self) {
        self.participant = None;
        self.submission_time = None;
        self.stake_accepted = false;
        self.accepted_stake_amount = None;
        self.stake_submissions.clear();
        self.withdraw_requests_pending = false;
    }

    /// Minimum balance left on the staking target (pool contract or wallet) when computing
    /// spendable stake liquidity.
    async fn stake_balance(&mut self, gas_fee: u64) -> anyhow::Result<u64> {
        if let Some(pool) = self.pool.as_ref() {
            let addr = self
                .pool_addr_cache
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("pool address not found"))?;
            let reserve = pool.storage_reserve() + EXTRA_STORAGE_FEES;
            return self
                .api
                .account(&addr.to_string())
                .await
                .map(|x| x.balance().saturating_sub(reserve));
        }
        self.api
            .account(&self.wallet.address().await?.to_string())
            .await
            .map(|x| x.balance().saturating_sub(gas_fee).saturating_sub(WALLET_STORAGE_RESERVE))
    }

    async fn wallet_balance(&mut self) -> anyhow::Result<u64> {
        self.api.account(&self.wallet.address().await?.to_string()).await.map(|x| x.balance())
    }

    async fn find_election_key(&mut self, election_id: u64) -> Option<ValidatorEntry> {
        let mut validator_entry = self.validator_config.find(election_id);
        if let Some(entry) = validator_entry.as_mut() {
            let pubkey = self
                .api
                .export_public_key(entry.key_id.as_slice())
                .await
                .map_err(|e| tracing::error!("export public key error: {}", e))
                .ok()?;
            entry.public_key = pubkey;
        }
        validator_entry
    }
}

pub(crate) struct ElectionRunner {
    nodes: HashMap<String, Node>,
    elector: Arc<dyn ElectorWrapper>,
    default_max_factor: f32,
    default_stake_policy: StakePolicy,
    past_elections: Vec<PastElections>,
    /// Election ID for which `past_elections` and `cached_prev_min_eff` were fetched.
    /// Used to avoid redundant RPC calls within the same election round.
    past_elections_cache_id: u64,
    /// Cached prev_min_eff_stake computed from past_elections.
    cached_prev_min_eff: Option<u64>,
    // Snapshot cache updated during tick execution and published to SnapshotStore in run_loop().
    snapshot_cache: SnapshotCache,
    /// AdaptiveSplit50: minimum wait fraction of election duration.
    sleep_pct: f64,
    /// AdaptiveSplit50: maximum wait fraction of election duration.
    waiting_pct: f64,
    /// Callback to persist freshly generated static ADNL addresses into runtime config.
    /// `None` in tests that don't care about persistence.
    persist_static_adnls: Option<PersistStaticAdnls>,
}

#[derive(Default)]
struct SnapshotCache {
    last_elections: Option<ElectionsSnapshot>,
    last_elections_status: ElectionsStatus,
    last_max_factor: Option<f32>,
    next_elections_range: Option<TimeRange>,
    // Current validator set (config param 34), cached for is_validator/index calculation.
    last_validator_set: Option<ValidatorSet>,
    // Next validator set (config param 36), if exists.
    last_next_validator_set: Option<ValidatorSet>,
    /// Last binding statuses, cached for comparison in run_loop().
    last_binding_statuses: HashMap<String, BindingStatus>,
}

impl SnapshotCache {
    fn update_next_elections_range(&mut self, cfg15: &ConfigParam15) {
        if let Some(vset) = &self.last_validator_set {
            let now = time_format::now();
            let elections_start_before = cfg15.elections_start_before as u64;
            let elections_end_before = cfg15.elections_end_before as u64;
            let validators_elected_for = cfg15.validators_elected_for as u64;
            // utime_until of the current validator set == election_id of the next cycle.
            let current_cycle_end = vset.utime_until() as u64;
            let upcoming_elections_end = current_cycle_end.saturating_sub(elections_end_before);
            // If the upcoming cycle's elections window has already closed, advance to the
            // cycle after that.
            let next_cycle_election_id = if now >= upcoming_elections_end {
                current_cycle_end.saturating_add(validators_elected_for)
            } else {
                current_cycle_end
            };
            let start = next_cycle_election_id.saturating_sub(elections_start_before);
            let end = next_cycle_election_id.saturating_sub(elections_end_before);
            self.next_elections_range = Some(TimeRange {
                start,
                start_utc: time_format::format_ts(start),
                end,
                end_utc: time_format::format_ts(end),
            });
        }
    }
}

struct ConfigParams<'a> {
    elections_info: &'a ElectionsInfo,
    cfg15: &'a ConfigParam15,
    cfg16: &'a ConfigParam16,
    cfg17: &'a ConfigParam17,
}

/// Context needed by [`ElectionRunner::calc_stake`].
/// Built from individual `ElectionRunner` fields to avoid borrow conflicts.
struct StakeContext<'a> {
    past_elections: &'a [PastElections],
    our_max_factor: u32,
    sleep_pct: f64,
    waiting_pct: f64,
    prev_min_eff_stake: Option<u64>,
}

impl ElectionRunner {
    /// Fill `node.pool_addr_cache` if the node has a pool but no cached address.
    /// On failure the node is appended to `skip_tick_nodes` so participation is deferred
    /// to the next tick; the next tick's resolve pass will retry.
    async fn resolve_pool_addr(
        node_id: &str,
        node: &mut Node,
        skip_tick_nodes: &mut Vec<String>,
    ) {
        let Some(pool) = &node.pool else {
            node.pool_addr_cache = None;
            return;
        };
        if node.pool_addr_cache.is_some() {
            return;
        }
        match pool.address().await {
            Ok(addr) => {
                tracing::info!("node [{}] pool address cached: {}", node_id, addr);
                node.pool_addr_cache = Some(addr);
            }
            Err(e) => {
                tracing::error!("node [{}] pool address error: {}", node_id, e);
                skip_tick_nodes.push(node_id.to_string());
            }
        }
    }

    pub(crate) fn new(
        elections_config: &ElectionsConfig,
        bindings: &HashMap<String, NodeBinding>,
        elector: Arc<dyn ElectorWrapper>,
        providers: HashMap<String, Box<dyn ElectionsProvider>>,
        wallets: Arc<HashMap<String, Arc<dyn TonWallet>>>,
        pools: Arc<HashMap<String, Arc<dyn NominatorWrapper>>>,
        persist_static_adnls: Option<PersistStaticAdnls>,
    ) -> Self {
        let mut nodes = HashMap::new();
        for (node_id, provider) in providers {
            let wallet = match wallets.get(&node_id) {
                Some(wallet) => wallet.clone(),
                None => {
                    tracing::error!("node [{}] skipped: wallet not found", node_id);
                    continue;
                }
            };
            let node_pools = pools.get(&node_id).cloned();
            let binding = bindings.get(&node_id);
            let excluded = !binding.map(|b| b.enable).unwrap_or(false);
            let binding_status = binding.map(|b| b.status).unwrap_or(BindingStatus::Idle);
            let stake_policy = elections_config.stake_policy(&node_id).clone();
            let static_adnl_disabled = elections_config.static_adnl_disabled.contains(&node_id);
            let static_adnl = elections_config.static_adnls.get(&node_id).and_then(|b64| {
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64)
                    .map_err(|e| {
                        tracing::error!("node [{}] invalid static_adnl base64: {}", node_id, e);
                    })
                    .ok()
            });
            nodes.insert(
                node_id,
                Node {
                    api: provider,
                    wallet,
                    pool: node_pools,
                    pool_addr_cache: None,
                    static_adnl_addr: static_adnl,
                    static_adnl_disabled,
                    excluded,
                    stake_policy,
                    key_id: vec![],
                    participant: None,
                    submission_time: None,
                    stake_accepted: false,
                    accepted_stake_amount: None,
                    stake_submissions: Vec::new(),
                    is_validator: false,
                    is_next_validator: false,
                    last_error: None,
                    validator_config: ValidatorConfig::new(),
                    binding_status,
                    last_recover_amount: 0,
                    withdraw_requests_pending: false,
                },
            );
        }
        Self {
            default_max_factor: elections_config.max_factor,
            default_stake_policy: elections_config.policy.clone(),
            nodes,
            elector,
            snapshot_cache: SnapshotCache::default(),
            past_elections: vec![],
            past_elections_cache_id: 0,
            cached_prev_min_eff: None,
            sleep_pct: elections_config.sleep_period_pct,
            waiting_pct: elections_config.waiting_period_pct,
            persist_static_adnls,
        }
    }

    pub async fn run_loop(
        &mut self,
        tick_interval: Duration,
        cancellation_ctx: CancellationCtx,
        store: Arc<SnapshotStore>,
        on_status_change: Option<OnStatusChange>,
    ) -> anyhow::Result<()> {
        let mut cancellation_rx = cancellation_ctx.subscribe();
        tracing::info!("tick interval: {} seconds", tick_interval.as_secs());
        let mut interval = tokio::time::interval(tick_interval);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    tracing::info!("TICK");

                    // Clear per-node last_error at the start of the tick (best-effort).
                    for node in self.nodes.values_mut() {
                        node.last_error = None;
                        node.withdraw_requests_pending = false;
                    }
                    self.refresh_validator_set().await;
                    self.refresh_next_validator_set().await;
                    self.refresh_validator_configs().await;

                    if let Err(e) = &self.run().await {
                        tracing::error!("runner tick error: {:#}", e);
                    }

                    // Publish snapshot after each tick (also computes binding statuses).
                    self.publish_snapshot(&store).await;

                    // Report binding status changes if callback provided.
                    if let Some(callback) = &on_status_change {
                        let statuses = self.take_binding_status_changes();
                        if !statuses.is_empty() {
                            callback(statuses);
                        }
                    }

                    tracing::info!("SLEEP");
                }
                _ = cancellation_rx.changed() => {
                    tracing::info!("cancel received");
                    if let Err(e) = self.shutdown().await {
                        tracing::error!("runner failed to shutdown: {}", e);
                    }
                    return Ok(());
                }
            }
        }
    }

    pub(crate) async fn run(&mut self) -> anyhow::Result<()> {
        self.ensure_static_adnls().await;
        let cfg15 = self.election_parameters().await?;
        self.snapshot_cache.update_next_elections_range(&cfg15);

        let election_id =
            self.elector.get_active_election_id().await.context("get_active_election_id")?;
        if election_id == 0 {
            self.snapshot_cache.last_elections_status = ElectionsStatus::Closed;
            tracing::info!("no active elections");
            return Ok(());
        }
        tracing::info!(
            "elections parameters: validators_elected_for={}, elections_start_before={}, elections_end_before={}, stake_held_for={}",
            cfg15.validators_elected_for,
            cfg15.elections_start_before,
            cfg15.elections_end_before,
            cfg15.stake_held_for
        );
        tracing::info!(
            "elections are open: id={} time={}",
            election_id,
            time_format::format_ts(election_id)
        );

        Self::print_election_cycle(&cfg15, election_id);

        let elections_info = self.elector.elections_info().await.context("elections_info")?;
        tracing::info!(
            "elections: close={}({}), min_stake={} TON, total_stake={} TON, failed={}, finished={}, participants={}",
            time_format::format_ts(elections_info.elect_close),
            elections_info.elect_close,
            elections_info.min_stake as f64 / 1_000_000_000.0,
            elections_info.total_stake as f64 / 1_000_000_000.0,
            elections_info.failed,
            elections_info.finished,
            elections_info.participants.len()
        );

        // Config param 17: effective `max_factor` in snapshot; 16/17: participation (e.g. AdaptiveSplit50).
        let cfg16 = self.fetch_config_param_16().await?;
        let cfg17 = self.fetch_config_param_17().await?;

        self.build_elections_snapshot(election_id, &cfg15, &elections_info, &cfg17).await;

        let mut skip_tick_nodes = vec![];

        // Pool address cache must be valid before any branch that uses `stake_addr`/`pool_target`
        // (the finished branch below included). TONCore router pool address changes per election
        // cycle (the router alternates between two pools), so invalidate the cache on election_id
        // transition. SNP pool addresses are stable but invalidating uniformly is cheap.
        // Also covers elections-task restart: `past_elections_cache_id` is 0 after start, so the
        // first tick lands here and re-resolves.
        if self.past_elections_cache_id != election_id {
            for node in self.nodes.values_mut() {
                if node.pool.is_some() {
                    node.pool_addr_cache = None;
                }
            }
        }
        // Resolve pool address for any node where it isn't cached yet. On election_id transition
        // the cache was just invalidated above; on other ticks this recovers from a transient
        // `pool.address()` failure (e.g. a `get_pool_data` parse error on TONCore).
        for (node_id, node) in self.nodes.iter_mut() {
            Self::resolve_pool_addr(node_id, node, &mut skip_tick_nodes).await;
        }

        if elections_info.finished {
            self.snapshot_cache.last_elections_status = ElectionsStatus::Finished;
            tracing::warn!("elections are finished");
            // check if node stakes are accepted by the elector
            for (node_id, node) in self.nodes.iter_mut() {
                // Reset previous state; only mark as accepted if present in current participants
                node.stake_accepted = false;
                node.accepted_stake_amount = None;
                // Skip nodes whose pool address didn't resolve this tick: we cannot determine
                // the correct staking address, and the next tick will retry.
                if skip_tick_nodes.contains(node_id) {
                    continue;
                }

                let staking_addr = node.stake_addr().await?;
                if let Some(p) =
                    elections_info.participants.iter().find(|p| p.wallet_addr == staking_addr)
                {
                    node.stake_accepted = true;
                    node.accepted_stake_amount = Some(p.stake);
                }
            }
            return Ok(());
        }
        if elections_info.failed {
            self.snapshot_cache.last_elections_status = ElectionsStatus::Failed;
            tracing::warn!("elections marked as failed");
        }

        if time_format::now() < elections_info.elect_close {
            self.snapshot_cache.last_elections_status = ElectionsStatus::Active;
        } else {
            self.snapshot_cache.last_elections_status = ElectionsStatus::Postponed;
        }

        // Fetch past_elections only when election_id changes (cache across ticks).
        if self.past_elections_cache_id != election_id {
            self.past_elections = self.elector.past_elections().await.context("past_elections")?;
            self.cached_prev_min_eff = self
                .past_elections
                .first()
                .and_then(|pe| pe.frozen_map.values().min_by_key(|f| f.stake).map(|f| f.stake));

            if let Some(prev) = self.cached_prev_min_eff {
                tracing::info!(
                    "prev_min_eff_stake from past elections: {} TON",
                    nanotons_to_tons_f64(prev)
                );
            }
            self.past_elections_cache_id = election_id;
        }

        // walk through the nodes and try to participate in the elections
        let mut nodes = self
            .nodes
            .keys()
            .cloned()
            .filter(|id| !skip_tick_nodes.contains(id))
            .collect::<Vec<String>>();
        nodes.sort();
        for node_id in nodes {
            tracing::info!("node [{}] recover stake", node_id);
            let excluded = self.nodes.get(&node_id).map(|node| node.excluded).unwrap_or(true);
            let recover_amount = match self.recover_stake(&node_id).await {
                Ok(amount) => amount,
                Err(e) => {
                    if let Some(node) = self.nodes.get_mut(&node_id) {
                        node.last_error = Some(e.to_string());
                    }
                    tracing::error!("node [{}] recover stake error: {}", node_id, e);
                    continue;
                }
            };
            if excluded || recover_amount > 0 {
                // skip elections for this node in two cases:
                // 1) the node is excluded from elections by config
                // 2) there is some amount to recover so wait until recovered amount will be returned
                tracing::info!(
                    "node [{}] skip elections: excluded={}, recover_amount={} TON",
                    node_id,
                    excluded,
                    recover_amount as f64 / 1_000_000_000.0
                );
                continue;
            }

            // TONCore-only: probe `has_withdraw_requests` each tick before staking; send op = 2 when
            // the queue is non-empty and skip `participate` this tick. Next tick probes again (new
            // withdraws can appear after op = 2). RPC/build failures log `last_error` but do not
            // skip participation unless `Ok(true)` (opcode-2 actually sent this tick).
            match self.process_pending_withdraw_requests(&node_id, election_id).await {
                Ok(true) => {
                    tracing::info!(
                        "node [{}] skip participate this tick: withdraw requests sent, awaiting pool drain",
                        node_id
                    );
                    continue;
                }
                Ok(false) => {}
                Err(e) => {
                    if let Some(node) = self.nodes.get_mut(&node_id) {
                        node.last_error = Some(format!("{:#}", e));
                    }
                    tracing::warn!("node [{}] withdraw requests error: {:#}", node_id, e);
                }
            }

            tracing::info!("node [{}] participate in elections: id={}", node_id, election_id);
            let config_params = ConfigParams {
                elections_info: &elections_info,
                cfg15: &cfg15,
                cfg16: &cfg16,
                cfg17: &cfg17,
            };
            if let Err(e) = self.participate(&node_id, election_id, &config_params).await {
                if let Some(node) = self.nodes.get_mut(&node_id) {
                    node.last_error = Some(format!("{:#}", e));
                }
                tracing::error!("node [{}] participate error: {:#}", node_id, e);
            }
        }
        Ok(())
    }

    async fn build_elections_snapshot(
        &mut self,
        election_id: u64,
        cfg15: &ConfigParam15,
        elections_info: &ElectionsInfo,
        cfg17: &ConfigParam17,
    ) {
        self.snapshot_cache.last_max_factor = Some(self.calc_max_factor(cfg17.max_stake_factor).1);

        // Include the selected staking target (pool_addr_cache) or wallet fallback.
        let mut wallet_addrs: HashSet<Vec<u8>> = HashSet::new();
        for node in self.nodes.values() {
            match node.stake_addr().await {
                Ok(addr) => {
                    let _ = wallet_addrs.insert(addr);
                }
                Err(e) => {
                    tracing::error!("stake addr error: {:#}", e);
                }
            }
        }

        let participants = Self::build_participants_snapshot(elections_info, &wallet_addrs);
        let participant_min_stake =
            elections_info.participants.iter().map(|p| p.stake).min().map(nanotons_to_dec_string);
        let participant_max_stake =
            elections_info.participants.iter().map(|p| p.stake).max().map(nanotons_to_dec_string);

        let validation_start = election_id;
        let validation_end = election_id + cfg15.validators_elected_for as u64;
        let elections_start = election_id.saturating_sub(cfg15.elections_start_before as u64);
        let elections_end = election_id.saturating_sub(cfg15.elections_end_before as u64);
        let validation_start_utc = time_format::format_ts(validation_start);
        let validation_end_utc = time_format::format_ts(validation_end);
        let elections_start_utc = time_format::format_ts(elections_start);
        let elections_end_utc = time_format::format_ts(elections_end);
        let next_validation_range = TimeRange {
            start: validation_start,
            start_utc: validation_start_utc,
            end: validation_end,
            end_utc: validation_end_utc,
        };
        let elections_range = TimeRange {
            start: elections_start,
            start_utc: elections_start_utc,
            end: elections_end,
            end_utc: elections_end_utc,
        };

        let snapshot = ElectionsSnapshot {
            election_id,
            elect_close: elections_info.elect_close,
            elect_close_utc: time_format::format_ts(elections_info.elect_close),
            finished: elections_info.finished,
            failed: elections_info.failed,
            participants_count: elections_info.participants.len() as u32,
            min_stake: nanotons_to_dec_string(elections_info.min_stake),
            participant_min_stake,
            participant_max_stake,
            total_stake: nanotons_to_dec_string(elections_info.total_stake),
            next_validation_range,
            elections_range,
            participants,
        };
        self.snapshot_cache.last_elections = Some(snapshot);
    }

    async fn participate(
        &mut self,
        node_id: &str,
        election_id: u64,
        params: &ConfigParams<'_>,
    ) -> anyhow::Result<()> {
        let configured_raw = self.configured_max_factor_raw();
        let (max_factor, _) = self.calc_max_factor(params.cfg17.max_stake_factor);
        if max_factor != configured_raw {
            tracing::warn!(
                "max_factor clamped: configured={}, used={} (network limit from cfg17)",
                max_stake_factor_raw_to_multiplier(configured_raw),
                max_stake_factor_raw_to_multiplier(max_factor),
            );
        }
        let stake_ctx = StakeContext {
            past_elections: &self.past_elections,
            our_max_factor: max_factor,
            sleep_pct: self.sleep_pct,
            waiting_pct: self.waiting_pct,
            prev_min_eff_stake: self.cached_prev_min_eff,
        };
        let node = self.nodes.get_mut(node_id).expect("node not found");

        // Resolve target once per tick:
        // if pool address is cached use it, otherwise fallback to elector.
        let elector_addr = self.elector.address().await?;
        // address to which the wallet will send stake request: pool or elector.
        // `pool_target()?` errors if pool is configured but its address is not yet cached —
        // this prevents the request from being misrouted directly to the elector.
        let to_addr = node.pool_target()?.cloned().unwrap_or(elector_addr);
        // address from which the stake will be sent to elector: wallet or pool
        let from_addr = node.stake_addr().await?;
        // Find validator key for current elections in the validator config
        let validator_key = node.find_election_key(election_id).await;
        // Find participant in the elections info by validator public key
        let participant = validator_key.as_ref().and_then(|entry| {
            params
                .elections_info
                .participants
                .iter()
                .find(|p| p.pub_key == entry.public_key)
                .cloned()
        });

        // Refresh participant if missing or stale (different election cycle)
        let needs_refresh = match node.participant.as_ref() {
            Some(existing) => existing.election_id != election_id,
            None => true,
        };
        if needs_refresh {
            node.stake_accepted = false;
            node.accepted_stake_amount = None;
            node.submission_time = None;
            node.stake_submissions.clear();
            node.participant = match (participant.as_ref(), validator_key.as_ref()) {
                (Some(p), _) => Some(p.clone()),
                (None, Some(v)) => {
                    let adnl_addr = v
                        .adnl_addr()
                        .ok_or_else(|| anyhow::anyhow!("validator has no adnl address"))?;
                    if adnl_addr.is_empty() {
                        anyhow::bail!("validator adnl address is empty");
                    }
                    if v.public_key.is_empty() {
                        anyhow::bail!("validator public key is empty");
                    }
                    Some(Participant {
                        stake_message_boc: None,
                        pub_key: v.public_key.clone(),
                        adnl_addr,
                        election_id,
                        wallet_addr: from_addr.clone(),
                        stake: 0,
                        max_factor,
                    })
                }
                (None, None) => None,
            };
            node.key_id =
                validator_key.as_ref().map(|entry| entry.key_id.clone()).unwrap_or_default();
        }
        // If the elector already has our stake, mark it accepted early
        // so that `calc_stake` uses the correct current_stake (not 0).
        if let Some(participant) = participant.as_ref() {
            tracing::info!(
                "node [{}] stake found in elector: stake={} TON, sender_addr=-1:{}, pubkey={}, adnl={}, election_id={}",
                node_id,
                display_tons(participant.stake),
                hex::encode(&participant.wallet_addr),
                hex::encode(participant.pub_key.as_slice()),
                base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    participant.adnl_addr.as_slice(),
                ),
                participant.election_id
            );
            node.stake_accepted = true;
            node.accepted_stake_amount = Some(participant.stake);
        }
        let elections_stake = participant.as_ref().map(|p| p.stake).unwrap_or(0);
        let stake = Self::calc_stake(node, node_id, elections_stake, params, &stake_ctx)
            .await
            .context("stake calculation error")?;

        if stake == 0 {
            tracing::info!("node [{}] skipping elections this tick (stake=0)", node_id);
            return Ok(());
        }

        tracing::info!(
            "node [{}] max_factor={}, stake={} TON, strategy={}",
            node_id,
            max_factor,
            stake as f64 / 1_000_000_000.0,
            serde_json::to_string(&node.stake_policy).unwrap_or_default()
        );

        match validator_key {
            None => {
                tracing::warn!(
                    "node [{}] validator key not found: election_id={}",
                    node_id,
                    election_id
                );
                let key_expired_at =
                    election_id + params.cfg15.validators_elected_for as u64 + EXPIRED_LAG;
                let (key_id, pub_key) = node
                    .api
                    .new_validator_key(election_id, key_expired_at)
                    .await
                    .map_err(|e| anyhow::anyhow!("new validator key error: {}", e))?;
                tracing::info!(
                    "node [{}] generate new validator key: id={}, expired_at={}",
                    node_id,
                    base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        key_id.as_slice()
                    ),
                    key_expired_at
                );
                let adnl_addr = node
                    .resolve_adnl_addr(key_id.clone(), key_expired_at)
                    .await
                    .map_err(|e| anyhow::anyhow!("adnl address error: {}", e))?;
                tracing::info!(
                    "node [{}] {} adnl address: {}",
                    node_id,
                    if node.static_adnl_disabled { "generated new" } else { "registered static" },
                    base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        adnl_addr.as_slice()
                    )
                );
                node.participant = Some(Participant {
                    stake_message_boc: None,
                    pub_key,
                    adnl_addr,
                    election_id,
                    wallet_addr: from_addr.clone(),
                    stake,
                    max_factor,
                });
                node.key_id = key_id;
                Self::send_stake(node_id, node, stake, to_addr).await?;
                Ok(())
            }
            Some(entry) => {
                node.key_id = entry.key_id.clone();
                tracing::info!(
                    "node [{}] validator key found: election_id={} key_id={}, pubkey={}",
                    node_id,
                    election_id,
                    base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        entry.key_id.as_slice()
                    ),
                    hex::encode(entry.public_key.as_slice())
                );
                match participant {
                    Some(_) => {
                        if matches!(node.stake_policy, StakePolicy::AdaptiveSplit50) && stake > 0 {
                            let old_stake = node.participant.as_ref().map(|p| p.stake).unwrap_or(0);
                            tracing::info!(
                                "node [{}] adaptive_split50: top-up {} TON → {} TON (delta={} TON)",
                                node_id,
                                nanotons_to_tons_f64(old_stake),
                                nanotons_to_tons_f64(old_stake + stake),
                                nanotons_to_tons_f64(stake),
                            );
                            Self::send_stake(node_id, node, stake, to_addr).await?;
                            node.participant.as_mut().map(|p| p.stake += stake);
                        }
                    }
                    None => {
                        tracing::warn!("node [{}] stake not found in elector", node_id);
                        if let Some(p) = node.participant.as_mut() {
                            p.stake = stake;
                        }
                        Self::send_stake(node_id, node, stake, to_addr).await?;
                    }
                }
                Ok(())
            }
        }
    }

    async fn send_stake(
        node_id: &str,
        node: &mut Node,
        stake: u64,
        to_addr: MsgAddressInt,
    ) -> anyhow::Result<()> {
        tracing::info!("node [{}] build stake message", node_id);
        let payload = Self::build_new_stake_payload(node_id, node, stake).await?;
        let fee = ELECTOR_STAKE_FEE + NPOOL_COMPUTE_FEE;
        let stake_balance = node.stake_balance(fee).await?;
        if stake_balance < stake {
            anyhow::bail!(
                "low stake balance: required={} TON, available={} TON",
                stake as f64 / 1_000_000_000.0,
                stake_balance as f64 / 1_000_000_000.0
            );
        }
        let wallet_balance = node.wallet_balance().await?;
        if wallet_balance < fee + WALLET_COMPUTE_FEE {
            anyhow::bail!(
                "low wallet balance: required={} TON, available={} TON",
                (fee + WALLET_COMPUTE_FEE) as f64 / 1_000_000_000.0,
                wallet_balance as f64 / 1_000_000_000.0
            );
        }

        let send_value = node.pool.as_ref().map(|_| fee).unwrap_or(stake + fee);
        let msg_boc = write_boc(&node.wallet.message(to_addr, send_value, payload).await?)?;
        tracing::debug!("wallet external message: boc={}", hex::encode(&msg_boc));
        tracing::info!("node [{}] send stake", node_id);
        node.api.send_boc(&msg_boc).await?;
        let submission_time = time_format::now();
        let max_factor = node.participant.as_ref().map(|p| p.max_factor).unwrap_or(0);
        if let Some(participant) = &mut node.participant {
            participant.stake_message_boc = Some(msg_boc);
        }
        node.submission_time = Some(submission_time);
        node.stake_submissions.push(StakeSubmissionRecord { stake, max_factor, submission_time });
        // Cap submissions to avoid unbounded growth
        if node.stake_submissions.len() > MAX_STAKE_SUBMISSIONS {
            node.stake_submissions.remove(0);
        }
        Ok(())
    }

    async fn build_new_stake_payload(
        node_id: &str,
        node: &mut Node,
        stake: u64,
    ) -> anyhow::Result<Cell> {
        let Some(participant) = &mut node.participant else {
            anyhow::bail!("node [{}] no participant info", node_id);
        };
        tracing::info!(
            "node [{}] build stake: election_id={}, max_factor={}, stake={} TON, pubkey={}, adnl={}",
            node_id,
            participant.election_id,
            participant.max_factor,
            stake as f64 / 1_000_000_000.0,
            hex::encode(participant.pub_key.as_slice()),
            base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                participant.adnl_addr.as_slice()
            )
        );
        // todo: move to ElectorWrapper
        // validator-elect-req.fif
        let mut data = 0x654C5074u32.to_be_bytes().to_vec();
        data.extend_from_slice(&(participant.election_id as u32).to_be_bytes());
        data.extend_from_slice(&participant.max_factor.to_be_bytes());
        data.extend_from_slice(&participant.wallet_addr);
        data.extend_from_slice(&participant.adnl_addr);
        tracing::debug!("data to sign {}", hex::encode_upper(&data));
        let signature = node.api.sign(node.key_id.clone(), data).await?;
        let body = nominator::new_stake(&nominator::NewStakeParams {
            query_id: UnixTime::now(),
            stake_amount: stake,
            validator_pubkey: participant.pub_key.as_slice(),
            stake_at: participant.election_id as u32,
            max_factor: participant.max_factor,
            adnl_addr: participant.adnl_addr.as_slice(),
            signature: signature.as_slice(),
        })?;

        tracing::debug!("message body {}", body);
        Ok(body)
    }

    async fn build_recover_stake_payload() -> anyhow::Result<Cell> {
        let body = nominator::recover_stake(UnixTime::now())?;
        tracing::trace!("message body {}", body);
        Ok(body)
    }

    /// Send `process_withdraw_requests` (TONCore op = 2) when the pool's `withdraw_requests` queue
    /// is non-empty (per `has_withdraw_requests` on each tick). Returns `Ok(true)` only when the
    /// message was actually broadcast (caller should skip `participate` for this tick to give the
    /// pool time to drain before computing the new stake amount).
    ///
    /// Non-fatal cases — `has_withdraw_requests` is false (including non-TONCore pools where the trait
    /// default returns without an on-chain probe), the probe failed transiently, or the validator wallet
    /// balance is below the opcode-2 fee — return `Ok(false)`.
    async fn process_pending_withdraw_requests(
        &mut self,
        node_id: &str,
        election_id: u64,
    ) -> anyhow::Result<bool> {
        let node = self.nodes.get_mut(node_id).expect("node not found");
        let Some(pool) = node.pool.clone() else {
            return Ok(false);
        };

        // RPC failures here are transient — log and skip without blocking participation.
        let has_withdraw_requests = match pool.has_withdraw_requests().await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    "node [{}] withdraw requests probe failed (will retry next tick): {:#}",
                    node_id,
                    e
                );
                return Ok(false);
            }
        };

        // Same-tick snapshot + next probe: queue state comes from the contract, not a "sent op" flag.
        node.withdraw_requests_pending = has_withdraw_requests;

        if !has_withdraw_requests {
            return Ok(false);
        }

        let fee = WITHDRAW_PROCESS_GAS + WALLET_COMPUTE_FEE;
        let wallet_balance = node.wallet_balance().await?;
        if wallet_balance < fee {
            tracing::warn!(
                "node [{}] skip process_withdraw_requests: low wallet balance (required={} TON, available={} TON)",
                node_id,
                fee as f64 / 1_000_000_000.0,
                wallet_balance as f64 / 1_000_000_000.0
            );
            return Ok(false);
        }

        let wallet = node.wallet.clone();
        let msg = pool
            .send_process_withdraw_requests(
                wallet,
                UnixTime::now(),
                WITHDRAW_PROCESS_LIMIT,
                WITHDRAW_PROCESS_GAS,
            )
            .await
            .context("build process_withdraw_requests message")?;
        let msg_boc = write_boc(&msg).context("encode process_withdraw_requests boc")?;
        node.api.send_boc(&msg_boc).await.context("send process_withdraw_requests boc")?;

        tracing::info!(
            "node [{}] process_withdraw_requests sent (limit={}, election_id={})",
            node_id,
            WITHDRAW_PROCESS_LIMIT,
            election_id
        );
        Ok(true)
    }

    async fn recover_stake(&mut self, node_id: &str) -> anyhow::Result<u64> {
        let node = self.nodes.get_mut(node_id).expect("node not found");

        let amount = self.elector.compute_returned_stake(&node.stake_addr().await?).await?;
        node.last_recover_amount = amount;
        if amount > 0 {
            tracing::info!(
                "node [{}] send recover stake: amount={} TON",
                node_id,
                amount as f64 / 1_000_000_000.0
            );
            let fee = RECOVER_FEE + WALLET_COMPUTE_FEE;
            let wallet_balance = node.wallet_balance().await?;
            if wallet_balance < fee {
                anyhow::bail!(
                    "node [{}] low wallet balance: required={} TON, available={} TON",
                    node_id,
                    fee as f64 / 1_000_000_000.0,
                    wallet_balance as f64 / 1_000_000_000.0
                );
            }
            let elector_addr = self.elector.address().await?;
            // pool_target() errors if pool is set but its address is not cached yet — avoids
            // routing recover stake to the elector when the pool is actually configured.
            let to_addr = node.pool_target()?.cloned().unwrap_or(elector_addr);
            let msg_boc = write_boc(
                &node
                    .wallet
                    .message(to_addr, RECOVER_FEE, Self::build_recover_stake_payload().await?)
                    .await?,
            )?;
            node.api.send_boc(&msg_boc).await?;
        }
        Ok(amount)
    }

    pub(crate) async fn shutdown(&mut self) -> anyhow::Result<()> {
        for (node_id, node) in self.nodes.iter_mut() {
            tracing::info!("node [{}] shutdown provider", node_id);
            node.reset_participation();
            if let Err(e) = node.api.shutdown().await {
                tracing::error!("node [{}] shutdown error: {}", node_id, e);
            }
        }
        Ok(())
    }

    async fn election_parameters(&mut self) -> anyhow::Result<ConfigParam15> {
        for (node_id, node) in self.nodes.iter_mut() {
            match node.api.election_parameters().await {
                Ok(cfg) => return Ok(cfg),
                Err(e) => {
                    tracing::warn!("node [{}] get election parameters error: {}", node_id, e)
                }
            }
        }
        anyhow::bail!("get election parameters: all nodes failed");
    }

    async fn fetch_config_param_16(&mut self) -> anyhow::Result<ConfigParam16> {
        for (node_id, node) in self.nodes.iter_mut() {
            match node.api.config_param_16().await {
                Ok(cfg) => return Ok(cfg),
                Err(e) => {
                    tracing::warn!("node [{}] get config param 16 error: {:#}", node_id, e)
                }
            }
        }
        anyhow::bail!("get config param 16: all nodes failed");
    }

    async fn fetch_config_param_17(&mut self) -> anyhow::Result<ConfigParam17> {
        for (node_id, node) in self.nodes.iter_mut() {
            match node.api.config_param_17().await {
                Ok(cfg) => return Ok(cfg),
                Err(e) => {
                    tracing::warn!("node [{}] get config param 17 error: {:#}", node_id, e)
                }
            }
        }
        anyhow::bail!("get config param 17: all nodes failed");
    }

    fn print_election_cycle(cfg15: &ConfigParam15, election_id: u64) {
        let validation_start =
            time_format::format_ts(election_id - cfg15.validators_elected_for as u64);
        let validation_end = time_format::format_ts(election_id);
        tracing::info!("validation: start={}, end={}", validation_start, validation_end);
        let elections_start =
            time_format::format_ts(election_id - cfg15.elections_start_before as u64);
        let elections_end = time_format::format_ts(election_id - cfg15.elections_end_before as u64);
        tracing::info!("elections: start={}, end={}", elections_start, elections_end);
    }

    #[inline]
    fn configured_max_factor_raw(&self) -> u32 {
        (self.default_max_factor * MAX_STAKE_FACTOR_SCALE) as u32
    }

    /// Resolves elector `max_factor`: fixed-point `raw` for the Elector and `multiplier` for logs/UI.
    ///
    /// Applies configured `default_max_factor` and clamps to the chain cap
    /// (`network_max_stake_factor_raw` from masterchain config param 17), in fixed-point
    /// `[65536, network_max_stake_factor_raw]` (see [`MAX_STAKE_FACTOR_SCALE`]).
    fn calc_max_factor(&self, network_max_stake_factor_raw: u32) -> (u32, f32) {
        let configured_raw = self.configured_max_factor_raw();
        let raw = configured_raw.clamp(MAX_STAKE_FACTOR_SCALE as u32, network_max_stake_factor_raw);
        (raw, max_stake_factor_raw_to_multiplier(raw))
    }

    /// Calculate stake for a node according to the stake policy.
    async fn calc_stake(
        node: &mut Node,
        node_id: &str,
        elections_stake: u64,
        configs: &ConfigParams<'_>,
        ctx: &StakeContext<'_>,
    ) -> anyhow::Result<u64> {
        let min_stake = configs.elections_info.min_stake;
        tracing::info!("node [{}] calc stake", node_id);
        let fee = ELECTOR_STAKE_FEE + NPOOL_COMPUTE_FEE;
        let stake_addr = node.stake_addr().await?;
        let mut frozen_stake = 0;
        // Calculate frozen stake from past elections
        for election in ctx.past_elections {
            let validator_entry = node.find_election_key(election.election_id).await;
            if let Some(entry) = validator_entry {
                let mut pubkey_array = [0u8; 32];
                pubkey_array.copy_from_slice(&entry.public_key);
                frozen_stake += election
                    .frozen_map
                    .get(&pubkey_array)
                    .map(|frozen| {
                        if frozen.wallet_addr.as_slice() == stake_addr.as_slice() {
                            frozen.stake
                        } else {
                            0
                        }
                    })
                    .unwrap_or(0);
            }
        }

        // Get pool free balance
        let pool_free_balance = node.stake_balance(fee).await?;
        let total_balance =
            frozen_stake.saturating_add(pool_free_balance).saturating_add(elections_stake);
        tracing::info!(
            "node [{}] frozen_stake={} TON, pool_balance={} TON, elections_stake={} TON, total_balance={} TON",
            node_id,
            frozen_stake as f64 / 1_000_000_000.0,
            pool_free_balance as f64 / 1_000_000_000.0,
            elections_stake as f64 / 1_000_000_000.0,
            total_balance as f64 / 1_000_000_000.0
        );
        if total_balance < min_stake {
            anyhow::bail!(
                "not enough funds: available={} TON, min_stake={} TON",
                total_balance as f64 / 1_000_000_000.0,
                min_stake as f64 / 1_000_000_000.0
            );
        }

        // IMPORTANT: split50/AdaptiveSplit50 policy is supported only for SNP nominator pools.
        // Details: TONCore nominator has two different pools, each pool stakes in its own round,
        // they cannot stake in same round, so split50/AdaptiveSplit50 cannot be used; instead stake the full
        // liquid balance of the selected pool (still >= min_stake).
        if matches!(&node.stake_policy, StakePolicy::Split50 | StakePolicy::AdaptiveSplit50)
            && node.pool.as_ref().is_some_and(|p| p.pool_kind() == PoolKind::TONCore)
        {
            tracing::info!(
                "node [{}] {}: TONCore nominator - ignore, stake all",
                node_id,
                node.stake_policy.to_string()
            );
            return Ok(total_balance);
        }

        match &node.stake_policy {
            StakePolicy::AdaptiveSplit50 => {
                if !adaptive_strategy::is_adaptive_split50_ready(
                    node_id,
                    configs.elections_info,
                    configs.cfg15.elections_start_before,
                    configs.cfg15.elections_end_before,
                    configs.cfg16,
                    ctx.sleep_pct,
                    ctx.waiting_pct,
                ) {
                    return Ok(0);
                }
                let current_stake = if node.stake_accepted { elections_stake } else { 0 };
                let stakes: Vec<_> = configs
                    .elections_info
                    .participants
                    .iter()
                    .filter(|p| {
                        p.pub_key.as_slice()
                            != node
                                .participant
                                .as_ref()
                                .map(|p| p.pub_key.as_slice())
                                .unwrap_or_default()
                    })
                    .map(|p| ParticipantStake { stake: p.stake, max_factor: p.max_factor })
                    .collect();
                adaptive_strategy::calc_adaptive_stake(
                    node_id,
                    total_balance,
                    pool_free_balance,
                    current_stake,
                    ctx.our_max_factor,
                    stakes,
                    configs.cfg16,
                    configs.cfg17,
                    ctx.prev_min_eff_stake,
                )
            }
            other => other.calculate_stake(min_stake, total_balance),
        }
    }

    fn build_participants_snapshot(
        elections_info: &ElectionsInfo,
        wallet_addrs: &HashSet<Vec<u8>>,
    ) -> Vec<ElectionsParticipantSnapshot> {
        elections_info
            .participants
            .iter()
            .map(|p| ElectionsParticipantSnapshot {
                pubkey: base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    p.pub_key.as_slice(),
                ),
                adnl: base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    p.adnl_addr.as_slice(),
                ),
                sender_addr: format!("-1:{}", hex::encode(&p.wallet_addr)),
                is_controlled: wallet_addrs.contains(&p.wallet_addr),
                stake: nanotons_to_dec_string(p.stake),
                max_factor: p.max_factor as f32 / 65536.0,
                election_id: p.election_id,
            })
            .collect()
    }

    async fn refresh_validator_set(&mut self) {
        tracing::trace!("fetch validator set");
        let mut last_err: Option<anyhow::Error> = None;
        for (node_id, node) in self.nodes.iter_mut() {
            match node.api.get_current_vset().await {
                Ok(vset) => {
                    self.snapshot_cache.last_validator_set = Some(vset);
                    return;
                }
                Err(e) => {
                    tracing::warn!("node [{}] get vset error: {}", node_id, e);
                    last_err = Some(e);
                }
            }
        }

        if self.nodes.is_empty() {
            tracing::warn!("get vset: no nodes configured");
        } else if let Some(e) = last_err {
            tracing::warn!("get vset: all nodes failed (last error: {})", e);
        }
        self.snapshot_cache.last_validator_set = None;
    }

    async fn refresh_next_validator_set(&mut self) {
        tracing::trace!("fetch next validator set (p36)");
        for (_node_id, node) in self.nodes.iter_mut() {
            match node.api.get_next_vset().await {
                Ok(Some(vset)) => {
                    self.snapshot_cache.last_next_validator_set = Some(vset);
                    return;
                }
                Ok(None) => {
                    // Node returned no next validator set, try other nodes.
                    tracing::trace!("get next vset: node returned no next validator set (None)");
                }
                Err(e) => {
                    tracing::trace!("get next vset error: {}", e);
                }
            }
        }
        self.snapshot_cache.last_next_validator_set = None;
    }

    async fn refresh_validator_configs(&mut self) {
        tracing::trace!("fetch validator configs");
        for (node_id, node) in self.nodes.iter_mut() {
            node.validator_config = match node.api.validator_config().await {
                Ok(cfg) => cfg,
                Err(e) => {
                    tracing::error!("node [{}] get validator config error: {}", node_id, e);
                    ValidatorConfig::new()
                }
            };
        }
    }

    /// Generate and persist static ADNL addresses for any node that is missing one
    /// and not explicitly opted out. After the first successful tick this is a no-op.
    ///
    /// Per-node generation failures are logged and retried on the next tick. The
    /// in-memory `Node.static_adnl_addr` is only committed once `persist` succeeds,
    /// so a failed persist leaves the node looking "missing" on the next tick and it
    /// retries cleanly.
    async fn ensure_static_adnls(&mut self) {
        let mut generated: HashMap<String, Vec<u8>> = HashMap::new();
        for (node_id, node) in self.nodes.iter_mut() {
            if node.static_adnl_disabled || node.static_adnl_addr.is_some() {
                continue;
            }
            match node.api.generate_adnl_addr().await {
                Ok(key_id) => {
                    tracing::info!(
                        "node [{}] static adnl address generated: {}",
                        node_id,
                        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &key_id,)
                    );
                    generated.insert(node_id.clone(), key_id);
                }
                Err(e) => {
                    tracing::error!(
                        "node [{}] failed to generate static adnl address (will retry next tick): {:#}",
                        node_id,
                        e
                    );
                }
            }
        }
        if generated.is_empty() {
            return;
        }
        // Persist first; only commit in-memory state if the disk write succeeded.
        if let Some(persist) = &self.persist_static_adnls {
            let payload = generated
                .iter()
                .map(|(id, key)| {
                    (
                        id.clone(),
                        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, key),
                    )
                })
                .collect();
            if let Err(e) = persist(payload) {
                tracing::error!(
                    "static-adnl: persist failed, dropping generated addresses (will retry next tick): {:#}",
                    e
                );
                return;
            }
        }
        for (node_id, key_id) in generated {
            if let Some(node) = self.nodes.get_mut(&node_id) {
                node.static_adnl_addr = Some(key_id);
            }
        }
    }

    async fn build_validators_snapshot(&mut self) -> ValidatorsSnapshot {
        let mut node_ids = self.nodes.keys().cloned().collect::<Vec<String>>();
        node_ids.sort();

        let mut controlled_nodes = Vec::new();
        for node_id in node_ids {
            let node = self.nodes.get_mut(&node_id).expect("node not found");

            let (validator_entry, is_next_validator) = find_validator_entries(
                node,
                self.snapshot_cache.last_validator_set.as_ref(),
                self.snapshot_cache.last_next_validator_set.as_ref(),
            )
            .await
            .map_err(|e| {
                let error =
                    anyhow::anyhow!("node [{}] find validator entry error: {:#}", node_id, e);
                tracing::error!("{:#}", error);
                node.last_error = Some(format!("{:#}", error))
            })
            .unwrap_or((None, false));

            let is_validator = validator_entry.is_some();
            node.is_validator = is_validator;
            node.is_next_validator = is_next_validator;

            let wallet_addr = node.wallet.address().await.ok().map(|a| a.to_string());
            let pool_addr = node.pool_addr_cache.as_ref().map(|a| a.to_string());
            let pubkey = validator_entry.as_ref().map(|(_, entry, ..)| {
                base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    entry.public_key.as_bytes(),
                )
            });
            let adnl = validator_entry
                .as_ref()
                .and_then(|(_, entry, ..)| {
                    entry.adnl_addr.as_ref().map(|x| x.as_slice().as_slice())
                })
                .map(|x| base64::Engine::encode(&base64::engine::general_purpose::STANDARD, x));
            let key_id = validator_entry.as_ref().map(|(.., matched_entry)| {
                base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    &matched_entry.key_id,
                )
            });
            let (key_election_id, key_expires_at, key_expires_at_utc, is_key_active) =
                validator_entry
                    .as_ref()
                    .map(|(_, _, matched_election_id, matched_entry)| {
                        let expires = matched_entry.expired_at;
                        let now = time_format::now();
                        (
                            Some(*matched_election_id),
                            Some(expires),
                            Some(time_format::format_ts(expires)),
                            Some(expires > now),
                        )
                    })
                    .unwrap_or((None, None, None, None));
            let stake = validator_entry.as_ref().and_then(|(_, vd, ..)| {
                let mut pubkey = [0u8; 32];
                pubkey.copy_from_slice(vd.public_key.as_slice());
                self.past_elections
                    .iter()
                    .find_map(|pe| pe.frozen_map.get(&pubkey))
                    .map(|frozen| nanotons_to_dec_string(frozen.stake))
            });

            let validator_index = validator_entry.as_ref().map(|(idx, ..)| *idx);
            let weight = validator_entry.as_ref().map(|(_, entry, ..)| entry.weight);

            // Compute and update binding status
            let is_participating = node.participant.is_some();
            let new_status = Self::compute_node_status(
                node.excluded,
                is_validator,
                node.last_recover_amount > 0,
                is_participating,
            );
            if new_status != node.binding_status {
                tracing::info!(
                    "node [{}] binding status: {} → {}",
                    node_id,
                    node.binding_status,
                    new_status
                );
                node.binding_status = new_status;
            }

            controlled_nodes.push(ValidatorNodeSnapshot {
                node_id,
                is_validator,
                validator_index,
                weight,
                wallet_addr,
                pool_addr,
                pubkey,
                adnl,
                key_id,
                key_election_id,
                key_expires_at,
                key_expires_at_utc,
                is_key_active,
                stake,
                stake_accepted: node.stake_accepted,
                last_error: node.last_error.clone(),
                binding_status: node.binding_status,
            });
        }

        let validation_range =
            self.snapshot_cache.last_validator_set.as_ref().map(|vset| TimeRange {
                start: vset.utime_since() as u64,
                start_utc: time_format::format_ts(vset.utime_since() as u64),
                end: vset.utime_until() as u64,
                end_utc: time_format::format_ts(vset.utime_until() as u64),
            });

        ValidatorsSnapshot {
            controlled_nodes,
            default_stake_policy: self.default_stake_policy.clone(),
            validation_range,
        }
    }

    async fn build_our_participants_snapshot(&self) -> Vec<OurElectionParticipant> {
        let elections_snapshot = self.snapshot_cache.last_elections.as_ref();

        // Build ranked list of participants by stake (descending) for position calculation
        let ranked_participants: Vec<&ElectionsParticipantSnapshot> =
            if let Some(snapshot) = elections_snapshot {
                let mut sorted: Vec<_> = snapshot.participants.iter().collect();
                sorted.sort_by(|a, b| {
                    let stake_a: u128 = a.stake.parse().unwrap_or(0);
                    let stake_b: u128 = b.stake.parse().unwrap_or(0);
                    stake_b.cmp(&stake_a)
                });
                sorted
            } else {
                Vec::new()
            };

        let mut node_ids = self.nodes.keys().cloned().collect::<Vec<String>>();
        node_ids.sort();

        let mut our_participants = Vec::new();
        for node_id in node_ids {
            let node = self.nodes.get(&node_id).expect("node not found");
            let participant = node.participant.as_ref();
            let wallet_addr = node.wallet.address().await.ok().map(|a| a.to_string());
            let pool_addr = node.pool_addr_cache.as_ref().map(|a| a.to_string());

            let pubkey = participant.map(|p| {
                base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    p.pub_key.as_slice(),
                )
            });
            let adnl = participant.map(|p| {
                base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    p.adnl_addr.as_slice(),
                )
            });
            let key_id = if node.key_id.is_empty() {
                None
            } else {
                Some(base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    node.key_id.as_slice(),
                ))
            };

            let stake_submissions: Vec<StakeSubmission> = node
                .stake_submissions
                .iter()
                .map(|s| StakeSubmission {
                    stake: nanotons_to_dec_string(s.stake),
                    max_factor: s.max_factor as f32 / 65536.0,
                    submission_time: s.submission_time,
                    submission_time_utc: time_format::format_ts(s.submission_time),
                })
                .collect();

            let staking_addrs: Vec<String> = match node.stake_addr().await {
                Ok(addr) => vec![format!("-1:{}", hex::encode(addr))],
                Err(e) => {
                    tracing::error!("stake addr error: {:#}", e);
                    vec![]
                }
            };
            let accepted_stake = if node.stake_accepted {
                node.accepted_stake_amount.map(nanotons_to_dec_string).or_else(|| {
                    node.stake_submissions.last().map(|s| nanotons_to_dec_string(s.stake))
                })
            } else {
                None
            };

            // Find position in ranked list (1-based); for TONCore nominator pair check both addresses.
            let position = ranked_participants
                .iter()
                .position(|p| staking_addrs.contains(&p.sender_addr))
                .map(|pos| (pos + 1) as u32);

            let elections_running = matches!(
                self.snapshot_cache.last_elections_status,
                ElectionsStatus::Active | ElectionsStatus::Finished | ElectionsStatus::Postponed
            );
            // `ProcessingWithdrawRequests`: elections active, stake not yet submitted, and the last
            // on-chain probe still reports a non-empty `withdraw_requests` queue for this tick.
            let processing_withdraw_requests = elections_running
                && node.stake_submissions.is_empty()
                && node.withdraw_requests_pending;
            let status = if node.is_next_validator {
                ParticipationStatus::Elected
            } else if elections_running && node.stake_accepted {
                ParticipationStatus::Accepted
            } else if elections_running && !node.stake_submissions.is_empty() {
                ParticipationStatus::Submitted
            } else if processing_withdraw_requests {
                ParticipationStatus::ProcessingWithdrawRequests
            } else if elections_running && node.participant.is_some() {
                ParticipationStatus::Participating
            } else if node.is_validator {
                ParticipationStatus::Validating
            } else {
                ParticipationStatus::Idle
            };

            our_participants.push(OurElectionParticipant {
                node_id,
                status,
                pubkey,
                key_id,
                adnl,
                stake_submissions,
                accepted_stake,
                stake_accepted: node.stake_accepted,
                elected: node.is_validator || node.is_next_validator,
                position,
                wallet_addr,
                pool_addr,
                last_error: node.last_error.clone(),
            });
        }

        our_participants
    }

    async fn publish_snapshot(&mut self, store: &SnapshotStore) {
        tracing::trace!("update snapshot");
        let elections = self.snapshot_cache.last_elections.clone();
        let elections_status = self.snapshot_cache.last_elections_status.clone();
        let next_elections_range = self.snapshot_cache.next_elections_range.clone();
        let validators = self.build_validators_snapshot().await;
        let our_participants = self.build_our_participants_snapshot().await;
        store.update_with(|s| {
            s.elections = elections;
            s.elections_status = elections_status;
            s.next_elections_range = next_elections_range;
            s.our_participants = our_participants;
            s.validators = validators;
        });
    }

    pub(crate) fn compute_node_status(
        excluded: bool,
        is_validator: bool,
        has_recover: bool,
        is_participating: bool,
    ) -> BindingStatus {
        if is_validator {
            return BindingStatus::Validating;
        }

        if excluded {
            if has_recover { BindingStatus::Draining } else { BindingStatus::Idle }
        } else if is_participating {
            BindingStatus::Participating
        } else if has_recover {
            BindingStatus::Draining
        } else {
            BindingStatus::Idle
        }
    }

    /// Returns a map of node_id → new BindingStatus for nodes whose status
    /// changed during the last tick. Called after `publish_snapshot`.
    pub(crate) fn take_binding_status_changes(&mut self) -> HashMap<String, BindingStatus> {
        let mut changes = HashMap::new();
        let last_binding_statuses = &mut self.snapshot_cache.last_binding_statuses;
        for (node_id, node) in self.nodes.iter() {
            let last_status = last_binding_statuses
                .insert(node_id.clone(), node.binding_status)
                .unwrap_or(BindingStatus::Idle);
            if node.binding_status != last_status {
                changes.insert(node_id.clone(), node.binding_status);
            }
        }
        changes
    }
}

async fn find_validator_entries(
    node: &mut Node,
    current_vset: Option<&ValidatorSet>,
    next_vset: Option<&ValidatorSet>,
) -> anyhow::Result<(Option<(u16, ValidatorDescr, u64, ValidatorEntry)>, bool)> {
    let config = &node.validator_config;

    let mut election_ids = config.keys.keys().cloned().collect::<Vec<_>>();
    election_ids.sort();

    let mut current_entry: Option<(u16, ValidatorDescr, u64, ValidatorEntry)> = None;
    let mut is_in_next = false;

    for election_id in &election_ids[election_ids.len().saturating_sub(3)..] {
        let entry = config
            .keys
            .get(election_id)
            .ok_or_else(|| anyhow::anyhow!("validator entry not found"))?;

        let public_key = node.api.export_public_key(&entry.key_id).await?;
        let mut key = [0u8; 32];
        key.copy_from_slice(&public_key);

        if current_entry.is_none()
            && let Some(vset) = current_vset
            && let Some(idx) =
                vset.list().iter().position(|item| item.public_key.as_slice() == &key)
        {
            current_entry =
                Some((u16::try_from(idx)?, vset.list()[idx].clone(), *election_id, entry.clone()));
        }

        if !is_in_next
            && let Some(vset) = next_vset
            && vset.list().iter().any(|item| item.public_key.as_slice() == &key)
        {
            is_in_next = true;
        }

        if current_entry.is_some() && is_in_next {
            break;
        }
    }

    Ok((current_entry, is_in_next))
}
