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
use crate::{
    config::{ConfigEvent, NodeConfigHandler, NodeConfigSubscriber, TonNodeConfig},
    engine_traits::{EngineAlloc, PrivateOverlayOperations},
    network::catchain_client::CatchainClient,
};
#[cfg(feature = "telemetry")]
use crate::{engine_traits::EngineTelemetry, network::telemetry::FullNodeNetworkTelemetry};
use adnl::{
    common::{add_counted_object_to_map, spawn_cancelable, CountedObject, Counter},
    declare_counted,
    node::AdnlNode,
    AddressSearchContext, DhtNode, DhtSearchPolicy, NetworkStack, OverlayNode, OverlayShortId,
    PrivateOverlayShortId, RldpNode,
};
use catchain::{
    CatchainFactory, CatchainNode, CatchainOverlay, CatchainOverlayListenerPtr,
    CatchainOverlayLogReplayListenerPtr, CatchainOverlayManagerPtr, CatchainOverlayPtr, PrivateKey,
};
use std::{
    hash::Hash,
    net::{Ipv4Addr, SocketAddr},
    sync::{
        atomic::{AtomicI32, Ordering},
        Arc,
    },
    time::Duration,
};
use ton_block::{error, fail, KeyId, KeyOption, Result, UInt256};

const USE_CATCHAIN_ADNL_OVERLAY: bool = true;

pub struct NetworkContext {
    pub stack: Arc<NetworkStack>,
    pub catchain_overlay_manager: CatchainOverlayManagerPtr,
    pub broadcast_hops: Option<u8>,
    #[cfg(feature = "telemetry")]
    pub telemetry: FullNodeNetworkTelemetry,
    #[cfg(feature = "telemetry")]
    pub engine_telemetry: Arc<EngineTelemetry>,
    pub engine_allocated: Arc<EngineAlloc>,
}

pub struct NodeNetwork {
    network_context: Arc<NetworkContext>,
    validator_context: ValidatorContext,
    runtime_handle: tokio::runtime::Handle,
    config_handler: Arc<NodeConfigHandler>,
    default_rldp_roundtrip: Option<u32>,
    cancellation_token: tokio_util::sync::CancellationToken,
}

declare_counted!(
    struct PeerContext {
        count: AtomicI32,
    }
);

declare_counted!(
    struct ValidatorAdnlKeyId {
        key_id: Arc<KeyId>,
    }
);

type ValidatorAdnlKeyIds = lockfree::map::Map<i32, ValidatorAdnlKeyId>;

struct ValidatorContext {
    private_overlays: Arc<lockfree::map::Map<Arc<OverlayShortId>, Arc<CatchainClient>>>,
    catchain_overlays: Arc<lockfree::map::Map<Arc<PrivateOverlayShortId>, CatchainOverlayPtr>>,
    adnl_key_ids: Arc<ValidatorAdnlKeyIds>,
    all_validator_peers: Arc<lockfree::map::Map<Arc<KeyId>, Arc<PeerContext>>>,
    sets_contexts: Arc<lockfree::map::Map<UInt256, Arc<ValidatorSetContext>>>,
    current_set: Arc<lockfree::map::Map<u8, UInt256>>, // zero or one element [0]
}

declare_counted!(
    struct ValidatorSetContext {
        validator_peers: Vec<Arc<KeyId>>,
        validator_key: Arc<dyn KeyOption>,
        validator_adnl_key: Arc<dyn KeyOption>,
        election_id: usize,
    }
);

impl NodeNetwork {
    pub const TAG_DHT_KEY: usize = 1;
    pub const TAG_OVERLAY_KEY: usize = 2;

    const TIMEOUT_FIND_DHT_NODES: Duration = Duration::from_secs(60);
    const TIMEOUT_SEARCH_VALIDATOR_KEYS: Duration = Duration::from_secs(1);
    const TIMEOUT_STORE_IP_ADDRESS_FAIL: Duration = Duration::from_secs(30);
    const TIMEOUT_STORE_IP_ADDRESS_OK: Duration = Duration::from_secs(500);

    pub async fn new(
        config: TonNodeConfig,
        cancellation_token: tokio_util::sync::CancellationToken,
        #[cfg(feature = "telemetry")] engine_telemetry: Arc<EngineTelemetry>,
        engine_allocated: Arc<EngineAlloc>,
    ) -> Result<Arc<Self>> {
        let global_config = config.load_global_config()?;
        let masterchain_zero_state_id = global_config.zero_state()?;
        let broadcast_hops = config.extensions().broadcast_hops;

        let adnl = AdnlNode::with_config(config.adnl_node()?).await?;
        if config.extensions().adnl_compression {
            adnl.set_options(AdnlNode::OPTION_FORCE_COMPRESSION)
        }
        let dht = DhtNode::with_adnl_node(adnl.clone(), Self::TAG_DHT_KEY)?;
        let overlay = OverlayNode::with_params(
            adnl.clone(),
            masterchain_zero_state_id.file_hash.as_slice(),
            Self::TAG_OVERLAY_KEY,
        )?;
        if config.extensions().disable_broadcast_retransmit {
            overlay.set_broadcast_retransmit(false)
        }
        let rldp = RldpNode::with_params(adnl.clone(), vec![overlay.clone()], None)?;
        overlay.set_rldp(rldp.clone())?;

        // Initialize QUIC transport (lazy: no endpoint bound until add_key() is called).
        // Validator ADNL keys are registered when a validator set is activated.
        let quic = {
            let quic = adnl::QuicNode::new(vec![overlay.clone()], cancellation_token.clone());
            overlay.set_quic(quic.clone())?;
            Some(quic)
        };

        let nodes = global_config.dht_nodes()?;
        for peer in nodes.iter() {
            dht.add_peer(peer)?;
        }

        let dht_key = adnl.key_by_tag(Self::TAG_DHT_KEY)?;
        NodeNetwork::periodic_store_ip_addr(dht.clone(), dht_key, None, cancellation_token.clone());

        let overlay_key = adnl.key_by_tag(Self::TAG_OVERLAY_KEY)?;
        NodeNetwork::periodic_store_ip_addr(
            dht.clone(),
            overlay_key,
            None,
            cancellation_token.clone(),
        );

        let default_rldp_roundtrip = config.default_rldp_roundtrip();

        NodeNetwork::find_dht_nodes(dht.clone(), cancellation_token.clone());

        let (config_handler, config_handler_context) =
            NodeConfigHandler::create(config, tokio::runtime::Handle::current())?;

        let stack = Arc::new(NetworkStack { adnl, dht, overlay, rldp, quic });

        //TODO: remove CatchainClient and configure overlay manager to track private peers
        let catchain_tracks_private_peers = false;
        let catchain_overlay_manager = CatchainFactory::create_adnl_overlay_manager(
            tokio::runtime::Handle::current(),
            stack.clone(),
            broadcast_hops,
            catchain_tracks_private_peers,
        )?;

        let validator_context = ValidatorContext {
            private_overlays: Arc::new(lockfree::map::Map::new()),
            catchain_overlays: Arc::new(lockfree::map::Map::new()),
            adnl_key_ids: Arc::new(lockfree::map::Map::new()),
            all_validator_peers: Arc::new(lockfree::map::Map::new()),
            sets_contexts: Arc::new(lockfree::map::Map::new()),
            current_set: Arc::new(lockfree::map::Map::new()),
        };

        let network_context = NetworkContext {
            stack,
            catchain_overlay_manager,
            broadcast_hops,
            #[cfg(feature = "telemetry")]
            telemetry: FullNodeNetworkTelemetry::new_client(),
            #[cfg(feature = "telemetry")]
            engine_telemetry,
            engine_allocated,
        };
        let network_context = Arc::new(network_context);

        let node_network = NodeNetwork {
            network_context: network_context.clone(),
            validator_context,
            runtime_handle: tokio::runtime::Handle::current(),
            config_handler,
            default_rldp_roundtrip,
            cancellation_token,
        };
        let node_network = Arc::new(node_network);

        NodeConfigHandler::start_sheduler(
            node_network.config_handler.clone(),
            config_handler_context,
            vec![node_network.clone()],
        )?;
        Ok(node_network)
    }

    pub async fn start(&self) -> Result<()> {
        log::info!(
            "start network: ip: {}, adnl_id: {}",
            self.network_context.stack.adnl.ip_address(),
            self.network_context.stack.adnl.key_by_tag(Self::TAG_OVERLAY_KEY)?.id()
        );
        self.network_context.stack.start_over_udp_tcp().await?;
        log::info!(
            "network started; TCP support {}",
            if self.network_context.stack.is_tcp_available() { "ON" } else { "OFF" }
        );
        Ok(())
    }

    pub fn context(&self) -> &Arc<NetworkContext> {
        &self.network_context
    }

    pub fn try_get_validator_adnl_key(&self, val_list_id: &UInt256) -> Option<Arc<dyn KeyOption>> {
        self.validator_context
            .sets_contexts
            .get(val_list_id)
            .map(|set_context| set_context.val().validator_adnl_key.clone())
    }

    pub fn cancellation_token(&self) -> &tokio_util::sync::CancellationToken {
        &self.cancellation_token
    }

    pub fn default_rldp_roundtrip(&self) -> Option<u32> {
        self.default_rldp_roundtrip
    }

    pub fn get_key_id_by_tag(&self, tag: usize) -> Result<Arc<KeyId>> {
        let key_id = self.network_context.stack.adnl.key_by_tag(tag)?;
        Ok(key_id.id().clone())
    }

    pub fn config_handler(&self) -> Arc<NodeConfigHandler> {
        self.config_handler.clone()
    }

    pub async fn stop_adnl(&self) {
        log::info!("Stopping node network loops...");
        self.cancellation_token.cancel();
        log::info!("Node network loops stopped. Stopping adnl...");
        self.network_context.stack.adnl.stop().await;
        log::info!("Stopped adnl");
    }

    fn try_add_counted_object<K: Hash + Ord + Clone, T: CountedObject>(
        &self,
        map: &Arc<lockfree::map::Map<K, Arc<T>>>,
        id: &K,
        factory: impl FnMut() -> Result<Arc<T>>,
        msg: String,
    ) -> Result<Arc<T>> {
        add_counted_object_to_map(map, id.clone(), factory)?;
        if let Some(found) = map.get(id) {
            Ok(found.val().clone())
        } else {
            fail!("Cannot add {msg}")
        }
    }

    fn periodic_store_ip_addr(
        dht: Arc<DhtNode>,
        node_key: Arc<dyn KeyOption>,
        validator_info: Option<(Arc<ValidatorAdnlKeyIds>, i32)>,
        cancellation_token: tokio_util::sync::CancellationToken,
    ) {
        spawn_cancelable(cancellation_token, async move {
            let key_id = node_key.id();
            loop {
                let timeout = if let Err(e) = dht.store_ip_address(&node_key).await {
                    log::warn!("store {key_id} IP address ERROR: {e}");
                    Self::TIMEOUT_STORE_IP_ADDRESS_FAIL
                } else {
                    Self::TIMEOUT_STORE_IP_ADDRESS_OK
                };
                tokio::time::sleep(timeout).await;
                if let Some((adnl_key_ids, election_id)) = validator_info.as_ref() {
                    if adnl_key_ids.get(&election_id).is_none() {
                        log::info!("stop storing {key_id} IP address for elections {election_id}");
                        break;
                    }
                }
            }
        });
    }

    fn find_dht_nodes(dht: Arc<DhtNode>, cancellation_token: tokio_util::sync::CancellationToken) {
        spawn_cancelable(cancellation_token, async move {
            loop {
                let mut iter = None;
                while let Some(id) = dht.get_known_peer(&mut iter) {
                    if let Err(e) = dht.find_dht_nodes(&id).await {
                        log::warn!("find_dht_nodes result: {:?}", e)
                    }
                }
                tokio::time::sleep(Self::TIMEOUT_FIND_DHT_NODES).await;
            }
        });
    }

    fn search_validator_keys_for_validator(
        &self,
        local_adnl_id: Arc<KeyId>,
        validators_contexts: Arc<lockfree::map::Map<UInt256, Arc<ValidatorSetContext>>>,
        validator_list_id: UInt256,
        validators: Vec<CatchainNode>,
    ) {
        let dht = self.network_context.stack.dht.clone();
        let adnl = self.network_context.stack.adnl.clone();
        let overlay = self.network_context.stack.overlay.clone();

        spawn_cancelable(self.cancellation_token.clone(), async move {
            let mut current_validators = validators;
            loop {
                match Self::search_validator_keys_round(
                    local_adnl_id.clone(),
                    &adnl,
                    &dht,
                    &overlay,
                    current_validators,
                    None,
                )
                .await
                {
                    Ok(missing_validators) => {
                        current_validators = missing_validators;
                    }
                    Err(e) => {
                        log::warn!("{:?}", e);
                        break;
                    }
                }
                if current_validators.is_empty() {
                    log::info!("search_validator_keys: finished.");
                    break;
                } else {
                    log::info!(
                        "search_validator_keys: {} missing validator keys",
                        current_validators.len()
                    );
                }
                tokio::time::sleep(Self::TIMEOUT_SEARCH_VALIDATOR_KEYS).await;
                if validators_contexts.get(&validator_list_id).is_none() {
                    break;
                }
            }
        });
    }

    async fn search_validator_keys_round<'a>(
        local_adnl_id: Arc<KeyId>,
        adnl: &'a AdnlNode,
        dht: &'a Arc<DhtNode>,
        overlay: &'a OverlayNode,
        validators: Vec<CatchainNode>,
        full_node_callback: Option<Arc<dyn Fn(Arc<KeyId>) + Sync + Send>>,
    ) -> Result<Vec<CatchainNode>> {
        let mut lost_validators = Vec::new();
        for val in validators {
            match dht
                .find_address(&mut AddressSearchContext::with_params(
                    &val.adnl_id,
                    DhtSearchPolicy::default(),
                )?)
                .await
            {
                Ok(Some((ip, key))) => {
                    log::info!("peer {}: found ip: {ip:?}, key: {key:x?}", val.adnl_id);
                    match full_node_callback {
                        Some(ref callback) => {
                            adnl.add_peer(&local_adnl_id, &ip, &Arc::new(key))?;
                            callback(val.adnl_id.clone());
                        }
                        None => {
                            overlay.add_private_peers(&local_adnl_id, vec![(ip, key)])?;
                        }
                    }
                }
                Ok(None) => {
                    log::warn!("find address for {} failed", &val.adnl_id);
                    lost_validators.push(val);
                }
                Err(e) => {
                    log::error!("find address for {} failed: {e:?}", &val.adnl_id);
                    lost_validators.push(val.clone());
                }
            }
        }
        Ok(lost_validators)
    }

    #[cfg(feature = "telemetry")]
    pub fn telemetry(&self) -> &FullNodeNetworkTelemetry {
        &self.network_context.telemetry
    }

    async fn load_and_store_validator_adnl_key(
        &self,
        key_id: Arc<KeyId>,
        election_id: i32,
    ) -> Result<bool> {
        log::info!("load_and_store_validator_adnl_key: id {key_id} elections {election_id}");
        let added =
            add_counted_object_to_map(&self.validator_context.adnl_key_ids, election_id, || {
                Ok(ValidatorAdnlKeyId {
                    key_id: key_id.clone(),
                    counter: self
                        .network_context
                        .engine_allocated
                        .validator_adnl_keys
                        .clone()
                        .into(),
                })
            })?;
        if let Some(key_id_stored) = self.validator_context.adnl_key_ids.get(&election_id) {
            let key_id_stored = &key_id_stored.val().key_id;
            if key_id_stored != &key_id {
                fail!("ADNL key mismatch for election {election_id}: {key_id_stored} vs. {key_id}");
            }
        } else {
            fail!("Cannot get saved ADNL key for election {election_id}");
        };
        if !added {
            log::info!(
                "load_and_store_validator_adnl_key: \
                id {key_id} already added for elections {election_id}"
            );
            return Ok(false);
        }
        let Some((adnl_key, _)) = self.config_handler.get_validator_key(&key_id).await else {
            fail!("Cannot find validator ADNL key {key_id} for election {election_id} in config");
        };
        self.network_context.stack.adnl.add_key(adnl_key.clone(), election_id as usize).map_err(
            |e| error!("Cannot add validator ADNL key {key_id} for election {election_id}: {e}"),
        )?;
        if let Some(quic) = &self.network_context.stack.quic {
            let ip_addr = self.network_context.stack.adnl.ip_address();
            let Some(quic_port) = ip_addr.port().checked_add(adnl::QuicNode::OFFSET_PORT) else {
                log::warn!(
                    "QUIC port overflow for ADNL port {}, skipping QUIC key {key_id}",
                    ip_addr.port()
                );
                return Ok(true);
            };
            let bind_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), quic_port);
            match adnl_key.pvt_key() {
                Ok(pvt_key) => {
                    if let Err(e) = quic.add_key(pvt_key, &key_id, bind_addr) {
                        log::warn!("Cannot add validator ADNL key {key_id} to QUIC: {e}");
                    }
                }
                Err(e) => log::warn!("Cannot get private key for QUIC key {key_id}: {e}"),
            }
        }
        NodeNetwork::periodic_store_ip_addr(
            self.network_context.stack.dht.clone(),
            self.network_context.stack.adnl.key_by_id(&key_id)?,
            Some((self.validator_context.adnl_key_ids.clone(), election_id)),
            self.cancellation_token.clone(),
        );
        log::info!(
            "load_and_store_validator_adnl_key: \
            id {key_id} added for elections {election_id}"
        );
        Ok(true)
    }
}

#[async_trait::async_trait]
impl PrivateOverlayOperations for NodeNetwork {
    async fn set_validator_list(
        &self,
        validator_list_id: UInt256,
        validators: &[CatchainNode],
    ) -> Result<Option<Arc<dyn KeyOption>>> {
        log::trace!("start set_validator_list validator_list_id: {validator_list_id:x}");

        let validator_adnl_key_ids = self.config_handler.get_actual_validator_adnl_key_ids()?;
        let validator_key_ids = self.config_handler.get_actual_validator_key_ids()?;
        let local_validator = validators.iter().find_map(|val| {
            if !validator_adnl_key_ids.contains(&val.adnl_id) {
                return None;
            }
            if !validator_key_ids.contains(&val.public_key.id()) {
                return None;
            }
            Some(val.clone())
        });
        let Some(local_validator) = local_validator else {
            return Ok(None);
        };

        let local_validator_key_raw =
            self.config_handler.get_validator_key(local_validator.public_key.id()).await;
        let (local_validator_key, election_id) =
            local_validator_key_raw.ok_or_else(|| error!("validator key not found!"))?;
        let local_validator_adnl_key =
            match self.network_context.stack.adnl.key_by_id(&local_validator.adnl_id) {
                Ok(adnl_key) => adnl_key,
                Err(e) => {
                    // adnl key isn`t stored in two cases:
                    // 1. First elections. Then make storing adnl key and repeat its load.
                    // 2. Internal error. In this case the error will be returned
                    log::warn!("error load adnl validator key (first attempt): {e}");
                    if !self
                        .load_and_store_validator_adnl_key(
                            local_validator.adnl_id.clone(),
                            election_id,
                        )
                        .await?
                    {
                        fail!("can't load and store adnl key (id: {})", &local_validator.adnl_id);
                    }
                    self.network_context.stack.adnl.key_by_id(&local_validator.adnl_id)?
                }
            };

        let mut peers = Vec::new();
        let mut lost_validators = Vec::new();
        let mut peers_ids = Vec::new();

        for val in validators {
            if val.public_key.id() == local_validator_key.id() {
                continue;
            }
            peers_ids.push(val.adnl_id.clone());
            lost_validators.push(val.clone());
            match self.network_context.stack.dht.fetch_address(&val.adnl_id).await {
                Ok(Some((addr, key))) => {
                    log::info!("addr: {:?}, key: {:x?}", &addr, &key);
                    peers.push((addr, key));
                }
                Ok(None) => {
                    log::info!("addr: {:?} skipped.", &val.adnl_id);
                    lost_validators.push(val.clone());
                }
                Err(e) => {
                    log::error!("find address failed: {:?}", e);
                    lost_validators.push(val.clone());
                }
            }
        }

        self.network_context
            .stack
            .overlay
            .add_private_peers(local_validator_adnl_key.id(), peers)?;

        let context = self.try_add_counted_object(
            &self.validator_context.sets_contexts,
            &validator_list_id,
            || {
                let ret = ValidatorSetContext {
                    validator_peers: peers_ids.clone(),
                    validator_key: local_validator_key.clone(),
                    validator_adnl_key: local_validator_adnl_key.clone(),
                    election_id: election_id as usize,
                    counter: self.network_context.engine_allocated.validator_sets.clone().into(),
                };
                #[cfg(feature = "telemetry")]
                self.network_context.engine_telemetry.validator_sets.update(
                    self.network_context.engine_allocated.validator_sets.load(Ordering::Relaxed),
                );
                Ok(Arc::new(ret))
            },
            format!("vaidator set for validator list id {validator_list_id:x}"),
        )?;

        if !lost_validators.is_empty() {
            self.search_validator_keys_for_validator(
                local_validator_adnl_key.id().clone(),
                //                self.network_context.dht.clone(),
                //                self.network_context.overlay.clone(),
                self.validator_context.sets_contexts.clone(),
                validator_list_id.clone(),
                lost_validators,
            );
        }

        for peer in context.validator_peers.iter() {
            match self.validator_context.all_validator_peers.get(peer) {
                None => {
                    self.try_add_counted_object(
                        &self.validator_context.all_validator_peers,
                        peer,
                        || {
                            let ret = PeerContext {
                                count: AtomicI32::new(0),
                                counter: self
                                    .network_context
                                    .engine_allocated
                                    .validator_peers
                                    .clone()
                                    .into(),
                            };
                            #[cfg(feature = "telemetry")]
                            self.network_context.engine_telemetry.validator_peers.update(
                                self.network_context
                                    .engine_allocated
                                    .validator_peers
                                    .load(Ordering::Relaxed),
                            );
                            Ok(Arc::new(ret))
                        },
                        format!("context for validator peer {peer}"),
                    )?;
                }
                Some(peer_context) => {
                    peer_context.val().count.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        log::trace!("finish set_validator_list validator_list_id: {:x}", validator_list_id);
        Ok(Some(context.validator_key.clone()))
    }

    fn activate_validator_list(&self, validator_list_id: UInt256) -> Result<()> {
        log::trace!("activate_validator_list {:x}", validator_list_id);
        self.validator_context.current_set.insert(0, validator_list_id);
        Ok(())
    }

    fn remove_validator_list(&self, validator_list_id: UInt256) -> Result<bool> {
        let context = self.validator_context.sets_contexts.get(&validator_list_id);
        let mut status = false;
        if let Some(context) = context {
            let adnl_key = &context.val().validator_adnl_key;
            let mut removed_peers = Vec::new();
            let mut removed_peers_from_context = Vec::new();

            for peer in context.val().validator_peers.iter() {
                match self.validator_context.all_validator_peers.get(peer) {
                    None => {
                        removed_peers.push(peer.clone());
                    }
                    Some(peer_context) => {
                        let val = peer_context.val().count.fetch_sub(1, Ordering::Relaxed);
                        if val <= 0 {
                            removed_peers_from_context.push(peer.clone());
                            removed_peers.push(peer.clone());
                        }
                    }
                }
            }
            for peer in removed_peers_from_context.iter() {
                self.validator_context.all_validator_peers.remove(peer);
            }

            self.network_context
                .stack
                .overlay
                .delete_private_peers(adnl_key.id(), &removed_peers)?;
            self.validator_context.sets_contexts.remove(&validator_list_id);
            log::trace!("remove validator list (validator key id: {validator_list_id:x})");
            status = true;
        }

        Ok(status)
    }

    fn create_catchain_client(
        &self,
        validator_list_id: UInt256,
        local_validator_key: &PrivateKey,
        overlay_short_id: &Arc<PrivateOverlayShortId>,
        nodes_public_keys: &[CatchainNode],
        listener: CatchainOverlayListenerPtr,
        _log_replay_listener: CatchainOverlayLogReplayListenerPtr,
        broadcast_hops: Option<u8>,
        transport_type: consensus_common::OverlayTransportType,
    ) -> Result<Arc<dyn CatchainOverlay + Send>> {
        let validator_set_context =
            self.validator_context.sets_contexts.get(&validator_list_id).ok_or_else(|| {
                error!("bad validator_list_id ({})!", validator_list_id.to_hex_string())
            })?;
        let adnl_key =
            self.network_context.stack.adnl.key_by_tag(validator_set_context.val().election_id)?;

        if USE_CATCHAIN_ADNL_OVERLAY {
            let overlay = self.network_context.catchain_overlay_manager.start_overlay(
                &local_validator_key,
                overlay_short_id,
                nodes_public_keys,
                listener,
                _log_replay_listener,
                transport_type,
            )?;

            self.validator_context
                .catchain_overlays
                .insert(overlay_short_id.clone(), overlay.clone());

            return Ok(overlay);
        }

        let client = self.try_add_counted_object(
            &self.validator_context.private_overlays,
            overlay_short_id,
            || {
                let ret = CatchainClient::new(
                    &self.runtime_handle,
                    overlay_short_id,
                    &self.network_context,
                    nodes_public_keys,
                    &adnl_key,
                    validator_set_context.val().validator_key.clone(),
                    listener.clone(),
                    broadcast_hops,
                )?;
                Ok(Arc::new(ret))
            },
            format!("catchain overlay {overlay_short_id}"),
        )?;

        CatchainClient::run_wait_broadcast(
            client.clone(),
            &self.runtime_handle,
            overlay_short_id,
            &self.network_context.stack.overlay,
            client.validator_keys(),
            client.catchain_listener(),
        );
        Ok(client as Arc<dyn CatchainOverlay + Send>)
    }

    fn stop_catchain_client(&self, overlay_short_id: &Arc<PrivateOverlayShortId>) {
        if let Some(catchain_client) =
            self.validator_context.private_overlays.remove(overlay_short_id)
        {
            let client = catchain_client.val().clone();

            self.runtime_handle.spawn(async move {
                client.stop().await;
            });
        } else if let Some(catchain_overlay) =
            self.validator_context.catchain_overlays.get(overlay_short_id)
        {
            let catchain_overlay = catchain_overlay.val();

            // Stop the catchain overlay using the overlay manager
            self.network_context
                .catchain_overlay_manager
                .stop_overlay(overlay_short_id, catchain_overlay);

            // Remove the catchain overlay from the map
            let _catchain_overlay =
                self.validator_context.catchain_overlays.remove(overlay_short_id);
        }
    }
}

#[async_trait::async_trait]
impl NodeConfigSubscriber for NodeNetwork {
    async fn event(&self, sender: ConfigEvent) -> Result<bool> {
        match sender {
            ConfigEvent::AddValidatorAdnlKey(validator_adnl_key_id, election_id) => {
                self.load_and_store_validator_adnl_key(validator_adnl_key_id, election_id).await
            } // Unused
              // ConfigEvent::RemoveValidatorAdnlKey(validator_adnl_key_id, election_id) => {
              //     log::info!("config event (RemoveValidatorAdnlKey) id: {}.", &validator_adnl_key_id);
              //     self.network_context.adnl.delete_key(&validator_adnl_key_id, election_id as usize)?;
              //     let status = self.validator_context.actual_local_adnl_keys.remove(&validator_adnl_key_id).is_some();
              //     log::info!("config event (RemoveValidatorAdnlKey) id: {} finished({}).", &validator_adnl_key_id, &status);
              //     return Ok(status);
              // }
        }
    }
}
