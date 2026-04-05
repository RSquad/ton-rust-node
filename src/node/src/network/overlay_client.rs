/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
#[cfg(feature = "telemetry")]
use crate::network::telemetry::FullNodeNetworkTelemetry;
use crate::network::{
    neighbours::{
        Neighbour, Neighbours, MAX_NEIGHBOURS, UPDATE_FLAG_IS_REGISTER,
        UPDATE_FLAG_IS_REG_IN_COMMON_STAT, UPDATE_FLAG_IS_RLDP,
    },
    node_network::NetworkContext,
};
use adnl::{
    common::{spawn_cancelable, Subscriber, TaggedByteSlice, TaggedObject, TaggedTlObject},
    node::{AdnlNode, AdnlSendMethod},
    BroadcastSendInfo, DhtNode, DhtSearchPolicy, OverlayId, OverlayNode, OverlayNodeInfo,
    OverlayNodesResolveContext, OverlayNodesSearchContext, OverlayParams, OverlayShortId,
};
use rand::seq::{IteratorRandom, SliceRandom};
use std::{
    collections::VecDeque,
    io::Cursor,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use ton_api::{
    deserialize_typed, serialize_boxed_append,
    ton::{
        overlay::{
            membercertificate::MemberCertificate, node::Node as OverlayNodeV1,
            nodev2::NodeV2 as OverlayNodeV2,
        },
        ton_node::Broadcast,
        validator::Telemetry,
    },
    BoxedDeserialize, BoxedSerialize, Deserializer,
};
use ton_block::{fail, KeyId, KeyOption, Result};

const TIMEOUT_LATENCY_SEC: Duration = Duration::from_secs(1);
const TIMEOUT_STORE_OVERLAY_NODE_SEC: Duration = Duration::from_secs(600);
const ADNL_ATTEMPTS: u32 = 50;
const TIMEOUT_DELTA: u64 = 50; // Milliseconds
const TIMEOUT_NO_NEIGHBOURS: u64 = 1000; // Milliseconds

struct OverlayClientContext {
    id: Arc<OverlayShortId>,
    id_full: OverlayId,
    neighbours_manager: Arc<Neighbours>,
    cancellation_token: tokio_util::sync::CancellationToken,
    is_active: AtomicBool,
    network_context: Arc<NetworkContext>,
    is_semiprivate: bool,
}

impl OverlayClientContext {
    fn dht_node(&self) -> &Arc<DhtNode> {
        &self.network_context.stack.dht
    }
    fn overlay_node(&self) -> &Arc<OverlayNode> {
        &self.network_context.stack.overlay
    }
    #[cfg(feature = "telemetry")]
    fn telemetry(&self) -> &FullNodeNetworkTelemetry {
        &self.network_context.telemetry
    }
}

pub struct OverlayClient {
    ctx: Arc<OverlayClientContext>,
    neighbours_rotation_handle: tokio::task::JoinHandle<()>,
    ping_handle: Option<tokio::task::JoinHandle<()>>,
    resolve_incoming_query_peers_handle: tokio::task::JoinHandle<()>,
    saving_to_dht_handle: Option<tokio::task::JoinHandle<()>>,
}

impl OverlayClient {
    pub async fn new_public(
        id: Arc<OverlayShortId>,
        id_full: OverlayId,
        network_context: Arc<NetworkContext>,
        cancellation_token: tokio_util::sync::CancellationToken,
        policy: DhtSearchPolicy,
        default_rldp_roundtrip: Option<u32>,
    ) -> Result<Arc<Self>> {
        // Add a new overlay to the protocol
        let params = OverlayParams {
            flags: 0,
            hops: network_context.broadcast_hops,
            overlay_id: &id,
            runtime: None,
        };
        network_context.stack.overlay.add_local_workchain_overlay(params)?;

        // Find initial neighbors in the DHT and add them to the overlay protocol
        let mut search_ctx = OverlayNodesSearchContext::with_params(&id, policy.clone())?;
        let initial_peers = find_neighbors_in_dht(
            network_context.stack.dht.clone(),
            &network_context.stack.overlay,
            cancellation_token.clone(),
            &id,
            &mut search_ctx,
        )
        .await?;

        log::trace!("{}: found {} initial peers from DHT", id, initial_peers.len());

        OverlayClient::init(
            id,
            id_full,
            network_context,
            cancellation_token,
            policy,
            default_rldp_roundtrip,
            false,
            initial_peers,
            Some(search_ctx),
        )
        .await
    }

    pub async fn new_semiprivate(
        id: Arc<OverlayShortId>,
        id_full: OverlayId,
        root_members: Vec<Arc<KeyId>>,
        key: Option<&Arc<dyn KeyOption>>,
        certificate: Option<MemberCertificate>,
        network_context: Arc<NetworkContext>,
        cancellation_token: tokio_util::sync::CancellationToken,
        policy: DhtSearchPolicy,
        default_rldp_roundtrip: Option<u32>,
        max_clients: usize,
    ) -> Result<Arc<Self>> {
        // Add a new overlay to the protocol
        let params = OverlayParams::with_id_only(&id);
        network_context.stack.overlay.add_semiprivate_overlay(
            params,
            key,
            &root_members,
            certificate,
            max_clients,
        )?;

        OverlayClient::init(
            id,
            id_full,
            network_context,
            cancellation_token,
            policy,
            default_rldp_roundtrip,
            true,
            root_members,
            None,
        )
        .await
    }

    async fn init(
        id: Arc<OverlayShortId>,
        id_full: OverlayId,
        network_context: Arc<NetworkContext>,
        cancellation_token: tokio_util::sync::CancellationToken,
        policy: DhtSearchPolicy,
        default_rldp_roundtrip: Option<u32>,
        is_semiprivate: bool,
        initial_peers: Vec<Arc<KeyId>>,
        search_ctx: Option<OverlayNodesSearchContext>,
    ) -> Result<Arc<Self>> {
        // Create a neighbors manager
        let neighbours_manager = Arc::new(Neighbours::new(
            &initial_peers,
            &network_context.stack.overlay,
            id.clone(),
            &default_rldp_roundtrip,
            cancellation_token.clone(),
        )?);

        let ctx = Arc::new(OverlayClientContext {
            id: id.clone(),
            id_full,
            network_context,
            neighbours_manager,
            cancellation_token,
            is_active: AtomicBool::new(true),
            is_semiprivate,
        });

        let saving_to_dht_handle =
            if !is_semiprivate { Some(start_saving_to_dht_worker(ctx.clone())?) } else { None };

        // Start a worker that pings active neighbors
        let ping_handle = if !is_semiprivate {
            Some(ctx.neighbours_manager.clone().start_ping_worker())
        } else {
            None
        };

        // Get the neighbors list from the overlay protocol and replace bad active neighbors
        let neighbours_rotation_handle = ctx.neighbours_manager.clone().start_rotation_worker();

        if let Some(search_ctx) = search_ctx {
            // The next worker calls add_neighbors_from_dht until found MAX_NEIGHBOURS
            let _ = start_find_neighbors_in_dht_worker(ctx.clone(), search_ctx);
        }

        // The next worker:
        // - Gets peers from incoming GetRandomPeers queries of the overlay protocol
        // - Resolves them via DHT
        // - Adds resolved peers back to the overlay protocol
        // - Adds peers to the all-neighbors list
        let resolve_incoming_query_peers_handle =
            start_resolve_incoming_query_peers_worker(ctx.clone(), policy);

        Ok(Arc::new(Self {
            ctx,
            neighbours_rotation_handle,
            ping_handle,
            resolve_incoming_query_peers_handle,
            saving_to_dht_handle,
        }))
    }

    pub fn id(&self) -> &OverlayShortId {
        &self.ctx.id
    }

    #[allow(dead_code)]
    pub fn id_full(&self) -> &OverlayId {
        &self.ctx.id_full
    }

    pub fn network_context(&self) -> &Arc<NetworkContext> {
        &self.ctx.network_context
    }

    pub fn neighbours(&self) -> &Arc<Neighbours> {
        &self.ctx.neighbours_manager
    }

    pub fn add_consumer(&self, consumer: Arc<dyn Subscriber>) -> Result<()> {
        let _ = self.ctx.overlay_node().add_consumer(&self.ctx.id, consumer)?;
        Ok(())
    }

    pub fn deactivate(&self) {
        if !self.is_died() && self.ctx.is_active.swap(false, Ordering::Relaxed) {
            let ctx = self.ctx.clone();
            tokio::spawn(async move {
                // Wait while dht announce expires
                tokio::time::sleep(TIMEOUT_STORE_OVERLAY_NODE_SEC + Duration::from_secs(60)).await;
                if !ctx.is_active.load(Ordering::Relaxed) {
                    ctx.cancellation_token.cancel();
                }
            });
        }
    }

    pub fn delete(&self) -> Result<bool> {
        self.ctx.cancellation_token.cancel();
        self.ctx.is_active.store(false, Ordering::Relaxed);
        self.ctx.overlay_node().delete_public_overlay(&self.ctx.id)
    }

    pub fn is_active(&self) -> bool {
        !self.is_died() && self.ctx.is_active.load(Ordering::Relaxed)
    }

    pub fn activate(&self) -> Result<()> {
        self.ctx.is_active.store(true, std::sync::atomic::Ordering::Relaxed);
        if self.is_died() {
            fail!("OverlayClient is already died. Need to recreate it");
        }
        Ok(())
    }

    pub fn is_died(&self) -> bool {
        self.neighbours_rotation_handle.is_finished()
            || self.ping_handle.as_ref().map_or(false, |h| h.is_finished())
            || self.resolve_incoming_query_peers_handle.is_finished()
            || self.saving_to_dht_handle.as_ref().map_or(false, |h| h.is_finished())
    }

    pub async fn broadcast(
        &self,
        data: &TaggedByteSlice<'_>,
        source: Option<&Arc<dyn KeyOption>>,
        flags: u32,
        method: AdnlSendMethod,
    ) -> Result<BroadcastSendInfo> {
        self.ctx.overlay_node().broadcast(&self.ctx.id, data, source, flags, method).await
    }

    pub async fn send_adnl_query_to_peer<D: ton_api::AnyBoxedSerialize>(
        &self,
        peer: &Arc<Neighbour>,
        request: &TaggedTlObject,
        timeout: Option<u64>,
    ) -> Result<Option<D>> {
        let request_str = if log::log_enabled!(log::Level::Trace) || cfg!(feature = "telemetry") {
            format!("ADNL {:?}", request.object)
        } else {
            String::default()
        };
        log::trace!("USE PEER {peer}, {request_str}");

        let now = Instant::now();
        let timeout = timeout.or(Some(AdnlNode::calc_timeout(peer.roundtrip_adnl())));
        let answer =
            self.ctx.overlay_node().query(peer.id(), request, &self.ctx.id, timeout).await?;
        let elapsed = now.elapsed();
        let roundtrip = elapsed.as_millis() as u64;
        let labels = [("peer", peer.id().to_string())];
        metrics::histogram!("ton_node_network_adnl_roundtrip_seconds", &labels).record(elapsed);

        if let Some(answer) = answer {
            match answer.downcast::<D>() {
                Ok(answer) => {
                    peer.query_success(roundtrip, false);
                    #[cfg(feature = "telemetry")]
                    self.ctx.telemetry().consumed_query(request_str, true, now.elapsed(), 0); // TODO data size (need to patch overlay)
                    return Ok(Some(answer));
                }
                Err(obj) => {
                    #[cfg(feature = "telemetry")]
                    self.ctx.telemetry().consumed_query(request_str, false, now.elapsed(), 0);
                    log::warn!("Wrong answer {:?} to {:?} from {}", obj, request.object, peer.id())
                }
            }
        } else {
            #[cfg(feature = "telemetry")]
            self.ctx.telemetry().consumed_query(request_str, false, now.elapsed(), 0);
            log::warn!(
                "No reply to {:?} from {} in overlay {}",
                request.object,
                peer.id(),
                self.ctx.id
            )
        }

        self.ctx.neighbours_manager.update_neighbour_stats(
            peer,
            roundtrip,
            UPDATE_FLAG_IS_REGISTER | UPDATE_FLAG_IS_REG_IN_COMMON_STAT,
        );
        Ok(None)
    }

    // use this function if request size and answer size < 768 bytes (send query via ADNL)
    pub async fn send_adnl_query_to_peer_id<D: ton_api::AnyBoxedSerialize>(
        &self,
        peer: &Arc<KeyId>,
        data: &TaggedTlObject,
        timeout: Option<u64>,
    ) -> Result<(D, Arc<Neighbour>)> {
        let peer = match self.ctx.neighbours_manager.peer(peer) {
            Some(peer) => peer,
            None => {
                if self.ctx.neighbours_manager.add_overlay_peer(peer.clone()) {
                    if let Some(peer) = self.ctx.neighbours_manager.peer(peer) {
                        // add_peer = Some(peer.clone());
                        peer
                    } else {
                        self.ctx.neighbours_manager.new_neighbour(peer.clone())
                    }
                } else {
                    self.ctx.neighbours_manager.new_neighbour(peer.clone())
                }
            }
        };
        match self.send_adnl_query_to_peer::<D>(&peer, data, timeout).await {
            Ok(Some(answer)) => Ok((answer, peer)),
            Ok(None) => {
                fail!("Cannot send query {:?} to peer {}: no reply", data.object, peer.id())
            }
            Err(e) => fail!("Cannot send query {:?} to peer {}: {}", data.object, peer.id(), e),
        }
    }

    // use this function if request size and answer size < 768 bytes (send query via ADNL)
    pub async fn send_adnl_query_to_all_peers<D: ton_api::AnyBoxedSerialize>(
        &self,
        request: &TaggedTlObject,
        timeout: Option<u64>,
        active_peers: Option<&lockfree::set::Set<Arc<KeyId>>>,
        bad_peers: &lockfree::set::Set<Arc<KeyId>>,
        f: impl Fn(&D) -> bool,
    ) -> Result<(D, Arc<Neighbour>)> {
        // first use active peers and add them to neighbour cache
        if let Some(active_peers) = active_peers {
            let peers: Vec<Arc<KeyId>> = active_peers.iter().map(|peer| peer.clone()).collect();
            for peer in peers.iter() {
                match self.send_adnl_query_to_peer_id::<D>(peer, &request, timeout).await {
                    Ok((result, peer)) => {
                        if f(&result) {
                            return Ok((result, peer));
                        }
                    }
                    Err(e) => log::warn!("Bad active peer {} detected: {}", peer, e),
                }
                active_peers.remove(peer);
                bad_peers.insert(peer.clone()).ok();
            }
        }
        // next try to send to all peers
        let mut all_peers = self
            .ctx
            .neighbours_manager
            .all_peers()
            .iter()
            .map(|peer| peer.clone())
            .collect::<Vec<_>>();
        all_peers.shuffle(&mut rand::thread_rng());
        for peer in all_peers.iter() {
            if let Some(active_peers) = active_peers {
                if active_peers.contains(peer) {
                    continue;
                }
            }
            if bad_peers.contains(peer) {
                continue;
            }
            match self.send_adnl_query_to_peer_id::<D>(peer, &request, timeout).await {
                Ok((result, peer)) => {
                    if f(&result) {
                        if let Some(active_peers) = active_peers {
                            active_peers.insert(peer.id().clone()).ok();
                        }
                        return Ok((result, peer));
                    }
                }
                Err(e) => log::warn!("New bad peer {} detected: {}", peer, e),
            }
            bad_peers.insert(peer.clone()).ok();
        }
        fail!("Cannot send query {:?} to all peers", request.object)
    }

    // use this function if request size and answer size < 768 bytes (send query via ADNL)
    pub async fn send_adnl_query<D: ton_api::AnyBoxedSerialize>(
        &self,
        request: &TaggedTlObject,
        attempts: Option<u32>,
        timeout: Option<u64>,
        active_peers: Option<&lockfree::set::Set<Arc<KeyId>>>,
    ) -> Result<(D, Arc<Neighbour>)> {
        let attempts = attempts.unwrap_or(ADNL_ATTEMPTS);
        for _ in 0..attempts {
            let (peer, active) = loop {
                if let Some(ap) = active_peers {
                    if let Some(p) = ap.iter().choose(&mut rand::thread_rng()) {
                        if let Some(n) = self.ctx.neighbours_manager.peer(&p) {
                            break (n, true);
                        }
                    }
                }
                if let Some(n) = self.ctx.neighbours_manager.choose_neighbour()? {
                    break (n, false);
                } else {
                    tokio::time::sleep(Duration::from_millis(TIMEOUT_NO_NEIGHBOURS)).await;
                    fail!(
                        "Neighbour is not found ({} in list) for query {:?}",
                        self.ctx.neighbours_manager.count(),
                        request.object
                    )
                }
            };
            match self.send_adnl_query_to_peer::<D>(&peer, request, timeout).await {
                Err(e) => {
                    if let Some(active_peers) = active_peers {
                        active_peers.remove(peer.id());
                    }
                    return Err(e);
                }
                Ok(Some(answer)) => {
                    if let Some(active_peers) = active_peers {
                        if !active {
                            active_peers.insert(peer.id().clone()).ok();
                        }
                    }
                    return Ok((answer, peer));
                }
                Ok(None) => {
                    if let Some(active_peers) = active_peers {
                        active_peers.remove(peer.id());
                    }
                }
            }
        }
        fail!("No reply to query {:?} in {} attempts", request.object, attempts)
    }

    pub async fn send_rldp_query_raw<T>(
        &self,
        request: &TaggedObject<T>,
        peer: &Arc<Neighbour>,
        attempt: u32,
        v2: bool,
    ) -> Result<Vec<u8>>
    where
        T: BoxedSerialize + std::fmt::Debug,
    {
        let (answer, roundtrip) = self.send_rldp_query(request, peer, attempt, v2).await?;
        peer.query_success(roundtrip, true);
        Ok(answer)
    }

    pub async fn send_rldp_query_typed<T, D>(
        &self,
        request: &TaggedObject<T>,
        peer: &Arc<Neighbour>,
        attempt: u32,
        v2: bool,
    ) -> Result<D>
    where
        T: BoxedSerialize + std::fmt::Debug,
        D: BoxedDeserialize,
    {
        let (answer, roundtrip) = self.send_rldp_query(request, peer, attempt, v2).await?;
        match Deserializer::new(&mut Cursor::new(answer)).read_boxed() {
            Ok(data) => {
                peer.query_success(roundtrip, true);
                Ok(data)
            }
            Err(e) => {
                self.ctx.neighbours_manager.update_neighbour_stats(
                    peer,
                    roundtrip,
                    UPDATE_FLAG_IS_RLDP
                        | UPDATE_FLAG_IS_REGISTER
                        | UPDATE_FLAG_IS_REG_IN_COMMON_STAT,
                );
                fail!(e)
            }
        }
    }

    pub async fn wait_broadcast(&self) -> Result<Option<(Broadcast, Arc<KeyId>)>> {
        loop {
            match self.ctx.overlay_node().wait_for_broadcast(&self.ctx.id).await? {
                Some(info) => {
                    if let Ok(answer) = deserialize_typed::<Broadcast>(&info.data) {
                        return Ok(Some((answer, info.recv_from)));
                    } else if let Ok(_telemetry) = deserialize_typed::<Telemetry>(&info.data) {
                        log::debug!(
                            "Telemetry received from {} in overlay {}, but not supported yet",
                            info.recv_from,
                            self.ctx.id
                        );
                        continue;
                    } else {
                        log::warn!(
                            "Cannot deserialize broadcast from {} in overlay {}",
                            info.recv_from,
                            self.ctx.id
                        );
                        continue;
                    }
                }
                None => return Ok(None),
            }
        }
    }

    async fn send_rldp_query<T>(
        &self,
        request: &TaggedObject<T>,
        peer: &Arc<Neighbour>,
        attempt: u32,
        v2: bool,
    ) -> Result<(Vec<u8>, u64)>
    where
        T: BoxedSerialize + std::fmt::Debug,
    {
        let mut query = self.ctx.overlay_node().get_query_prefix(&self.ctx.id)?;
        serialize_boxed_append(&mut query, &request.object)?;
        let request_str = if log::log_enabled!(log::Level::Trace) || cfg!(feature = "telemetry") {
            std::any::type_name::<T>().to_string()
        } else {
            String::default()
        };
        log::trace!("USE PEER {}, {}", peer, request_str);
        #[cfg(feature = "telemetry")]
        let now = Instant::now();
        let (answer, roundtrip) = self
            .ctx
            .overlay_node()
            .query_via_rldp(
                peer.id(),
                &TaggedByteSlice {
                    object: &query[..],
                    #[cfg(feature = "telemetry")]
                    tag: request.tag,
                },
                &self.ctx.id,
                Some(10 * 1024 * 1024),
                v2,
                peer.roundtrip_rldp().map(|t| t + attempt as u64 * TIMEOUT_DELTA),
            )
            .await?;
        if let Some(answer) = answer {
            #[cfg(feature = "telemetry")]
            self.ctx.telemetry().consumed_query(request_str, true, now.elapsed(), answer.len());
            Ok((answer, roundtrip))
        } else {
            #[cfg(feature = "telemetry")]
            self.ctx.telemetry().consumed_query(request_str, false, now.elapsed(), 0);
            self.ctx.neighbours_manager.update_neighbour_stats(
                peer,
                roundtrip,
                UPDATE_FLAG_IS_RLDP | UPDATE_FLAG_IS_REGISTER | UPDATE_FLAG_IS_REG_IN_COMMON_STAT,
            );
            fail!("No RLDP answer to {:?} from {}", request.object, peer.id())
        }
    }
}

fn start_saving_to_dht_worker(
    ctx: Arc<OverlayClientContext>,
) -> Result<tokio::task::JoinHandle<()>> {
    let OverlayNodeInfo::V1(node) =
        ctx.overlay_node().get_signed_node(&ctx.id, ctx.is_semiprivate)?
    else {
        fail!("Unexpected V2 overlay node in overlay client")
    };
    let ret = spawn_cancelable(ctx.cancellation_token.clone(), async move {
        loop {
            log::debug!("{}: saving_to_dht_worker: saving...", ctx.id);
            match ctx.dht_node().store_overlay_node(&ctx.id_full, &node).await {
                Ok(_res) => log::debug!("{}: saving_to_dht_worker: saved", ctx.id),
                Err(e) => {
                    log::error!("{}: saving_to_dht_worker: error: {e}", ctx.id);
                    tokio::time::sleep(TIMEOUT_LATENCY_SEC).await;
                    continue;
                }
            }
            tokio::time::sleep(TIMEOUT_STORE_OVERLAY_NODE_SEC).await;
        }
    });
    Ok(ret)
}

fn start_find_neighbors_in_dht_worker(
    ctx: Arc<OverlayClientContext>,
    mut ctx_search: OverlayNodesSearchContext,
) -> tokio::task::JoinHandle<()> {
    spawn_cancelable(ctx.cancellation_token.clone(), async move {
        loop {
            if let Err(e) = find_neighbors_in_dht_worker(ctx.clone(), &mut ctx_search).await {
                log::error!("{}: find_neighbors_in_dht_worker error: {e}", ctx.id);
                tokio::time::sleep(TIMEOUT_LATENCY_SEC).await;
            } else {
                break;
            }
        }
    })
}

fn start_resolve_incoming_query_peers_worker(
    ctx: Arc<OverlayClientContext>,
    policy: DhtSearchPolicy,
) -> tokio::task::JoinHandle<()> {
    spawn_cancelable(ctx.cancellation_token.clone(), async move {
        loop {
            if let Err(e) = resolve_incoming_query_peers_worker(ctx.clone(), policy.clone()).await {
                log::error!("{}: resolve_incoming_query_peers_worker error: {e}", ctx.id);
                tokio::time::sleep(TIMEOUT_LATENCY_SEC).await;
            }
        }
    })
}

async fn find_neighbors_in_dht_worker(
    ctx: Arc<OverlayClientContext>,
    ctx_search: &mut OverlayNodesSearchContext,
) -> Result<()> {
    loop {
        let peers = find_neighbors_in_dht(
            ctx.dht_node().clone(),
            ctx.overlay_node(),
            ctx.cancellation_token.clone(),
            &ctx.id,
            ctx_search,
        )
        .await?;
        for peer in peers {
            ctx.neighbours_manager.add(peer)?;
        }
        if ctx.neighbours_manager.count() >= MAX_NEIGHBOURS {
            log::trace!("{}: finish find overlay nodes", ctx.id);
            break;
        }
        tokio::time::sleep(TIMEOUT_LATENCY_SEC).await;
    }
    Ok(())
}

async fn resolve_incoming_query_peers_worker(
    ctx: Arc<OverlayClientContext>,
    policy: DhtSearchPolicy,
) -> Result<()> {
    let mut ctx_resolve = OverlayNodesResolveContext::with_params(policy);
    loop {
        log::trace!("{}: wait for incoming peers...", ctx.id);
        match ctx.overlay_node().wait_for_peers(&ctx.id).await? {
            None => {
                // No peers in queue - wait
                tokio::time::sleep(TIMEOUT_LATENCY_SEC).await;
            }
            Some(peers) => {
                log::trace!("{}: got {} incoming peers", ctx.id, peers.len());
                resolve_peer_ips(&ctx, &mut ctx_resolve, peers).await?;
                tokio::task::yield_now().await;
            }
        }
    }
}

async fn find_neighbors_in_dht(
    dht_node: Arc<DhtNode>,
    overlay_node: &OverlayNode,
    cancellation_token: tokio_util::sync::CancellationToken,
    id: &Arc<OverlayShortId>,
    ctx_search: &mut OverlayNodesSearchContext,
) -> Result<Vec<Arc<KeyId>>> {
    log::debug!("{id}: find_neighbors_in_dht: searching...");
    let peers = tokio::select! {
        peers = dht_node.find_overlay_nodes(ctx_search) => peers,
        _ = cancellation_token.cancelled() => fail!("Overlay {id} node search cancelled")
    }?;
    log::debug!("{id}: find_neighbors_in_dht: found {} peers", peers.len());
    let mut ret = Vec::new();
    for (ip, peer) in peers.iter() {
        if let Some(peer) = overlay_node.add_public_peer(ip, &peer, id)? {
            log::trace!("{id}: find_neighbors_in_dht: peer {peer}, IP {ip}");
            ret.push(peer);
        } else {
            log::trace!("{id}: find_neighbors_in_dht: peer {peer:?}, IP {ip} skipped");
        }
    }
    Ok(ret)
}

async fn resolve_peer_ips(
    ctx: &Arc<OverlayClientContext>,
    ctx_resolve: &mut OverlayNodesResolveContext,
    new_peers: Vec<OverlayNodeInfo<OverlayNodeV1, OverlayNodeV2>>,
) -> Result<()> {
    for peer in new_peers {
        let peer_key: Arc<dyn KeyOption> = peer.id().try_into()?;
        if ctx.neighbours_manager.contains_overlay_peer(peer_key.id()) {
            continue;
        }
        ctx_resolve.add_node(peer, peer_key, true)?;
    }
    let mut postponed = VecDeque::new();
    while let Some(mut peer) = ctx_resolve.next() {
        if ctx.neighbours_manager.contains_overlay_peer(peer.key_id()) {
            continue;
        }
        log::trace!("{}: resolve_peer_ips: searching IP for peer {}...", ctx.id, peer.key_id());
        if let Some(ip) = peer.resolve(ctx.dht_node()).await? {
            if let Err(e) = ctx.overlay_node().add_public_peer(&ip, peer.node(), &ctx.id) {
                log::warn!(
                    "{}: resolve_peer_ips: failed to add peer {}, IP {ip} to overlay: {e}",
                    ctx.id,
                    peer.key_id()
                );
                continue;
            }
            if ctx.neighbours_manager.add_overlay_peer(peer.key_id().clone()) {
                log::trace!("{}: resolve_peer_ips: added peer {}, IP {ip}", ctx.id, peer.key_id());
            } else {
                log::trace!(
                    "{}: resolve_peer_ips: peer {}, IP {ip} already stored",
                    ctx.id,
                    peer.key_id()
                );
            }
        } else {
            log::debug!("{}: resolve_peer_ips: peer {}, IP not found", ctx.id, peer.key_id());
            postponed.push_back(peer);
        }
    }
    ctx_resolve.postpone(&mut postponed);
    Ok(())
}
