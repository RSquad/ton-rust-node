/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    adaptive_strategy,
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
    ton_utils::{display_tons, nanotons_to_dec_string, nanotons_to_tons_f64},
};
use contracts::{
    ElectionsInfo, ElectorWrapper, NominatorWrapper, Participant, TonWallet,
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
/// Reserved minimum balance on the wallet (or pool) balance for stake calculations
const MIN_NANOTON_FOR_STORAGE: u64 = 1_100_000_000;

type OnStatusChange = Arc<dyn Fn(HashMap<String, BindingStatus>) + Send + Sync>;

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
    /// Nominator pool instance. Optional.
    pool: Option<Arc<dyn NominatorWrapper>>,
    /// Address to which to send commands: stake & recover.
    /// It can be an elector address or a nominator pool address.
    elections_address: MsgAddressInt,
    /// Last error observed for this node during the current/previous tick (stringified).
    last_error: Option<String>,
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
    fn wallet_addr(&self) -> Vec<u8> {
        self.pool
            .as_ref()
            .map(|p| p.address())
            .unwrap_or_else(|| self.wallet.address())
            .address()
            .clone()
            .storage()
            .to_vec()
    }
    fn elections_addr(&self) -> MsgAddressInt {
        self.elections_address.clone()
    }
    fn reset_participation(&mut self) {
        self.participant = None;
        self.submission_time = None;
        self.stake_accepted = false;
        self.accepted_stake_amount = None;
        self.stake_submissions.clear();
    }
    async fn stake_balance(&mut self, gas_fee: u64) -> anyhow::Result<u64> {
        match self.pool.as_ref() {
            Some(pool) => self.api.account(&pool.address().to_string()).await.map(|x| x.balance()),
            None => self
                .api
                .account(&self.wallet.address().to_string())
                .await
                .map(|x| x.balance().saturating_sub(gas_fee)),
        }
        .map(|b| b.saturating_sub(MIN_NANOTON_FOR_STORAGE))
    }
    async fn wallet_balance(&mut self) -> anyhow::Result<u64> {
        self.api.account(&self.wallet.address().to_string()).await.map(|x| x.balance())
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
    adaptive_sleep_pct: f64,
    /// AdaptiveSplit50: maximum wait fraction of election duration.
    adaptive_waiting_pct: f64,
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
            let next_elections_start = if time_format::now()
                > vset.utime_until().saturating_sub(cfg15.elections_start_before) as u64
            {
                vset.utime_until().saturating_add(cfg15.elections_start_before)
            } else {
                vset.utime_since().saturating_add(cfg15.elections_start_before)
            };
            let next_elections_end = next_elections_start
                .saturating_add(cfg15.elections_start_before + cfg15.elections_end_before);
            self.next_elections_range = Some(TimeRange {
                start: next_elections_start as u64,
                start_utc: time_format::format_ts(next_elections_start as u64),
                end: next_elections_end as u64,
                end_utc: time_format::format_ts(next_elections_end as u64),
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
    adaptive_sleep_pct: f64,
    adaptive_waiting_pct: f64,
    prev_min_eff_stake: Option<u64>,
}

impl ElectionRunner {
    pub(crate) fn new(
        elections_config: &ElectionsConfig,
        bindings: &HashMap<String, NodeBinding>,
        elector: Arc<dyn ElectorWrapper>,
        providers: HashMap<String, Box<dyn ElectionsProvider>>,
        wallets: Arc<HashMap<String, Arc<dyn TonWallet>>>,
        pools: Arc<HashMap<String, Arc<dyn NominatorWrapper>>>,
    ) -> Self {
        Self {
            default_max_factor: elections_config.max_factor,
            default_stake_policy: elections_config.policy.clone(),
            nodes: providers
                .into_iter()
                .filter_map(|(node_id, provider)| {
                    let wallet = match wallets.get(&node_id) {
                        Some(wallet) => wallet.clone(),
                        None => {
                            tracing::error!("node [{}] skipped: wallet not found", node_id);
                            return None;
                        }
                    };
                    let pool = pools.get(&node_id).cloned();
                    let binding = bindings.get(&node_id);
                    let excluded = !binding.map(|b| b.enable).unwrap_or(false);
                    let binding_status = binding.map(|b| b.status).unwrap_or(BindingStatus::Idle);
                    let stake_policy = elections_config.stake_policy(&node_id).clone();
                    Some((
                        node_id,
                        Node {
                            api: provider,
                            elections_address: pool
                                .as_ref()
                                .map(|p| p.address())
                                .unwrap_or_else(|| elector.address()),
                            wallet,
                            pool,
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
                        },
                    ))
                })
                .collect::<HashMap<String, Node>>(),
            elector,
            snapshot_cache: SnapshotCache::default(),
            past_elections: vec![],
            past_elections_cache_id: 0,
            cached_prev_min_eff: None,
            adaptive_sleep_pct: elections_config.adaptive_sleep_period_pct,
            adaptive_waiting_pct: elections_config.adaptive_waiting_period_pct,
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

        self.build_elections_snapshot(election_id, &cfg15, &elections_info);

        if elections_info.finished {
            self.snapshot_cache.last_elections_status = ElectionsStatus::Finished;
            tracing::warn!("elections are finished");
            // check if node stakes are accepted by the elector
            for node in self.nodes.values_mut() {
                // Reset previous state; only mark as accepted if present in current participants
                node.stake_accepted = false;
                node.accepted_stake_amount = None;
                if let Some(p) =
                    elections_info.participants.iter().find(|p| p.wallet_addr == node.wallet_addr())
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
            self.past_elections_cache_id = election_id;
            if let Some(prev) = self.cached_prev_min_eff {
                tracing::info!(
                    "prev_min_eff_stake from past elections: {} TON",
                    nanotons_to_tons_f64(prev)
                );
            }
        }
        // Fetch config params 16/17 - used for AdaptiveSplit50 strategy
        let cfg16 = self.fetch_config_param_16().await?;
        let cfg17 = self.fetch_config_param_17().await?;

        // walk through the nodes and try to participate in the elections
        let mut nodes = self.nodes.keys().cloned().collect::<Vec<String>>();
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

    fn build_elections_snapshot(
        &mut self,
        election_id: u64,
        cfg15: &ConfigParam15,
        elections_info: &ElectionsInfo,
    ) {
        self.snapshot_cache.last_max_factor = Some(self.calc_max_factor());

        // It can be a validator wallet or nominator pool address.
        let wallet_addrs: HashSet<Vec<u8>> =
            self.nodes.values().map(|node| node.wallet_addr()).collect();

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
        let max_factor = (self.calc_max_factor() * 65536.0) as u32;
        let stake_ctx = StakeContext {
            past_elections: &self.past_elections,
            our_max_factor: max_factor,
            adaptive_sleep_pct: self.adaptive_sleep_pct,
            adaptive_waiting_pct: self.adaptive_waiting_pct,
            prev_min_eff_stake: self.cached_prev_min_eff,
        };
        let node = self.nodes.get_mut(node_id).expect("node not found");
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
            // Reset participation-related state for the new election cycle
            node.stake_accepted = false;
            node.accepted_stake_amount = None;
            node.submission_time = None;
            node.stake_submissions.clear();
            node.participant = participant.clone();
            node.key_id = validator_key.as_ref().map(|entry| entry.key_id.clone()).unwrap_or_default();
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
                    .api
                    .new_adnl_addr(key_id.clone(), key_expired_at)
                    .await
                    .map_err(|e| anyhow::anyhow!("new adnl address error: {}", e))?;
                tracing::info!(
                    "node [{}] generate new adnl address: {}",
                    node_id,
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
                    wallet_addr: node.wallet_addr(),
                    stake,
                    max_factor,
                });
                node.key_id = key_id;
                Self::send_stake(node_id, node, stake).await?;
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
                                "node [{}] adaptive top-up: {} TON → {} TON (delta={} TON)",
                                node_id,
                                nanotons_to_tons_f64(old_stake),
                                nanotons_to_tons_f64(old_stake + stake),
                                nanotons_to_tons_f64(stake),
                            );
                            Self::send_stake(node_id, node, stake).await?;
                            node.participant.as_mut().map(|p| p.stake += stake);
                        }
                    }
                    None => {
                        tracing::warn!("node [{}] stake not found in elector", node_id);
                        if let Some(p) = node.participant.as_mut() {
                            p.stake = stake;
                        }
                        Self::send_stake(node_id, node, stake).await?;
                    }
                }
                Ok(())
            }
        }
    }

    async fn send_stake(node_id: &str, node: &mut Node, stake: u64) -> anyhow::Result<()> {
        tracing::info!("node [{}] build stake message", node_id);
        let payload = Self::build_new_stake_payload(node_id, node, stake).await?;
        // For simplicity we always assume that the node has nominator pool.
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

        // if node has nominator pool, the wallet should send only gas fee,
        // otherwise the wallet should send stake + gas fee
        let send_value = node.pool.as_ref().map(|_| fee).unwrap_or(stake + fee);
        let msg_boc =
            write_boc(&node.wallet.message(node.elections_addr(), send_value, payload).await?)?;
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
        if !(1.0..=3.0).contains(&(participant.max_factor as f32 / 65536.0)) {
            anyhow::bail!("<max-factor> must be a real number 1..3");
        }
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

    async fn recover_stake(&mut self, node_id: &str) -> anyhow::Result<u64> {
        let node = self.nodes.get_mut(node_id).expect("node not found");
        let amount = self.elector.compute_returned_stake(&node.wallet_addr()).await?;
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
            let msg_boc = write_boc(
                &node
                    .wallet
                    .message(
                        node.elections_addr(),
                        RECOVER_FEE,
                        Self::build_recover_stake_payload().await?,
                    )
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

    fn calc_max_factor(&self) -> f32 {
        self.default_max_factor
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
        let mut frozen_stake = 0;
        // Calculate frozen stake from past elections
        for election in ctx.past_elections {
            let validator_entry = node.find_election_key(election.election_id).await;
            if let Some(entry) = validator_entry {
                let mut pubkey_array = [0u8; 32];
                pubkey_array.copy_from_slice(&entry.public_key);
                frozen_stake +=
                    election.frozen_map.get(&pubkey_array).map(|frozen| frozen.stake).unwrap_or(0);
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

        match &node.stake_policy {
            StakePolicy::AdaptiveSplit50 => {
                if !adaptive_strategy::is_adaptive_split50_ready(
                    node_id,
                    configs.elections_info,
                    configs.cfg15.elections_start_before,
                    configs.cfg15.elections_end_before,
                    configs.cfg16,
                    ctx.adaptive_sleep_pct,
                    ctx.adaptive_waiting_pct,
                ) {
                    return Ok(0);
                }
                let current_stake = if node.stake_accepted { elections_stake } else { 0 };
                adaptive_strategy::calc_adaptive_stake(
                    node_id,
                    total_balance,
                    pool_free_balance,
                    current_stake,
                    ctx.our_max_factor,
                    configs.elections_info,
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

    async fn build_validators_snapshot(&mut self) -> ValidatorsSnapshot {
        let current_election_id =
            self.snapshot_cache.last_elections.as_ref().map(|snapshot| snapshot.election_id);

        let mut node_ids = self.nodes.keys().cloned().collect::<Vec<String>>();
        node_ids.sort();

        let mut controlled_nodes = Vec::new();
        for node_id in node_ids {
            let node = self.nodes.get_mut(&node_id).expect("node not found");
            let current_cycle_key =
                current_election_id.and_then(|election_id| node.validator_config.find(election_id));

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

            let participant = node.participant.as_ref();
            let wallet_addr = Some(node.wallet.address().to_string());
            let pool_addr = node.pool.as_ref().map(|p| p.address().to_string());
            let pubkey = validator_entry
                .as_ref()
                .map(|(_, entry)| {
                    base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        entry.public_key.as_bytes(),
                    )
                })
                .or_else(|| {
                    participant.map(|p| {
                        base64::Engine::encode(
                            &base64::engine::general_purpose::STANDARD,
                            p.pub_key.as_slice(),
                        )
                    })
                });
            let adnl = current_cycle_key
                .as_ref()
                .and_then(|entry| entry.adnl_addr())
                .as_deref()
                .or_else(|| {
                    validator_entry.as_ref().and_then(|(_, entry)| {
                        entry.adnl_addr.as_ref().map(|x| x.as_slice().as_slice())
                    })
                })
                .or_else(|| participant.map(|p| p.adnl_addr.as_slice()))
                .map(|x| base64::Engine::encode(&base64::engine::general_purpose::STANDARD, x));
            let key_id = current_cycle_key
                .as_ref()
                .map(|entry| {
                    base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        &entry.key_id,
                    )
                })
                .or_else(|| {
                    if node.key_id.is_empty() {
                        None
                    } else {
                        Some(base64::Engine::encode(
                            &base64::engine::general_purpose::STANDARD,
                            &node.key_id,
                        ))
                    }
                });
            let (key_election_id, key_expires_at, key_expires_at_utc, is_key_active) =
                current_cycle_key
                    .as_ref()
                    .map(|entry| {
                        let expires = entry.expired_at;
                        let now = time_format::now();
                        (
                            current_election_id,
                            Some(expires),
                            Some(time_format::format_ts(expires)),
                            Some(expires > now),
                        )
                    })
                    .unwrap_or((None, None, None, None));
            let stake = participant.map(|p| nanotons_to_dec_string(p.stake));

            let validator_index = validator_entry.as_ref().map(|(idx, _)| *idx);
            let weight = validator_entry.as_ref().map(|(_, entry)| entry.weight);

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

    fn build_our_participants_snapshot(&self) -> Vec<OurElectionParticipant> {
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
            let wallet_addr = Some(node.wallet.address().to_string());
            let pool_addr = node.pool.as_ref().map(|p| p.address().to_string());

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

            let fallback_sender_addr = format!("-1:{}", hex::encode(node.wallet_addr()));
            let accepted_stake = if node.stake_accepted {
                node.accepted_stake_amount.map(nanotons_to_dec_string).or_else(|| {
                    node.stake_submissions.last().map(|s| nanotons_to_dec_string(s.stake))
                })
            } else {
                None
            };

            // Find position in ranked list (1-based)
            let position = ranked_participants
                .iter()
                .position(|p| p.sender_addr == fallback_sender_addr)
                .map(|pos| (pos + 1) as u32);

            let elections_running = matches!(
                self.snapshot_cache.last_elections_status,
                ElectionsStatus::Active | ElectionsStatus::Finished | ElectionsStatus::Postponed
            );
            let status = if node.is_next_validator {
                ParticipationStatus::Elected
            } else if elections_running && node.stake_accepted {
                ParticipationStatus::Accepted
            } else if elections_running && !node.stake_submissions.is_empty() {
                ParticipationStatus::Submitted
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
        let our_participants = self.build_our_participants_snapshot();
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
) -> anyhow::Result<(Option<(u16, ValidatorDescr)>, bool)> {
    let config = &node.validator_config;

    let mut election_ids = config.keys.keys().cloned().collect::<Vec<_>>();
    election_ids.sort();

    let mut current_entry: Option<(u16, ValidatorDescr)> = None;
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
            current_entry = Some((u16::try_from(idx)?, vset.list()[idx].clone()));
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
