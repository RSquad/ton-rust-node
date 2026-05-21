/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    block::BlockStuff,
    block_proof::BlockProofStuff,
    engine_traits::EngineOperations,
    network::{
        build_block_broadcast, build_block_broadcast_compressed,
        build_block_broadcast_compressed_v2, build_block_candidate_broadcast_compressed,
        custom_overlay_client::CustomOverlayClient,
        fast_sync_overlay_client::FastSyncOverlayClient,
        full_node_overlay_client::FullNodeOverlayClient, full_node_service::FullNodeOverlayService,
        node_network::NodeNetwork, overlay_client::OverlayClient, pack_block_signatures,
    },
    types::{awaiters_pool::AwaitersPool, top_block_descr::TopBlockDescrStuff},
    validator::validator_utils::{compute_validator_list_id, get_adnl_id, sigpubkey_to_publickey},
};
#[cfg(feature = "xp25")]
use adnl::OverlayNode;
use adnl::{
    common::{add_unbound_object_to_map_with_update, spawn_cancelable, TaggedByteSlice},
    node::AdnlSendMethod,
    DhtSearchPolicy, OverlayUtils,
};
use std::{
    collections::HashSet,
    net::{Ipv4Addr, SocketAddr},
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        Arc,
    },
    time::Duration,
};
use ton_api::{
    serialize_boxed,
    ton::{
        engine::validator::customoverlay::CustomOverlay,
        ton_node::{
            broadcast::{
                ExternalMessageBroadcast, NewShardBlockBroadcast, OutMsgQueueProofBroadcast,
            },
            externalmessage::ExternalMessage,
        },
    },
    IntoBoxed,
};
#[cfg(feature = "telemetry")]
use ton_api::{
    ton::ton_node::broadcast::{
        BlockBroadcast, BlockBroadcastCompressed, BlockBroadcastCompressedV2,
    },
    BoxedSerialize, Constructor,
};
use ton_block::{
    base64_encode, error, AccountIdPrefixFull, BlockIdExt, BlockSignaturesVariant, Cell,
    ConfigParams, ImportedMsgQueueLimits, KeyId, KeyOption, Result, ShardIdent, ValidatorDescr,
    ValidatorSet, BASE_WORKCHAIN_ID,
};

/// The router encapsulates work with full node overlays at the logical level. It abstracts creation,
/// deletion, and getting of specific overlay clients from higher-level classes. For lower-level
/// classes, overlays are physical entities not tied to shards or other blockchain entities.
///
/// ValidatorManager  Collator  ValidatorQuery  etc.   <- high level node commponents
///       ↓              ↓             ↓
///                 Engine
///                  ↓ ↑
///         **FullNodeOverlaysRouter** contains list of FullNodeOverlayClient
///                    ↓
///               NodeNetwork                   <- low level node components
///                    ↓
///            network protocols
pub struct FullNodeOverlaysRouter {
    engine: Arc<dyn EngineOperations>,
    network: Arc<NodeNetwork>,
    service: Arc<FullNodeOverlayService>,
    policy: DhtSearchPolicy,
    public_overlays: lockfree::map::Map<ShardIdent, Arc<FullNodeOverlayClient>>,
    fast_sync_overlays: lockfree::map::Map<ShardIdent, Arc<FastSyncOverlayClient>>,
    custom_overlays: lockfree::map::Map<String, Arc<CustomOverlayClient>>,
    public_overlays_awaiters: AwaitersPool<ShardIdent, Arc<FullNodeOverlayClient>>,
    validators: tokio::sync::Mutex<ValidatorSet>,
    monitor_min_split_for_fast_sync: AtomicU8,
    last_known_keyblock_id: tokio::sync::Mutex<BlockIdExt>,
    actual_monitor_min_split: AtomicU8,
    monitor_min_split_worker_started: AtomicBool,
    fast_sync_peer_resolver: tokio::sync::Mutex<Option<tokio_util::sync::CancellationToken>>,
}

impl FullNodeOverlaysRouter {
    pub async fn new(
        engine: Arc<dyn EngineOperations>,
        network: Arc<NodeNetwork>,
        policy: DhtSearchPolicy,
    ) -> Result<Arc<Self>> {
        let service = Arc::new(FullNodeOverlayService::new(engine.clone(), false)); // compression = false
        let public_overlays_awaiters = AwaitersPool::new(
            "public_overlays_awaiters",
            #[cfg(feature = "telemetry")]
            engine.engine_telemetry().clone(),
            engine.engine_allocated().clone(),
        );
        let actual_monitor_min_split = AtomicU8::new(engine.get_monitor_min_split());
        let overlays_router = Arc::new(Self {
            engine,
            network,
            service,
            policy,
            public_overlays: lockfree::map::Map::new(),
            fast_sync_overlays: lockfree::map::Map::new(),
            custom_overlays: lockfree::map::Map::new(),
            public_overlays_awaiters,
            validators: tokio::sync::Mutex::new(ValidatorSet::default()),
            monitor_min_split_for_fast_sync: AtomicU8::new(0),
            last_known_keyblock_id: tokio::sync::Mutex::new(BlockIdExt::default()),
            actual_monitor_min_split,
            monitor_min_split_worker_started: AtomicBool::new(false),
            fast_sync_peer_resolver: tokio::sync::Mutex::new(None),
        });
        futures::try_join!(
            overlays_router.overlay_client(&ShardIdent::MASTERCHAIN),
            overlays_router.overlay_client(&ShardIdent::BASE_WORKCHAIN)
        )?;
        Ok(overlays_router)
    }

    pub async fn overlay_client(
        self: &Arc<Self>,
        shard: &ShardIdent,
    ) -> Result<Arc<FullNodeOverlayClient>> {
        let shard = trim_shard(shard, self.actual_monitor_min_split.load(Ordering::Relaxed))?;
        loop {
            match self.public_overlays.get(&shard) {
                Some(guard) => {
                    let client = guard.val();
                    if client.overlay_client().is_active() {
                        return Ok(client.clone());
                    } else if client.overlay_client().is_died() {
                        log::debug!("Overlay client {} is dead - will be recreated", shard);
                        self.public_overlays.remove(&shard);
                    } else {
                        log::debug!(
                            "Overlay client {} was not died yet - will be activated",
                            shard
                        );
                        let _ = client.overlay_client().activate();
                    }
                }
                None => {
                    let result = self
                        .public_overlays_awaiters
                        .do_or_wait(&shard, Some(10_000), async {
                            let wc = shard.workchain_id();
                            let pfx = shard.shard_prefix_with_tag() as i64;
                            let zerostate_file_hash = self.engine.zerostate_id()?.file_hash();
                            let id_full = OverlayUtils::calc_overlay_id(
                                wc,
                                pfx,
                                zerostate_file_hash.as_slice(),
                            )?;
                            let id = OverlayUtils::calc_overlay_short_id(
                                wc,
                                pfx,
                                zerostate_file_hash.as_slice(),
                            )?;
                            log::info!(
                                "Creating public overlay for shard {}, id {}, full id {}",
                                shard,
                                id,
                                base64_encode(id_full)
                            );
                            let client = OverlayClient::new_public(
                                id,
                                id_full,
                                self.network.context().clone(),
                                self.network.cancellation_token().child_token(),
                                self.policy.clone(),
                                self.network.default_rldp_roundtrip(),
                            )
                            .await?;
                            client.add_consumer(self.service.clone())?;
                            let fullnode_client = FullNodeOverlayClient::new(
                                self.engine.clone(),
                                client,
                                shard.clone(),
                            );
                            self.public_overlays.insert(shard.clone(), fullnode_client.clone());

                            Ok(fullnode_client)
                        })
                        .await?;
                    if let Some(client) = result {
                        return Ok(client);
                    }
                }
            }
        }
    }

    // The worker periodically checks for new key blocks (if node is not in sync)
    // and updates monitor min split accordingly. It is so important to know the actual
    // monitor_min_split, otherwise the node may send queries to not existing overlays.
    fn start_monitor_min_split_worker(
        self: Arc<Self>,
        last_known_keyblock_id: BlockIdExt,
        last_known_monitor_min_split: u8,
    ) {
        if self.monitor_min_split_worker_started.fetch_or(true, Ordering::Relaxed) {
            return;
        }

        async fn try_update(router: &Arc<FullNodeOverlaysRouter>) -> Result<()> {
            let last_known_id: BlockIdExt;
            {
                let id = tokio::time::timeout(
                    Duration::from_millis(100),
                    router.last_known_keyblock_id.lock(),
                )
                .await?;
                last_known_id = id.clone();
                let _ = id;
            }

            let handle = router
                .engine
                .load_block_handle(&last_known_id)?
                .ok_or_else(|| error!("Can't load block handle for key block {}", last_known_id))?;
            let mut proof = router.engine.load_block_proof(&handle, false).await?;
            let mut updated = false;
            let mut attempts = 10;
            'top: while attempts > 0 {
                let mut ids = match router.engine.download_next_key_blocks_ids(proof.id()).await {
                    Err(e) => {
                        log::trace!(
                            "Monitor min split worker: no newer key blocks found after {}, error occurred: {e}",
                            proof.id(),
                        );
                        break 'top;
                    }
                    Ok(ids) => ids,
                };
                if ids.is_empty() {
                    log::trace!(
                        "Monitor min split worker: no newer key blocks found after {}",
                        proof.id()
                    );
                    attempts -= 1;
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    continue;
                };

                ids.sort();
                for new_id in ids {
                    if new_id.seq_no <= proof.id().seq_no {
                        attempts -= 1;
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        continue 'top;
                    }
                    log::trace!(
                        "Monitor min split worker: downloading newer key block {}...",
                        new_id,
                    );
                    let Ok(new_proof) =
                        router.engine.download_block_proof(&new_id, false, true).await
                    else {
                        log::warn!(
                            "Monitor min split worker: failed to download newer key block {}",
                            new_id,
                        );
                        attempts -= 1;
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        continue 'top;
                    };
                    log::trace!("Monitor min split worker: checking newer key block {}", new_id,);
                    if new_proof.check_with_prev_key_block_proof(&proof).is_err() {
                        log::warn!(
                            "Monitor min split worker: newer key block {} check failed",
                            new_id,
                        );
                        attempts -= 1;
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        continue 'top;
                    }
                    router.engine.store_block_proof(new_proof.id(), None, &new_proof).await?;
                    attempts = 10;
                    proof = new_proof;
                    updated = true;
                    log::debug!(
                        "Monitor min split worker: newer key block {} downloaded and checked",
                        new_id,
                    );
                }
            }
            if !updated {
                log::trace!("Monitor min split worker: no newer key blocks.");
            } else {
                let (block, _) = router.engine.download_block(&proof.id(), Some(5)).await?;
                router
                    .update_public_overlays(
                        proof.id(),
                        block.read_config_params()?.base_workchain()?.monitor_min_split(),
                    )
                    .await?;
            }
            Ok(())
        }

        spawn_cancelable(self.network.cancellation_token().child_token(), async move {
            {
                let mut id = self.last_known_keyblock_id.lock().await;
                *id = last_known_keyblock_id;
                log::info!(
                    "Starting monitor min split worker: last known keyblock: {}, monitor min split {}",
                    id,
                    last_known_monitor_min_split
                );
                let _ = id;
            }

            loop {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                if self.engine.check_sync().await.unwrap_or(false) {
                    log::trace!("Monitor min split worker: node is in sync - skipping iteration");
                } else {
                    log::trace!("Monitor min split worker: attempt to get newer key blocks...");
                    if let Err(e) = try_update(&self).await {
                        log::warn!("Monitor min split worker: {}", e);
                    }
                }
            }
        });
    }

    pub(crate) async fn update_public_overlays(
        self: &Arc<Self>,
        keyblock_id: &BlockIdExt,
        new_mms: u8,
    ) -> Result<()> {
        // Start the worker if not started yet
        self.clone().start_monitor_min_split_worker(keyblock_id.clone(), new_mms);
        {
            let mut last_known_id = self.last_known_keyblock_id.lock().await;
            if last_known_id.seq_no >= keyblock_id.seq_no {
                log::info!(
                    "Skipping monitor min split update for key block {}: last known key block is {}",
                    keyblock_id,
                    *last_known_id
                );
                return Ok(());
            }
            *last_known_id = keyblock_id.clone();
            let _ = last_known_id;
        }

        let old_mms = self.actual_monitor_min_split.load(Ordering::Relaxed);
        if old_mms == new_mms {
            return Ok(());
        }
        log::info!("Updating monitor min split from {} to {}", old_mms, new_mms);
        if new_mms < old_mms {
            for guard in self.public_overlays.iter() {
                let shard = guard.key();
                if shard.is_base_workchain() {
                    if shard.prefix_len() > new_mms {
                        log::info!("Deactivating public overlay {}", shard);
                        guard.val().overlay_client().deactivate();
                    }
                }
            }
        } else {
            let to_add = calc_new_shards(old_mms, new_mms)?;
            let mut tasks = Vec::new();
            for new in &to_add {
                tasks.push(self.overlay_client(new));
            }
            futures::future::try_join_all(tasks).await?;
        }
        self.actual_monitor_min_split.store(new_mms, Ordering::Relaxed);
        Ok(())
    }

    pub(crate) async fn special_update_fastsync_overlays(self: &Arc<Self>) -> Result<()> {
        // Special case: create fast sync overlays when we are a validator from the
        // previous set. The normal flow (update_validator_lists in validator_manager)
        // only processes current and next sets, so the prev set ADNL key is never
        // loaded and fast sync overlays are not created at startup.

        // 0) Load last master state and config
        let mc_state = self.engine.load_last_applied_mc_state().await?;
        let config = mc_state.config_params()?;

        // 1) Read prev validator set
        let prev_vset = config.prev_validator_set()?;
        let cur_vset = config.validator_set()?;
        let next_vset = config.next_validator_set()?;

        if prev_vset.list().is_empty() {
            return Ok(());
        }

        // 2) Check if keyring (not ADNL) contains a key from the prev set
        //    and doesn't contain anyone from current/next
        let config_handler = self.network.config_handler();
        let validator_key_ids = config_handler.get_actual_validator_key_ids()?;
        if validator_key_ids.is_empty() {
            return Ok(());
        }

        let matched = prev_vset.list().iter().find(|descr| {
            let pubkey = sigpubkey_to_publickey(&descr.public_key);
            validator_key_ids.iter().any(|kid| kid == pubkey.id())
        });
        let Some(matched_descr) = matched else {
            log::info!("special_update_fastsync: not a validator in prev set");
            return Ok(());
        };
        if cur_vset.list().iter().chain(next_vset.list().iter()).any(|descr| {
            let pubkey = sigpubkey_to_publickey(&descr.public_key);
            validator_key_ids.iter().any(|kid| kid == pubkey.id())
        }) {
            log::info!("special_update_fastsync: is a validator in current or next set - skipping");
            return Ok(());
        }

        let pubkey = sigpubkey_to_publickey(&matched_descr.public_key);
        let adnl_id = get_adnl_id(matched_descr);
        let (_, election_id) = config_handler
            .get_validator_key(pubkey.id())
            .await
            .ok_or_else(|| error!("special_update_fastsync: validator key not found in keyring"))?;

        // Build ADNL key candidates: adnl_id from descriptor, pubkey as fallback
        let mut candidates = vec![adnl_id.clone()];
        if *pubkey.id() != adnl_id {
            candidates.push(pubkey.id().clone());
        }

        let adnl = &self.network.context().stack.adnl;

        // Check if ADNL key is already loaded
        let mut adnl_key = None;
        for cid in &candidates {
            if let Ok(key) = adnl.key_by_id(cid) {
                adnl_key = Some(key);
                break;
            }
        }

        // 3) Load the key to ADNL
        if adnl_key.is_none() {
            for cid in &candidates {
                let Some((key, _)) = config_handler.get_validator_key(cid).await else {
                    continue;
                };
                if let Err(e) = adnl.add_key(key.clone(), election_id as usize) {
                    log::warn!("special_update_fastsync: cannot add ADNL key {cid}: {e}");
                    continue;
                }

                // Add to QUIC if available
                if let Some(quic) = &self.network.context().stack.quic {
                    let adnl_ip = adnl.ip_address_adnl();
                    let quic_addr = if let Some(addr) = self.network.context().quic_address {
                        addr
                    } else if let Some(port) =
                        adnl_ip.port().checked_add(adnl::QuicNode::OFFSET_PORT)
                    {
                        SocketAddr::new(Ipv4Addr::from(adnl_ip.ip()).into(), port)
                    } else {
                        log::warn!(
                            "special_update_fastsync: QUIC port overflow for ADNL port {}",
                            adnl_ip.port()
                        );
                        adnl_key = Some(key);
                        break;
                    };
                    let bind_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), quic_addr.port());
                    match key.pvt_key() {
                        Ok(pvt_key) => {
                            if let Err(e) = quic.add_key(
                                (&pvt_key.lock()? as &[u8]).try_into()?,
                                cid,
                                bind_addr,
                            ) {
                                log::warn!(
                                    "special_update_fastsync: cannot add QUIC key {cid}: {e}"
                                );
                            }
                        }
                        Err(e) => {
                            log::warn!(
                                "special_update_fastsync: cannot get private key for {cid}: {e}"
                            );
                        }
                    }
                }

                log::info!(
                    "special_update_fastsync: loaded ADNL key {cid} (election_id={election_id})"
                );
                adnl_key = Some(key);
                break;
            }
        }

        let Some(adnl_key) = adnl_key else {
            log::warn!(
                "special_update_fastsync: matched prev set validator {} \
                 but no ADNL key could be loaded",
                hex::encode(pubkey.id().data())
            );
            return Ok(());
        };

        // 4) Start periodic_store_ip_addr to DHT
        NodeNetwork::start_periodic_store_ip_addr(
            self.network.context().stack.dht.clone(),
            adnl_key.clone(),
            self.network.cancellation_token().child_token(),
        );

        // In update_fast_sync_overlays:
        // 5) Resolve peers and 6) create fast sync overlays
        let mc_use_quic = config.get_mc_simplex_config()?.map_or(false, |c| c.use_quic);
        let shard_use_quic = config.get_shard_simplex_config()?.map_or(false, |c| c.use_quic);
        self.update_fast_sync_overlays(
            &prev_vset,
            &cur_vset,
            &next_vset,
            config.base_workchain()?.monitor_min_split(),
            Some(&adnl_key),
            mc_use_quic,
            shard_use_quic,
        )
        .await
    }

    pub(crate) async fn update_private_overlays(
        self: &Arc<Self>,
        config: &ConfigParams,
    ) -> Result<()> {
        let prev_vset = config.prev_validator_set()?;
        let this_vset = config.validator_set()?;
        let next_vset = config.next_validator_set()?;
        let key = if let Some(k) = self.try_get_our_key(&this_vset)? {
            Some(k)
        } else if let Some(k) = self.try_get_our_key(&prev_vset)? {
            Some(k)
        } else {
            self.try_get_our_key(&next_vset)?
        };
        let mc_use_quic = config.get_mc_simplex_config()?.map_or(false, |c| c.use_quic);
        let shard_use_quic = config.get_shard_simplex_config()?.map_or(false, |c| c.use_quic);
        self.update_fast_sync_overlays(
            &prev_vset,
            &this_vset,
            &next_vset,
            config.base_workchain()?.monitor_min_split(),
            key.as_ref(),
            mc_use_quic,
            shard_use_quic,
        )
        .await?;
        Ok(())
    }

    pub async fn update_custom_overlays(&self, configs: Option<&[CustomOverlay]>) -> Result<()> {
        if let Some(configs) = configs {
            for config in configs {
                add_unbound_object_to_map_with_update(
                    &self.custom_overlays,
                    config.name.clone(),
                    |prev| {
                        if prev.is_some() {
                            log::info!(
                                "Custom overlay \"{}\" already exists - skipping creation",
                                config.name
                            );
                            Ok(None)
                        } else {
                            log::info!("Creating custom overlay \"{}\"", config.name);
                            Ok(Some(CustomOverlayClient::new(
                                config,
                                self.network.cancellation_token().child_token(),
                                self.network.context().stack.adnl.clone(),
                                self.network.context().stack.overlay.clone(),
                                self.network.context().stack.dht.clone(),
                                self.engine.clone(),
                            )?))
                        }
                    },
                )?;
            }
        }

        let mut to_remove = Vec::new();
        for guard in self.custom_overlays.iter() {
            let name = guard.key();
            if let Some(configs) = configs {
                if !configs.iter().any(|c| &c.name == name) {
                    log::info!("Deleting custom overlay \"{}\"", name);
                    guard.val().stop();
                    to_remove.push(name.clone());
                    continue;
                }
            }
            if let Err(e) = guard.val().clone().try_activate() {
                log::warn!("Error activating custom overlay \"{}\": {:?}", name, e);
            }
        }
        for name in to_remove {
            self.custom_overlays.remove(&name);
        }

        Ok(())
    }

    async fn update_fast_sync_overlays(
        self: &Arc<Self>,
        prev_validators: &ValidatorSet,
        this_validators: &ValidatorSet,
        next_validators: &ValidatorSet,
        new_monitor_min_split: u8,
        key: Option<&Arc<dyn KeyOption>>,
        mc_use_quic: bool,
        shard_use_quic: bool,
    ) -> Result<()> {
        let mut cur_validators = self.validators.lock().await;
        let validators_changed = *cur_validators != *this_validators;
        let old_monitor_min_split = self.monitor_min_split_for_fast_sync.load(Ordering::Relaxed);
        if (old_monitor_min_split == new_monitor_min_split) && !validators_changed {
            return Ok(());
        }

        log::info!(
            "Updating fast sync overlays: \
            monitor min split {old_monitor_min_split} -> {new_monitor_min_split}, \
            validators changed {validators_changed}, \
            mc_use_quic {mc_use_quic}, shard_use_quic {shard_use_quic}"
        );

        // Root members = union of past + current + next validator sets.
        // Duplicates (a validator present in multiple rounds) are collapsed by
        // the HashSet/HashMap inside add_semiprivate_overlay
        let mut validators: Vec<ValidatorDescr> = Vec::with_capacity(
            prev_validators.list().len()
                + this_validators.list().len()
                + next_validators.list().len(),
        );
        validators.extend(prev_validators.list().iter().cloned());
        validators.extend(this_validators.list().iter().cloned());
        validators.extend(next_validators.list().iter().cloned());

        let create_overlay = |shard: &ShardIdent, use_quic: bool| {
            FastSyncOverlayClient::new(
                shard.clone(),
                &validators,
                key,
                None,
                self.network.cancellation_token().child_token(),
                self.network.context().clone(),
                self.engine.clone(),
                self.policy.clone(),
                self.network.default_rldp_roundtrip(),
                use_quic,
            )
        };

        let update_monitor_min_split = async |monitor_min_split: u8, create| -> Result<()> {
            for prefix in 0..(1 << monitor_min_split) {
                let shard_prefix =
                    if monitor_min_split == 0 { 0 } else { prefix << (64 - monitor_min_split) };
                let shard = ShardIdent::with_prefix_len(
                    monitor_min_split,
                    BASE_WORKCHAIN_ID,
                    shard_prefix,
                )?;
                if create {
                    if let Some(old) = self.fast_sync_overlays.remove(&shard) {
                        old.val().stop();
                    }
                    let overlay = create_overlay(&shard, shard_use_quic).await?;
                    self.fast_sync_overlays.insert(shard, overlay)
                } else {
                    self.fast_sync_overlays.remove(&shard)
                }
                .map(|removed| removed.val().stop());
            }
            Ok(())
        };

        // Delete old overlays if monitor min split changed or we are not a validator anymore
        if (old_monitor_min_split != new_monitor_min_split) || key.is_none() {
            if key.is_none() {
                if let Some(old) = self.fast_sync_overlays.remove(&ShardIdent::MASTERCHAIN) {
                    old.val().stop();
                }
            }
            update_monitor_min_split(old_monitor_min_split, false).await?;
        }

        let Some(local_key) = key else {
            self.monitor_min_split_for_fast_sync.store(new_monitor_min_split, Ordering::Relaxed);
            log::info!("We are not a validator");
            *cur_validators = this_validators.clone();
            if let Some(prev) = self.fast_sync_peer_resolver.lock().await.take() {
                prev.cancel();
            }
            return Ok(());
        };

        // Resolve ADNL addresses for fastsync root members (prev/this/next union)
        // OverlayNode::pending_peers retry promotes peers automatically as soon
        // as they appear in ADNL.
        let local_key_id = local_key.id().clone();
        let adnl = &self.network.context().stack.adnl;
        let mut to_resolve: Vec<Arc<KeyId>> = Vec::new();
        let mut seen: HashSet<Arc<KeyId>> = HashSet::new();
        for vd in &validators {
            let adnl_id = vd.adnl_addr();
            if !seen.insert(adnl_id.clone()) {
                continue;
            }
            if adnl_id == local_key_id {
                continue;
            }
            if matches!(adnl.peer_ip_address(&local_key_id, &adnl_id), Ok(Some(_))) {
                continue;
            }
            to_resolve.push(adnl_id);
        }
        {
            let mut slot = self.fast_sync_peer_resolver.lock().await;
            if let Some(prev) = slot.take() {
                prev.cancel();
            }
            if !to_resolve.is_empty() {
                log::info!(
                    "fastsync: scheduling DHT resolve for {} peers under {local_key_id}",
                    to_resolve.len()
                );
                let token = self.network.cancellation_token().child_token();
                NodeNetwork::spawn_overlay_peer_resolver(
                    local_key_id.clone(),
                    to_resolve,
                    self.network.context().stack.dht.clone(),
                    self.network.context().stack.overlay.clone(),
                    token.clone(),
                    "fastsync".to_string(),
                );
                *slot = Some(token);
            }
        }

        // Update masterchain overlay
        if validators_changed {
            let shard = ShardIdent::MASTERCHAIN;
            if let Some(old) = self.fast_sync_overlays.remove(&shard) {
                old.val().stop();
            }
            let overlay = create_overlay(&shard, mc_use_quic).await?;
            self.fast_sync_overlays.insert(shard, overlay);
        }

        // Create new shard overlays
        update_monitor_min_split(new_monitor_min_split, true).await?;

        self.monitor_min_split_for_fast_sync.store(new_monitor_min_split, Ordering::Relaxed);
        *cur_validators = this_validators.clone();

        Ok(())
    }

    /// Look up the local ADNL key for the given validator set.
    ///
    /// Returns `None` both when the node is not a validator and when it is a validator
    /// but the ADNL/overlay context is not yet ready. Callers must tolerate `None`
    /// gracefully.
    fn try_get_our_key(
        self: &Arc<Self>,
        validators: &ValidatorSet,
    ) -> Result<Option<Arc<dyn KeyOption>>> {
        if validators.list().is_empty() {
            return Ok(None);
        }

        let val_list_id = compute_validator_list_id(validators.list(), None)?
            .ok_or_else(|| error!("Can't compute validator list id"))?;
        match self.network.try_get_validator_adnl_key(&val_list_id) {
            None => {
                log::info!(
                    "No local validator ADNL key for list {:x} (node is either not a validator \
                    for this list yet, or validator network context is still not ready)",
                    val_list_id
                );
                return Ok(None);
            }
            Some(k) => Ok(Some(k)),
        }
    }

    /// Send block broadcast to network overlays.
    ///
    /// Supports both ordinary (catchain) and simplex signature variants:
    /// - For ordinary signatures: Uses V1 format (backward compatible with older nodes)
    /// - For simplex signatures: Uses V2 format (required for simplex signature scheme)
    ///
    /// This matches C++ behavior in `serialize_block_broadcast()` which automatically
    /// chooses V2 for non-ordinary signatures.
    pub async fn send_block_broadcast(
        self: &Arc<Self>,
        block: &BlockStuff,
        proof: &BlockProofStuff,
        signatures: &BlockSignaturesVariant,
    ) -> Result<()> {
        log::debug!("Sending block broadcast {}...", block.id());

        let mut custom_overlays = vec![];
        for guard in self.custom_overlays.iter() {
            let overlay = guard.val();
            if overlay.sends_blocks_to(block.id().shard()) {
                custom_overlays.push(overlay.clone());
            }
        }
        let fast_sync_client = self.fast_sync_overlay(block.id().shard());

        match signatures {
            BlockSignaturesVariant::Ordinary(sigs) => {
                // Use V1 format for ordinary signatures (backward compatibility)
                let packed_signatures = pack_block_signatures(sigs)?;
                let catchain_seqno = sigs.validator_info.catchain_seqno;
                let validator_set_hash = sigs.validator_info.validator_list_hash_short;

                if fast_sync_client.is_some() || !custom_overlays.is_empty() {
                    let broadcast = TaggedByteSlice {
                        object: &serialize_boxed(
                            &build_block_broadcast_compressed(
                                block,
                                proof,
                                catchain_seqno,
                                packed_signatures.clone(),
                                validator_set_hash,
                            )?
                            .into_boxed(),
                        )?,
                        #[cfg(feature = "telemetry")]
                        tag: BlockBroadcastCompressed::constructor_const(),
                    };
                    if let Some(fast_sync_client) = &fast_sync_client {
                        if fast_sync_client.use_twostep() {
                            fast_sync_client.send_twostep_broadcast(&broadcast, 0).await?;
                        } else {
                            fast_sync_client
                                .send_broadcast(&broadcast, 0, AdnlSendMethod::Fast)
                                .await?;
                        }
                    }
                    for overlay in custom_overlays {
                        overlay.send_broadcast(&broadcast, 0, AdnlSendMethod::Fast).await?;
                    }
                }

                let client = self.overlay_client(block.id().shard()).await?;
                let broadcast = TaggedByteSlice {
                    object: &serialize_boxed(
                        &build_block_broadcast(
                            block,
                            proof,
                            catchain_seqno,
                            packed_signatures,
                            validator_set_hash,
                        )
                        .into_boxed(),
                    )?,
                    #[cfg(feature = "telemetry")]
                    tag: BlockBroadcast::constructor_const(),
                };
                client.send_broadcast(&broadcast, 0, AdnlSendMethod::Fast).await?;
            }
            BlockSignaturesVariant::Simplex(_) => {
                // Use V2 format for simplex signatures (required for proper verification)
                let broadcast_data = serialize_boxed(
                    &build_block_broadcast_compressed_v2(block, proof, signatures)?.into_boxed(),
                )?;
                let broadcast = TaggedByteSlice {
                    object: &broadcast_data,
                    #[cfg(feature = "telemetry")]
                    tag: BlockBroadcastCompressedV2::constructor_const(),
                };

                if let Some(fast_sync_client) = &fast_sync_client {
                    if fast_sync_client.use_twostep() {
                        fast_sync_client.send_twostep_broadcast(&broadcast, 0).await?;
                    } else {
                        fast_sync_client
                            .send_broadcast(&broadcast, 0, AdnlSendMethod::Fast)
                            .await?;
                    }
                }
                for overlay in custom_overlays {
                    overlay.send_broadcast(&broadcast, 0, AdnlSendMethod::Fast).await?;
                }

                let client = self.overlay_client(block.id().shard()).await?;
                client.send_broadcast(&broadcast, 0, AdnlSendMethod::Fast).await?;
            }
        }

        Ok(())
    }

    pub async fn send_top_shard_block_description(
        self: &Arc<Self>,
        tbd: Arc<TopBlockDescrStuff>,
    ) -> Result<()> {
        log::debug!("Sending top shard blocks broadcast {}...", tbd.proof_for());

        let broadcast = TaggedByteSlice {
            object: &serialize_boxed(
                &NewShardBlockBroadcast { block: tbd.new_shard_block()? }.into_boxed(),
            )?,
            #[cfg(feature = "telemetry")]
            tag: NewShardBlockBroadcast::constructor_const(),
        };

        #[cfg(not(feature = "xp25"))]
        let flags = 0;
        #[cfg(feature = "xp25")]
        let flags = OverlayNode::FLAG_BCAST_REPEATED;

        if let Some(client) = self.fast_sync_overlay(&ShardIdent::masterchain()) {
            client.send_broadcast(&broadcast, flags, AdnlSendMethod::Fast).await?;
        }
        let client = self.overlay_client(tbd.proof_for().shard()).await?;
        client.send_broadcast(&broadcast, flags, AdnlSendMethod::Fast).await?;
        Ok(())
    }

    pub async fn send_ext_message_broadcast(
        self: &Arc<Self>,
        to: &AccountIdPrefixFull,
        data: &[u8],
    ) -> Result<()> {
        let broadcast = TaggedByteSlice {
            object: &serialize_boxed(
                &ExternalMessageBroadcast { message: ExternalMessage { data: data.to_vec() } }
                    .into_boxed(),
            )?,
            #[cfg(feature = "telemetry")]
            tag: ExternalMessageBroadcast::constructor_const(),
        };
        let mut skip_public = false;
        for guard in self.custom_overlays.iter() {
            let overlay = guard.val();
            if overlay.sends_msgs_to(to) {
                overlay.send_broadcast(&broadcast, 0, AdnlSendMethod::Fast).await?;
                if overlay.skip_public_msg_send() {
                    skip_public = true;
                }
            }
        }
        if !skip_public {
            self.overlay_client(&to.shard_ident()?)
                .await?
                .send_broadcast(&broadcast, 0, AdnlSendMethod::Fast)
                .await?;
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn send_block_candidate_broadcast(
        &self,
        id: &BlockIdExt,
        cc_seqno: u32,
        validator_set_hash: u32,
        block_root: &Cell,
    ) -> Result<()> {
        let mut custom_overlays = vec![];
        for guard in self.custom_overlays.iter() {
            let overlay = guard.val();
            if overlay.sends_blocks_to(id.shard()) {
                custom_overlays.push(overlay.clone());
            }
        }
        let fast_sync_client = self.fast_sync_overlay(id.shard());
        if !custom_overlays.is_empty() || fast_sync_client.is_some() {
            log::debug!("Sending block candidate broadcast {}...", id);
            let broadcast = build_block_candidate_broadcast_compressed(
                id.clone(),
                cc_seqno,
                validator_set_hash,
                block_root,
            )?
            .into_boxed();
            let broadcast = TaggedByteSlice {
                object: &serialize_boxed(&broadcast)?,
                #[cfg(feature = "telemetry")]
                tag: broadcast.bare_object().constructor(),
            };
            for overlay in custom_overlays {
                overlay.send_broadcast(&broadcast, 0, AdnlSendMethod::Fast).await?;
            }
            if let Some(client) = fast_sync_client {
                if client.use_twostep() {
                    client.send_twostep_broadcast(&broadcast, 0).await?;
                } else {
                    client.send_broadcast(&broadcast, 0, AdnlSendMethod::Fast).await?;
                }
            }
        }
        Ok(())
    }

    // This function will be sent from the collator in the future.
    // It is part of the separated collator feature.
    #[allow(dead_code)]
    pub async fn send_out_msg_queue_proof_broadcast(
        &self,
        id: &BlockIdExt,
        dst_shard: &ShardIdent,
        state_proof_boc: Vec<u8>,
        queue_proof_boc: Vec<u8>,
        msg_count: u32,
        limits: ImportedMsgQueueLimits,
    ) -> Result<()> {
        if let Some(client) = self.fast_sync_overlay(&dst_shard) {
            log::debug!("Sending out msg queue proof broadcast {} for shard {}...", id, dst_shard);
            let broadcast = OutMsgQueueProofBroadcast {
                dst_shard: dst_shard.into(),
                block: id.clone(),
                limits: ton_api::ton::ton_node::importedmsgqueuelimits::ImportedMsgQueueLimits {
                    max_bytes: limits.max_bytes as i32,
                    max_msgs: limits.max_msgs as i32,
                }
                .into_boxed(),
                proof: ton_api::ton::ton_node::outmsgqueueproof::OutMsgQueueProof {
                    queue_proofs: queue_proof_boc,
                    block_state_proofs: state_proof_boc,
                    msg_counts: vec![msg_count as i32],
                }
                .into_boxed(),
            }
            .into_boxed();
            let broadcast = TaggedByteSlice {
                object: &serialize_boxed(&broadcast)?,
                #[cfg(feature = "telemetry")]
                tag: OutMsgQueueProofBroadcast::constructor_const(),
            };
            client.send_broadcast(&broadcast, 0, AdnlSendMethod::Fast).await?;
        }
        Ok(())
    }

    pub fn delete_overlays(&self) {
        for guard in self.public_overlays.iter() {
            let id = guard.val().overlay_client().id();
            log::info!("Deleting public overlay {}", id);
            match guard.val().overlay_client().delete() {
                Ok(result) => log::info!("Deleted overlay {} ({})", guard.key(), result),
                Err(e) => log::warn!("Deleting overlay {}: {}", guard.key(), e),
            }
        }
        for guard in self.fast_sync_overlays.iter() {
            let id = guard.val().id();
            log::info!("Deleting fast sync overlay {} {}", guard.key(), id);
            guard.val().stop();
        }
    }

    #[cfg(feature = "telemetry")]
    pub fn log_stat(&self) {
        for guard in self.fast_sync_overlays.iter() {
            let client = guard.val();
            let shard = guard.key();
            log::debug!(target: "telemetry", "Fast sync overlay for shard {}, id {}",
                shard, client.id());
            client.client().neighbours().log_neighbors_stat();
        }
        for guard in self.public_overlays.iter() {
            let client = guard.val().overlay_client();
            if client.is_died() {
                continue;
            }
            log::debug!(
                target: "telemetry",
                "Public overlay {}{}, short id {}, full id {}",
                guard.key(),
                if client.is_active() {
                    ""
                } else {
                    " (inactive)"
                },
                client.id(),
                base64_encode(client.id_full()),
            );
            client.neighbours().log_neighbors_stat();
        }
    }

    fn fast_sync_overlay(&self, shard: &ShardIdent) -> Option<Arc<FastSyncOverlayClient>> {
        let shard =
            trim_shard(shard, self.monitor_min_split_for_fast_sync.load(Ordering::Relaxed)).ok()?;
        self.fast_sync_overlays.get(&shard).map(|g| g.val().clone())
    }
}

fn calc_new_shards(old_min_split: u8, new_min_split: u8) -> Result<HashSet<ShardIdent>> {
    let mut new_shards = HashSet::new();
    for len in old_min_split + 1..=new_min_split {
        for prefix in 0..(1 << len) {
            let shard_prefix = if len == 0 { 0 } else { prefix << 64 - len };
            new_shards.insert(ShardIdent::with_prefix_len(len, BASE_WORKCHAIN_ID, shard_prefix)?);
        }
    }
    Ok(new_shards)
}

fn trim_shard(shard: &ShardIdent, min_split: u8) -> Result<ShardIdent> {
    if shard.is_masterchain() {
        Ok(ShardIdent::MASTERCHAIN)
    } else if min_split == 0 || !shard.is_base_workchain() {
        Ok(ShardIdent::full(shard.workchain_id()))
    } else if shard.prefix_len() <= min_split {
        Ok(shard.clone())
    } else {
        let prefix = shard.shard_prefix_without_tag();
        let mask = (1 << (64 - min_split)) - 1;
        let new_prefix = prefix & !mask;
        ShardIdent::with_prefix_len(min_split, shard.workchain_id(), new_prefix)
    }
}

#[cfg(test)]
#[path = "tests/test_full_node_overlays.rs"]
mod tests;
