/*
 * Copyright (C) 2019-2023 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
#[cfg(feature = "telemetry")]
use crate::adnl::telemetry::Metric;
use crate::{
    adnl::{
        common::{
            add_counted_object_to_map_with_update, add_unbound_object_to_map, hash, hash_boxed,
            AdnlPeers, CountedObject, Counter, Query, QueryResult, Subscriber, TaggedTlObject,
            Version, Wait,
        },
        node::{
            AddressCache, AddressCacheIterator, AddressCacheWithBads, AdnlNode, BadPolicy,
            IpAddress,
        },
    },
    declare_counted,
    overlay::{OverlayId, OverlayShortId, OverlayUtils},
    OverlayNodeInfo,
};
use rand::Rng;
use std::{
    collections::VecDeque,
    convert::TryInto,
    fmt::{self, Display, Formatter},
    sync::{atomic::AtomicU64, Arc},
};
#[cfg(feature = "telemetry")]
use std::{sync::atomic::Ordering, time::Instant};
use ton_api::{
    deserialize_boxed, serialize_boxed, serialize_boxed_inplace,
    ton::{
        adnl::AddressList as AddressListBoxed,
        dht::{
            key::Key as DhtKey,
            keydescription::KeyDescription as DhtKeyDescription,
            node::Node,
            nodes::Nodes,
            pong::Pong as DhtPong,
            value::Value as DhtValue,
            valueresult::{ValueFound, ValueNotFound},
            Node as NodeBoxed, Nodes as NodesBoxed, Pong as DhtPongBoxed, Stored, UpdateRule,
            ValueResult as DhtValueResult,
        },
        overlay::{
            node::Node as OverlayNodeV1, nodes::Nodes as OverlayNodesV1,
            nodev2::NodeV2 as OverlayNodeV2, Nodes as OverlayNodesV1Boxed,
        },
        pub_::publickey::Overlay,
        rpc::dht::{
            FindNode, FindValue, GetSignedAddressList, Ping as DhtPing, Query as DhtQuery, Store,
        },
        PublicKey,
    },
    AnyBoxedSerialize, IntoBoxed, Signing, TLObject,
};
use ton_block::{base64_encode, error, fail, KeyId, KeyOption, Result, UInt256};

pub const TARGET: &str = "dht";

pub struct DhtIterator {
    iter: Option<AddressCacheIterator>,
    key_id: Arc<DhtKeyId>,
    order: Vec<(u8, Arc<KeyId>)>,
}

impl DhtIterator {
    fn with_key_id(dht: &DhtNetwork, key_id: Arc<DhtKeyId>) -> Self {
        let mut ret = Self { iter: None, key_id, order: Vec::new() };
        ret.update(dht);
        ret
    }

    fn update(&mut self, dht: &DhtNetwork) {
        let mut next = if let Some(iter) = &self.iter {
            dht.known_peers.given(iter)
        } else {
            dht.known_peers.next(&mut self.iter)
        };
        while let Some(peer) = next {
            let mut affinity = DhtNode::get_affinity(peer.data(), &self.key_id);
            if let Some(score) = dht.known_peers.score(&peer) {
                let new_affinity = affinity.saturating_sub(score);
                log::debug!(
                    target: TARGET,
                    "Bad DHT peer {}, score {} affinity {} -> {}",
                    peer, score, affinity, new_affinity
                );
                affinity = new_affinity;
            }
            let add = if let Some((top_affinity, _)) = self.order.last() {
                (*top_affinity <= affinity) || (self.order.len() < DhtNode::MAX_TASKS as usize)
            } else {
                true
            };
            if add {
                self.order.push((affinity, peer))
            }
            next = dht.known_peers.next(&mut self.iter)
        }
        self.order.sort_unstable_by_key(|(affinity, _)| *affinity);
        if let Some((top_affinity, _)) = self.order.last() {
            let mut drop_to = 0;
            while self.order.len() - drop_to > DhtNode::MAX_TASKS as usize {
                let (affinity, _) = self.order[drop_to];
                if affinity < *top_affinity {
                    drop_to += 1
                } else {
                    break;
                }
            }
            self.order.drain(0..drop_to);
        }
        if log::log_enabled!(log::Level::Debug) {
            let mut out = format!("DHT search list for {}:\n", base64_encode(&self.key_id[..]));
            for (affinity, key_id) in self.order.iter().rev() {
                out.push_str(format!("order {} - {}\n", affinity, key_id).as_str())
            }
            log::debug!(target: TARGET, "{}", out);
        }
    }
}

impl Display for DhtIterator {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        if let Some(iter) = &self.iter {
            write!(f, "{} DHT peer(s) selected of {:?}", self.order.len(), iter)
        } else {
            write!(f, "no DHT peers yet")
        }
    }
}

type DhtKeyId = [u8; 32];

struct DhtKeyIdDumper {
    dump: Option<String>,
}

impl DhtKeyIdDumper {
    fn with_params(level: log::Level, src: &DhtKeyId) -> Self {
        let dump = if log::log_enabled!(level) { Some(base64_encode(src)) } else { None };
        Self { dump }
    }
}

impl Display for DhtKeyIdDumper {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if let Some(dump) = &self.dump {
            write!(f, "{}", dump)
        } else {
            fmt::Result::Ok(())
        }
    }
}

declare_counted!(
    struct NodeObject {
        object: Node,
    }
);

declare_counted!(
    struct ValueObject {
        object: DhtValue,
    }
);

#[derive(Clone)]
pub enum DhtSearchPolicy {
    FastSearch(u8), // Parameter: concurrency level
    FullSearch(u8), // Parameter: concurrency level
}

impl Default for DhtSearchPolicy {
    fn default() -> Self {
        Self::FullSearch(DhtNode::MAX_TASKS)
    }
}

#[cfg(feature = "telemetry")]
struct DhtTelemetry {
    peers: Arc<Metric>,
    values: Arc<Metric>,
}

struct DhtAlloc {
    peers: Arc<AtomicU64>,
    values: Arc<AtomicU64>,
}

pub struct AddressSearchContext {
    iter: Option<DhtIterator>,
    key_id: Arc<DhtKeyId>,
    policy: DhtSearchPolicy,
}

impl AddressSearchContext {
    pub fn with_params(key_id: &Arc<KeyId>, policy: DhtSearchPolicy) -> Result<Self> {
        let ret = Self {
            iter: None,
            key_id: Arc::new(hash(DhtNode::dht_key_from_key_id(key_id, "address"))?),
            policy,
        };
        Ok(ret)
    }

    pub fn can_iterate(&self) -> bool {
        self.iter.is_some()
    }
}

pub struct OverlayNodeResolveContext {
    node: OverlayNodeInfo<OverlayNodeV1, OverlayNodeV2>,
    key: Arc<dyn KeyOption>,
    search: AddressSearchContext,
}

impl OverlayNodeResolveContext {
    pub fn key_id(&self) -> &Arc<KeyId> {
        self.key.id()
    }

    pub fn node(&self) -> &OverlayNodeInfo<OverlayNodeV1, OverlayNodeV2> {
        &self.node
    }

    pub async fn resolve(&mut self, dht: &Arc<DhtNode>) -> Result<Option<IpAddress>> {
        Ok(dht.find_address(&mut self.search).await?.map(|(ip, _, _)| ip))
    }
}

pub struct OverlayNodesResolveContext {
    policy: DhtSearchPolicy,
    search: VecDeque<OverlayNodeResolveContext>,
    stored: AddressCache,
}

impl OverlayNodesResolveContext {
    pub fn with_params(policy: DhtSearchPolicy) -> Self {
        Self {
            policy,
            search: VecDeque::new(),
            stored: AddressCache::with_limit(DhtNode::MAX_PEERS),
        }
    }

    pub fn add_node(
        &mut self,
        node: OverlayNodeInfo<OverlayNodeV1, OverlayNodeV2>,
        key: Arc<dyn KeyOption>,
        store: bool,
    ) -> Result<bool> {
        if store {
            if !self.stored.put(key.id().clone())? {
                return Ok(false);
            }
        } else {
            if self.stored.contains(key.id()) {
                return Ok(false);
            }
        }
        self.search.push_back(OverlayNodeResolveContext {
            node,
            search: AddressSearchContext::with_params(key.id(), self.policy.clone())?,
            key,
        });
        Ok(true)
    }

    pub fn next(&mut self) -> Option<OverlayNodeResolveContext> {
        self.search.pop_front()
    }

    pub fn postpone(&mut self, postponed: &mut VecDeque<OverlayNodeResolveContext>) {
        self.search.append(postponed)
    }
}

pub struct OverlayNodesSearchContext {
    iter: Option<DhtIterator>,
    key_id: Arc<DhtKeyId>,
    resolve: OverlayNodesResolveContext,
}

impl OverlayNodesSearchContext {
    pub fn with_params(overlay_id: &Arc<OverlayShortId>, policy: DhtSearchPolicy) -> Result<Self> {
        let ret = Self {
            iter: None,
            key_id: Arc::new(hash(DhtNode::dht_key_from_key_id(overlay_id, "nodes"))?),
            resolve: OverlayNodesResolveContext::with_params(policy),
        };
        Ok(ret)
    }

    pub fn can_iterate(&self) -> bool {
        self.iter.is_some()
    }
}

struct DhtNetwork {
    buckets: lockfree::map::Map<u8, lockfree::map::Map<Arc<KeyId>, NodeObject>>,
    known_peers: AddressCacheWithBads,
    node_key: Arc<dyn KeyOption>,
    query_prefix: Vec<u8>,
    storage: lockfree::map::Map<DhtKeyId, ValueObject>,
}

impl DhtNetwork {
    pub fn get_known_nodes(&self, limit: usize) -> Result<Vec<Node>> {
        if limit == 0 {
            fail!("It is useless to ask for zero known nodes")
        }
        let mut ret = Vec::new();
        for i in 0..=255 {
            if let Some(bucket) = self.buckets.get(&i) {
                for node in bucket.val().iter() {
                    ret.push(node.val().object.clone());
                    if ret.len() == limit {
                        return Ok(ret);
                    }
                }
            }
        }
        Ok(ret)
    }

    fn search_dht_key(&self, key: &DhtKeyId) -> Option<DhtValue> {
        let version = Version::get();
        if let Some(value) = self.storage.get(key) {
            if value.val().object.ttl > version {
                Some(value.val().object.clone())
            } else {
                None
            }
        } else {
            None
        }
    }

    fn set_good_peer(&self, peer: &Arc<KeyId>, elapsed_sec: u32) {
        if let Some(score) = self.known_peers.amnesty(peer, elapsed_sec) {
            log::info!(target: TARGET, "Make DHT peer {peer} feel good: {score}");
        }
    }

    fn set_query_result(
        &self,
        result: Option<TLObject>,
        peer: &Arc<KeyId>,
        elapsed_sec: u32,
    ) -> Result<Option<TLObject>> {
        if result.is_some() {
            self.set_good_peer(peer, elapsed_sec)
        } else {
            if let Some(score) = self.known_peers.penalty(peer, elapsed_sec)? {
                log::info!(target: TARGET, "Make DHT peer {peer} feel bad: {score}");
            }
        }
        Ok(result)
    }
}

/// DHT Node
pub struct DhtNode {
    adnl: Arc<AdnlNode>,
    network: Arc<DhtNetwork>,
    #[cfg(feature = "telemetry")]
    telemetry: DhtTelemetry,
    allocated: DhtAlloc,
}

impl DhtNode {
    const BITS: [u8; 16] = [4, 3, 2, 2, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0];

    const BLOCK_LATENCY_SEC: u32 = 10;
    const MAX_FAIL_COUNT: u8 = 5;
    const MAX_PEERS: u32 = 65536;
    const MAX_TASKS: u8 = 5;
    const TIMEOUT_VALUE: i32 = 600; // Seconds

    /// Constructor
    pub fn with_adnl_node(adnl: Arc<AdnlNode>, key_tag: usize) -> Result<Arc<Self>> {
        let node_key = adnl.key_by_tag(key_tag)?;
        #[cfg(feature = "telemetry")]
        let telemetry = DhtTelemetry {
            peers: adnl.add_metric("Alloc DHT peers"),
            values: adnl.add_metric("Alloc DHT values"),
        };
        let allocated =
            DhtAlloc { peers: Arc::new(AtomicU64::new(0)), values: Arc::new(AtomicU64::new(0)) };
        let network = Self::create_network(node_key.clone(), &adnl)?;
        let ret = Self {
            adnl,
            network,
            #[cfg(feature = "telemetry")]
            telemetry,
            allocated,
        };
        Ok(Arc::new(ret))
    }

    /// Add DHT peer                
    pub fn add_peer(&self, peer: &Node) -> Result<Option<Arc<KeyId>>> {
        self.add_peer_to_dht_network(&self.network, peer)
    }

    /// Fetch address of node (locally) with given key ID
    pub async fn fetch_address(
        &self,
        key_id: &Arc<KeyId>,
    ) -> Result<Option<(IpAddress, Option<IpAddress>, Arc<dyn KeyOption>)>> {
        let key = Self::dht_key_from_key_id(key_id, "address");
        let value = self.network.search_dht_key(&hash(key)?);
        if let Some(value) = value {
            let object = deserialize_boxed(&value.value)?;
            Ok(Some(Self::parse_value_as_address(value.key, object)?))
        } else {
            Ok(None)
        }
    }

    /// Find address of node with given key ID, keeping search context
    pub async fn find_address(
        self: &Arc<Self>,
        ctx_search: &mut AddressSearchContext,
    ) -> Result<Option<(IpAddress, Option<IpAddress>, Arc<dyn KeyOption>)>> {
        let mut addr_list = self
            .find_value(
                &ctx_search.key_id,
                |object| object.is::<AddressListBoxed>(),
                &ctx_search.policy,
                false,
                &mut ctx_search.iter,
            )
            .await?;
        if let Some((key, addr_list)) = addr_list.pop() {
            Ok(Some(Self::parse_value_as_address(key, addr_list)?))
        } else {
            Ok(None)
        }
    }

    /// Find DHT nodes
    pub async fn find_dht_nodes(&self, dst: &Arc<KeyId>) -> Result<bool> {
        let query =
            FindNode { key: UInt256::with_array(*self.network.node_key.id().data()), k: 10 }
                .into_tl_object()
                .into();
        let answer = self.query_with_prefix(&self.network, dst, &query).await?;
        let answer: NodesBoxed = if let Some(answer) = answer {
            Query::parse(answer, &query.object)?
        } else {
            return Ok(false);
        };
        let src = answer.only().nodes;
        log::debug!(target: TARGET, "-------- Found DHT nodes:");
        for node in src.iter() {
            log::debug!(target: TARGET, "{:?}", node);
            self.add_peer_to_dht_network(&self.network, node)?;
        }
        Ok(true)
    }

    /// Get nodes of overlay with given ID, keeping search context
    pub async fn find_overlay_nodes(
        self: &Arc<Self>,
        ctx_search: &mut OverlayNodesSearchContext,
    ) -> Result<Vec<(IpAddress, OverlayNodeInfo<OverlayNodeV1, OverlayNodeV2>)>> {
        let mut ret = Vec::new();
        log::debug!(
            target: TARGET,
            "-------- Overlay nodes search, {}",
            if let Some(iter) = &ctx_search.iter {
                iter.to_string()
            } else {
                format!("{} DHT peer(s) to query", self.network.known_peers.all().count())
            }
        );
        let mut postponed = VecDeque::new();
        loop {
            if ctx_search.resolve.search.is_empty() {
                let mut nodes_lists = self
                    .find_value(
                        &ctx_search.key_id,
                        |object| object.is::<OverlayNodesV1Boxed>(),
                        &ctx_search.resolve.policy,
                        true,
                        &mut ctx_search.iter,
                    )
                    .await?;
                if nodes_lists.is_empty() {
                    // No more results
                    break;
                }
                while let Some((_, nodes_list)) = nodes_lists.pop() {
                    if let Ok(nodes_list) = nodes_list.downcast::<OverlayNodesV1Boxed>() {
                        for node in nodes_list.only().nodes {
                            let key: Arc<dyn KeyOption> = (&node.id).try_into()?;
                            let node_v1 = OverlayNodeInfo::V1(node);
                            ctx_search.resolve.add_node(node_v1, key, false)?;
                        }
                    } else {
                        fail!("INTERNAL ERROR: overlay nodes list type mismatch in search")
                    }
                }
                ctx_search.resolve.search.append(&mut postponed);
            }
            let (wait, mut queue_reader) = Wait::new();
            log::debug!(
                target: TARGET,
                "-------- Overlay nodes search, {} ({} suspicious) nodes to resolve",
                ctx_search.resolve.search.len() + postponed.len(),
                postponed.len()
            );
            let limit = match &ctx_search.resolve.policy {
                DhtSearchPolicy::FastSearch(_) => 1,
                DhtSearchPolicy::FullSearch(limit) => *limit,
            };
            while let Some(mut ctx_resolve) = ctx_search.resolve.next() {
                if ctx_search.resolve.stored.contains(ctx_resolve.key_id()) {
                    log::trace!(
                        target: TARGET,
                        "-------- Overlay nodes search, node {} already stored",
                        ctx_resolve.key_id()
                    );
                    continue;
                }
                let dht = self.clone();
                let wait = wait.clone();
                let reqs = wait.request_immediate();
                tokio::spawn(async move {
                    log::trace!(
                        target: TARGET,
                        "-------- Overlay nodes search, try resolve node {}",
                        ctx_resolve.key_id()
                    );
                    match ctx_resolve.resolve(&dht).await {
                        Ok(Some(ip)) => {
                            log::debug!(
                                target: TARGET,
                                "-------- Overlay nodes search, resolved {} IP: {ip}",
                                ctx_resolve.key_id()
                            );
                            wait.respond(Some((Some(ip), ctx_resolve)))
                        }
                        Ok(None) => {
                            log::trace!(
                                target: TARGET,
                                "-------- Overlay nodes search, {} not resolved",
                                ctx_resolve.key_id()
                            );
                            wait.respond(Some((None, ctx_resolve)))
                        }
                        Err(e) => {
                            log::debug!(
                                target: TARGET,
                                "-------- Overlay nodes search, cannot resolve {}: {e}",
                                ctx_resolve.key_id()
                            );
                            wait.respond(Some((None, ctx_resolve)))
                        }
                    }
                });
                if reqs >= limit as usize {
                    break;
                }
            }
            loop {
                match wait.wait(&mut queue_reader, false).await {
                    Some(Some((None, ctx_resolve))) => match &ctx_search.resolve.policy {
                        DhtSearchPolicy::FastSearch(_) => (),
                        DhtSearchPolicy::FullSearch(_) => postponed.push_back(ctx_resolve),
                    },
                    Some(Some((Some(ip), ctx_resolve))) => {
                        if ctx_search.resolve.stored.put(ctx_resolve.key_id().clone())? {
                            ret.push((ip, ctx_resolve.node));
                        }
                    }
                    _ => break,
                }
            }
            log::debug!(
                target: TARGET,
                "-------- Overlay nodes search, so far resolved {} nodes",
                ret.len()
            );
            if !ret.is_empty() {
                // Found some
                break;
            }
            if !ctx_search.can_iterate() {
                // Search is over
                break;
            }
        }
        ctx_search.resolve.postpone(&mut postponed);
        log::debug!(
            target: TARGET,
            "-------- Overlay nodes search, {} nodes yet to resolve",
            ctx_search.resolve.search.len()
        );
        Ok(ret)
    }

    /// Get DHT peer via iterator
    pub fn get_known_peer(&self, iter: &mut Option<AddressCacheIterator>) -> Option<Arc<KeyId>> {
        self.network.known_peers.next(iter)
    }

    /// Get known DHT nodes
    pub fn get_known_nodes(&self, limit: usize) -> Result<Vec<Node>> {
        self.network.get_known_nodes(limit)
    }

    /// Get signed address list
    pub async fn get_signed_address_list(&self, dst: &Arc<KeyId>) -> Result<bool> {
        let query = GetSignedAddressList.into_tl_object().into();
        let answer = self.query_with_prefix(&self.network, dst, &query).await?;
        let answer: NodeBoxed = if let Some(answer) = answer {
            Query::parse(answer, &query.object)?
        } else {
            return Ok(false);
        };
        self.add_peer_to_dht_network(&self.network, &answer.only())?;
        Ok(true)
    }

    /// Get signed node
    pub fn get_signed_node(&self) -> Result<Node> {
        self.sign_local_node(&self.network)
    }

    /// Node IP address
    pub fn ip_address(&self) -> &IpAddress {
        self.adnl.ip_address_adnl()
    }

    /// Node key
    pub fn key(&self) -> &Arc<dyn KeyOption> {
        &self.network.node_key
    }

    /// Ping
    pub async fn ping(&self, dst: &Arc<KeyId>) -> Result<bool> {
        let random_id = rand::thread_rng().gen();
        let query = DhtPing { random_id }.into_tl_object().into();
        let answer = self.query(&self.network, dst, &query).await?;
        let answer: DhtPongBoxed = if let Some(answer) = answer {
            Query::parse(answer, &query.object)?
        } else {
            return Ok(false);
        };
        Ok(answer.random_id() == &random_id)
    }

    /// Store own IP address
    pub async fn store_ip_address(self: &Arc<Self>, key: &Arc<dyn KeyOption>) -> Result<bool> {
        log::debug!(target: TARGET, "Storing key ID {}", key.id());
        let addr_list = self.adnl.build_address_list(None)?;
        let addrs = AdnlNode::parse_address_list(&addr_list)?
            .ok_or_else(|| error!("INTERNAL ERROR: cannot parse generated address list"))?;
        let value = serialize_boxed(&addr_list.into_boxed())?;
        let value = Self::sign_value("address", value, key)?;
        let key = Self::dht_key_from_key_id(key.id(), "address");
        let key_id = hash(key.clone())?;
        log::debug!(target: TARGET, "Storing DHT key ID {}", base64_encode(&key_id[..]));
        self.process_store_signed_value(&self.network, key_id, value.clone())?;
        self.store_value(
            key,
            value,
            |object| object.is::<AddressListBoxed>(),
            false,
            |mut objects| {
                while let Some((_, object)) = objects.pop() {
                    if let Ok(addr_list) = object.downcast::<AddressListBoxed>() {
                        let addr_list = addr_list.only();
                        if let Some(stored) = AdnlNode::parse_address_list(&addr_list)? {
                            if stored == addrs {
                                log::debug!(target: TARGET, "Checked stored address {stored:?}");
                                return Ok(true);
                            } else {
                                log::warn!(
                                    target: TARGET,
                                    "Found another stored address {stored:?}, expected {addrs:?}"
                                )
                            }
                        } else {
                            log::warn!(
                                target: TARGET,
                                "Found some wrong address list {:?}",
                                addr_list
                            )
                        }
                    } else {
                        fail!("INTERNAL ERROR: address list type mismatch in store")
                    }
                }
                Ok(false)
            },
        )
        .await
    }

    /// Store own overlay node
    pub async fn store_overlay_node(
        self: &Arc<Self>,
        overlay_id: &OverlayId,
        node_v1: &OverlayNodeV1,
    ) -> Result<bool> {
        log::debug!(target: TARGET, "Storing overlay node {:?}", node_v1);
        let overlay_id = Overlay { name: overlay_id.to_vec().into() };
        let overlay_short_id = OverlayShortId::from_data(hash(overlay_id.clone())?);
        OverlayUtils::verify_node(&overlay_short_id, node_v1)?;
        let nodes = OverlayNodesV1 { nodes: vec![node_v1.clone()].into() }.into_boxed();
        let key = Self::dht_key_from_key_id(&overlay_short_id, "nodes");
        let value = DhtValue {
            key: DhtKeyDescription {
                id: overlay_id.into_boxed(),
                key: key.clone(),
                signature: Default::default(),
                update_rule: UpdateRule::Dht_UpdateRule_OverlayNodes,
            },
            ttl: Version::get() + Self::TIMEOUT_VALUE,
            signature: Default::default(),
            value: serialize_boxed(&nodes)?.into(),
        };
        self.process_store_overlay_nodes(&self.network, hash(key.clone())?, value.clone())?;
        self.store_value(
            key,
            value,
            |object| object.is::<OverlayNodesV1Boxed>(),
            true,
            |mut objects| {
                while let Some((_, object)) = objects.pop() {
                    if let Ok(nodes_list) = object.downcast::<OverlayNodesV1Boxed>() {
                        for found_node in nodes_list.only().nodes {
                            if &found_node == node_v1 {
                                log::debug!(target: TARGET, "Checked stored node {:?}", node_v1);
                                return Ok(true);
                            }
                        }
                    } else {
                        fail!("INTERNAL ERROR: overlay nodes list type mismatch in store")
                    }
                }
                Ok(false)
            },
        )
        .await
    }

    fn add_peer_to_dht_network(
        &self,
        network: &Arc<DhtNetwork>,
        peer: &Node,
    ) -> Result<Option<Arc<KeyId>>> {
        if let Err(e) = DhtNode::verify_other_node(peer) {
            log::warn!(target: TARGET, "Error when verifying DHT peer: {}", e);
            return Ok(None);
        }
        let (adnl_addr, quic_addr) =
            if let Some(addrs) = AdnlNode::parse_address_list(&peer.addr_list)? {
                addrs
            } else {
                log::warn!(target: TARGET, "Wrong DHT peer address {:?}", peer.addr_list);
                return Ok(None);
            };
        let peer_key: Arc<dyn KeyOption> = (&peer.id).try_into()?;
        let ret =
            self.adnl.add_peer(network.node_key.id(), &adnl_addr, quic_addr.as_ref(), &peer_key)?;
        let ret = if let Some(ret) = ret { ret } else { return Ok(None) };
        if network.known_peers.all().put(ret.clone())? {
            let key1 = network.node_key.id().data();
            let key2 = ret.data();
            let affinity = DhtNode::get_affinity(key1, key2);
            add_unbound_object_to_map(
                &network.buckets,
                affinity,
                || Ok(lockfree::map::Map::new()),
            )?;
            if let Some(bucket) = network.buckets.get(&affinity) {
                add_counted_object_to_map_with_update(bucket.val(), ret.clone(), |old_node| {
                    if let Some(old_node) = old_node {
                        if old_node.object.version >= peer.version {
                            return Ok(None);
                        }
                    }
                    let ret = NodeObject {
                        object: peer.clone(),
                        counter: self.allocated.peers.clone().into(),
                    };
                    #[cfg(feature = "telemetry")]
                    self.telemetry.peers.update(self.allocated.peers.load(Ordering::Relaxed));
                    Ok(Some(ret))
                })?;
            }
        } else {
            network.set_good_peer(&ret, self.adnl.elapsed_sec())
        }
        Ok(Some(ret))
    }

    fn create_network(
        main_key: Arc<dyn KeyOption>,
        adnl: &Arc<AdnlNode>,
    ) -> Result<Arc<DhtNetwork>> {
        let node_key = main_key;
        let local_node = Node {
            id: (&node_key).try_into()?,
            addr_list: adnl.build_address_list(None)?,
            signature: Default::default(),
            version: Version::get(),
        };
        let node = local_node.sign(&node_key)?;
        let policy = BadPolicy {
            amnesty: 1,
            latency: Self::BLOCK_LATENCY_SEC,
            penalty: 2,
            to_block: Self::MAX_FAIL_COUNT,
        };
        let mut ret = DhtNetwork {
            buckets: lockfree::map::Map::new(),
            known_peers: AddressCacheWithBads::with_params(Self::MAX_PEERS, policy),
            node_key,
            query_prefix: Vec::new(),
            storage: lockfree::map::Map::new(),
        };
        let query = DhtQuery { node };
        serialize_boxed_inplace(&mut ret.query_prefix, &query)?;
        Ok(Arc::new(ret))
    }

    fn deserialize_overlay_nodes(value: &[u8]) -> Result<Vec<OverlayNodeV1>> {
        let nodes = deserialize_boxed(value)?
            .downcast::<OverlayNodesV1Boxed>()
            .map_err(|object| error!("Wrong OverlayNodes: {:?}", object))?;
        Ok(nodes.only().nodes)
    }

    fn dht_key_from_key_id(id: &Arc<KeyId>, name: &str) -> DhtKey {
        DhtKey {
            id: UInt256::with_array(*id.data()),
            idx: 0,
            name: name.as_bytes().to_vec().into(),
        }
    }

    async fn find_value(
        self: &Arc<Self>,
        key_id: &Arc<DhtKeyId>,
        check: impl Fn(&TLObject) -> bool + Copy + Send + 'static,
        policy: &DhtSearchPolicy,
        all: bool,
        iter_opt: &mut Option<DhtIterator>,
    ) -> Result<Vec<(DhtKeyDescription, TLObject)>> {
        let iter =
            iter_opt.get_or_insert_with(|| DhtIterator::with_key_id(&self.network, key_id.clone()));
        if &iter.key_id != key_id {
            fail!("INTERNAL ERROR: DHT key mismatch in value search")
        }
        let mut ret = Vec::new();
        let query: TaggedTlObject =
            FindValue { key: UInt256::from_slice(&key_id[..]), k: 6 }.into_tl_object().into();
        let key_dumper = DhtKeyIdDumper::with_params(log::Level::Debug, key_id);
        let query = Arc::new(query);
        let (wait, mut queue_reader) = Wait::new();
        let mut known_peers = self.network.known_peers.all().count();
        log::debug!(
            target: TARGET,
            "FindValue with DHT key ID {} query, {}",
            key_dumper, iter
        );
        let limit = match &policy {
            DhtSearchPolicy::FastSearch(limit) => *limit,
            DhtSearchPolicy::FullSearch(limit) => *limit,
        } as usize;
        loop {
            while let Some((_, peer)) = iter.order.pop() {
                let dht = self.clone();
                let key_id = key_id.clone();
                let peer = peer.clone();
                let query = query.clone();
                let wait = wait.clone();
                let reqs = wait.request_immediate();
                tokio::spawn(async move {
                    match dht.value_query(&dht.network, &peer, &query, &key_id, check).await {
                        Ok(found) => wait.respond(found),
                        Err(e) => {
                            log::warn!(target: TARGET, "ERROR: {}", e);
                            wait.respond(None)
                        }
                    }
                });
                if reqs >= limit {
                    break;
                }
            }
            log::debug!(
                target: TARGET,
                "FindValue with DHT key ID {} query, {} parallel reqs, {}",
                key_dumper, wait.count(), iter
            );
            let mut finished = match &policy {
                DhtSearchPolicy::FastSearch(_) => true,
                DhtSearchPolicy::FullSearch(_) => false,
            };
            loop {
                match wait.wait(&mut queue_reader, !all).await {
                    Some(None) => (),
                    Some(Some(val)) => ret.push(val),
                    None => finished = true,
                }
                // Update iterator if required
                if all || ret.is_empty() || finished {
                    let updated_known_peers = self.network.known_peers.all().count();
                    if updated_known_peers != known_peers {
                        iter.update(&self.network);
                        known_peers = updated_known_peers;
                    }
                }
                // Add more tasks if required
                if !all || (ret.len() < limit) || finished {
                    break;
                }
            }
            // Stop if possible
            if (all && (ret.len() >= limit)) || (!all && !ret.is_empty()) || finished {
                break;
            }
        }
        if iter.order.is_empty() {
            iter_opt.take();
        }
        Ok(ret)
    }

    fn get_affinity(key1: &DhtKeyId, key2: &DhtKeyId) -> u8 {
        let mut ret = 0;
        for i in 0..32 {
            match key1[i] ^ key2[i] {
                0 => ret += 8,
                x => {
                    if (x & 0xF0) == 0 {
                        ret += Self::BITS[(x & 0x0F) as usize] + 4
                    } else {
                        ret += Self::BITS[(x >> 4) as usize]
                    }
                    break;
                }
            }
        }
        ret
    }

    fn parse_value_as_address(
        key: DhtKeyDescription,
        value: TLObject,
    ) -> Result<(IpAddress, Option<IpAddress>, Arc<dyn KeyOption>)> {
        if let Ok(addr_list) = value.downcast::<AddressListBoxed>() {
            let addr_list = addr_list.only();
            let (adnl_addr, quic_addr) = AdnlNode::parse_address_list(&addr_list)?
                .ok_or_else(|| error!("Wrong address list in DHT search"))?;
            let peer_key: Arc<dyn KeyOption> = (&key.id).try_into()?;
            Ok((adnl_addr, quic_addr, peer_key))
        } else {
            fail!("Address list type mismatch in DHT search")
        }
    }

    fn process_find_node(&self, network: &Arc<DhtNetwork>, query: &FindNode) -> Result<Nodes> {
        log::trace!(target: TARGET, "Process FindNode query {:?}", query);
        let key1 = network.node_key.id().data();
        let key2 = query.key.as_slice();
        let mut dist = 0u8;
        let mut ret = Vec::new();
        for i in 0..32 {
            if ret.len() == query.k as usize {
                break;
            }
            let mut subdist = dist;
            let mut xor = key1[i] ^ key2[i];
            while xor != 0 {
                if (xor & 0xF0) == 0 {
                    subdist = subdist.saturating_add(4);
                    xor <<= 4;
                } else {
                    let shift = Self::BITS[(xor >> 4) as usize];
                    subdist = subdist.saturating_add(shift);
                    if let Some(bucket) = network.buckets.get(&subdist) {
                        for node in bucket.val().iter() {
                            ret.push(node.val().object.clone());
                            if ret.len() == query.k as usize {
                                break;
                            }
                        }
                    }
                    xor <<= shift + 1;
                    subdist = subdist.saturating_add(1);
                }
                if ret.len() == query.k as usize {
                    break;
                }
            }
            dist = dist.saturating_add(8);
        }
        let ret = Nodes { nodes: ret.into() };
        log::trace!(target: TARGET, "FindNode result {:?}", ret);
        Ok(ret)
    }

    fn process_find_value(
        &self,
        network: &Arc<DhtNetwork>,
        query: &FindValue,
    ) -> Result<DhtValueResult> {
        log::trace!(target: TARGET, "Process FindValue query {:?}", query);
        let ret = if let Some(value) = network.search_dht_key(query.key.as_slice()) {
            ValueFound { value: value.into_boxed() }.into_boxed()
        } else {
            ValueNotFound {
                nodes: Nodes { nodes: network.get_known_nodes(query.k as usize)?.into() },
            }
            .into_boxed()
        };
        log::trace!(target: TARGET, "FindValue result {:?}", ret);
        Ok(ret)
    }

    fn process_ping(&self, query: &DhtPing) -> Result<DhtPong> {
        Ok(DhtPong { random_id: query.random_id })
    }

    fn process_store(&self, network: &Arc<DhtNetwork>, query: Store) -> Result<Stored> {
        let dht_key_id = hash(query.value.key.key.clone())?;
        if query.value.ttl <= Version::get() {
            fail!("Ignore expired DHT value with key {}", base64_encode(&dht_key_id))
        }
        match query.value.key.update_rule {
            UpdateRule::Dht_UpdateRule_Signature => {
                self.process_store_signed_value(network, dht_key_id, query.value)?
            }
            UpdateRule::Dht_UpdateRule_OverlayNodes => {
                self.process_store_overlay_nodes(network, dht_key_id, query.value)?
            }
            _ => fail!("Unsupported store query {:?}", query),
        };
        Ok(Stored::Dht_Stored)
    }

    fn process_store_overlay_nodes(
        &self,
        network: &Arc<DhtNetwork>,
        dht_key_id: DhtKeyId,
        value: DhtValue,
    ) -> Result<bool> {
        log::trace!(target: TARGET, "Process Store Overlay Nodes {value:?}");
        if !value.signature.is_empty() {
            fail!("Wrong value signature for OverlayNodes")
        }
        if !value.key.signature.is_empty() {
            fail!("Wrong key signature for OverlayNodes")
        }
        let overlay_short_id = match value.key.id {
            PublicKey::Pub_Overlay(_) => OverlayShortId::from_data(hash_boxed(&value.key.id)?),
            _ => fail!("Wrong key description format for OverlayNodes"),
        };
        if Self::dht_key_from_key_id(&overlay_short_id, "nodes") != value.key.key {
            fail!("Wrong DHT key for OverlayNodes")
        }
        let mut nodes_list = Self::deserialize_overlay_nodes(&value.value)?;
        let mut nodes = Vec::new();
        while let Some(node) = nodes_list.pop() {
            if let Err(e) = OverlayUtils::verify_node(&overlay_short_id, &node) {
                log::warn!(target: TARGET, "Bad overlay node {node:?}: {e}")
            } else {
                nodes.push(node)
            }
        }
        if nodes.is_empty() {
            fail!("Empty overlay nodes list")
        }
        add_counted_object_to_map_with_update(&network.storage, dht_key_id, |old_value| {
            let old_value = if let Some(old_value) = old_value {
                if old_value.object.ttl < Version::get() {
                    None
                } else if old_value.object.ttl > value.ttl {
                    return Ok(None);
                } else {
                    Some(&old_value.object.value)
                }
            } else {
                None
            };
            let mut old_nodes = if let Some(old_value) = old_value {
                Self::deserialize_overlay_nodes(old_value)?
            } else {
                Vec::new()
            };
            for node in nodes.iter() {
                let mut found = false;
                for old_node in old_nodes.iter_mut() {
                    if node.id == old_node.id {
                        if node.version > old_node.version {
                            *old_node = node.clone()
                        } else {
                            return Ok(None);
                        }
                        found = true;
                        break;
                    }
                }
                if !found {
                    old_nodes.push(node.clone())
                }
            }
            let nodes = OverlayNodesV1 { nodes: old_nodes.into() }.into_boxed();
            let mut ret = ValueObject {
                object: value.clone(),
                counter: self.allocated.values.clone().into(),
            };
            #[cfg(feature = "telemetry")]
            self.telemetry.values.update(self.allocated.values.load(Ordering::Relaxed));
            ret.object.value = serialize_boxed(&nodes)?.into();
            log::trace!(target: TARGET, "Store Overlay Nodes result {:?}", ret.object);
            Ok(Some(ret))
        })
    }

    fn process_store_signed_value(
        &self,
        network: &Arc<DhtNetwork>,
        dht_key_id: DhtKeyId,
        mut value: DhtValue,
    ) -> Result<bool> {
        Self::verify_value(&mut value)?;
        add_counted_object_to_map_with_update(&network.storage, dht_key_id, |old_value| {
            if let Some(old_value) = old_value {
                if old_value.object.ttl >= value.ttl {
                    return Ok(None);
                }
            }
            let ret = ValueObject {
                object: value.clone(),
                counter: self.allocated.values.clone().into(),
            };
            #[cfg(feature = "telemetry")]
            self.telemetry.values.update(self.allocated.values.load(Ordering::Relaxed));
            Ok(Some(ret))
        })
    }

    async fn query(
        &self,
        network: &Arc<DhtNetwork>,
        dst: &Arc<KeyId>,
        query: &TaggedTlObject,
    ) -> Result<Option<TLObject>> {
        let peers = AdnlPeers::with_keys(network.node_key.id().clone(), dst.clone());
        let result = self.adnl.query(query, &peers, None).await?;
        network.set_query_result(result, dst, self.adnl.elapsed_sec())
    }

    async fn query_with_prefix(
        &self,
        network: &Arc<DhtNetwork>,
        dst: &Arc<KeyId>,
        query: &TaggedTlObject,
    ) -> Result<Option<TLObject>> {
        let peers = AdnlPeers::with_keys(network.node_key.id().clone(), dst.clone());
        let result = self
            .adnl
            .query_with_prefix(Some(&network.query_prefix[..]), query, &peers, None)
            .await?;
        network.set_query_result(result, dst, self.adnl.elapsed_sec())
    }

    fn sign_key_description(name: &str, key: &Arc<dyn KeyOption>) -> Result<DhtKeyDescription> {
        let key_description = DhtKeyDescription {
            id: key.try_into()?,
            key: Self::dht_key_from_key_id(key.id(), name),
            signature: Default::default(),
            update_rule: UpdateRule::Dht_UpdateRule_Signature,
        };
        key_description.sign(key)
    }

    fn sign_local_node(&self, network: &DhtNetwork) -> Result<Node> {
        let local_node = Node {
            id: (&network.node_key).try_into()?,
            addr_list: self.adnl.build_address_list(None)?,
            signature: Default::default(),
            version: Version::get(),
        };
        local_node.sign(&network.node_key)
    }

    fn sign_value(name: &str, value: Vec<u8>, key: &Arc<dyn KeyOption>) -> Result<DhtValue> {
        let value = DhtValue {
            key: Self::sign_key_description(name, key)?,
            ttl: Version::get() + Self::TIMEOUT_VALUE,
            signature: Default::default(),
            value: value.into(),
        };
        value.sign(key)
    }

    async fn store_value(
        self: &Arc<Self>,
        key: DhtKey,
        value: DhtValue,
        check_type: impl Fn(&TLObject) -> bool + Copy + Send + 'static,
        check_all: bool,
        check_vals: impl Fn(Vec<(DhtKeyDescription, TLObject)>) -> Result<bool>,
    ) -> Result<bool> {
        let key_id = Arc::new(hash(key)?);
        let query: TaggedTlObject = Store { value }.into_tl_object().into();
        let query = Arc::new(query);
        let policy = DhtSearchPolicy::FullSearch(Self::MAX_TASKS);
        let mut iter = None;
        let mut peer = self.network.known_peers.next(&mut iter);
        while peer.is_some() {
            let (wait, mut queue_reader) = Wait::new();
            while let Some(next) = peer {
                peer = self.network.known_peers.next(&mut iter);
                let dht = self.clone();
                let network = dht.network.clone();
                let query = query.clone();
                let wait = wait.clone();
                wait.request();
                tokio::spawn(async move {
                    let ret = match dht.query(&network, &next, &query).await {
                        Ok(Some(answer)) => {
                            match Query::parse::<TLObject, Stored>(answer, &query.object) {
                                Ok(_) => Some(()), // Probably stored
                                Err(answer) => {
                                    log::debug!(
                                        target: TARGET,
                                        "Improper store reply: {:?}",
                                        answer
                                    );
                                    None
                                }
                            }
                        }
                        Ok(None) => None, // No reply at all
                        Err(e) => {
                            log::warn!(target: TARGET, "Store error: {:?}", e);
                            None
                        }
                    };
                    wait.respond(ret)
                });
            }
            while wait.wait(&mut queue_reader, false).await.is_some() {}
            if check_vals(
                self.find_value(&key_id, check_type, &policy, check_all, &mut None).await?,
            )? {
                return Ok(true);
            }
            peer = self.network.known_peers.next(&mut iter);
        }
        Ok(false)
    }

    async fn try_process_query(
        &self,
        network: &Arc<DhtNetwork>,
        object: TLObject,
    ) -> Result<QueryResult> {
        let object = match object.downcast::<DhtPing>() {
            Ok(query) => {
                return QueryResult::consume(
                    self.process_ping(&query)?,
                    #[cfg(feature = "telemetry")]
                    None,
                )
            }
            Err(object) => object,
        };
        let object = match object.downcast::<FindNode>() {
            Ok(query) => {
                return QueryResult::consume(
                    self.process_find_node(&network, &query)?,
                    #[cfg(feature = "telemetry")]
                    None,
                )
            }
            Err(object) => object,
        };
        let object = match object.downcast::<FindValue>() {
            Ok(query) => {
                return QueryResult::consume_boxed(
                    self.process_find_value(&network, &query)?,
                    #[cfg(feature = "telemetry")]
                    None,
                )
            }
            Err(object) => object,
        };
        let object = match object.downcast::<GetSignedAddressList>() {
            Ok(_) => {
                return QueryResult::consume(
                    self.sign_local_node(&network)?,
                    #[cfg(feature = "telemetry")]
                    None,
                )
            }
            Err(object) => object,
        };
        match object.downcast::<Store>() {
            Ok(query) => QueryResult::consume_boxed(
                self.process_store(&network, query)?,
                #[cfg(feature = "telemetry")]
                None,
            ),
            Err(object) => {
                log::warn!(target: TARGET, "Unexpected DHT query {:?}", object);
                Ok(QueryResult::Rejected(object))
            }
        }
    }

    async fn value_query(
        &self,
        network: &Arc<DhtNetwork>,
        peer: &Arc<KeyId>,
        query: &Arc<TaggedTlObject>,
        key: &Arc<DhtKeyId>,
        check: impl Fn(&TLObject) -> bool,
    ) -> Result<Option<(DhtKeyDescription, TLObject)>> {
        let answer = self.query(network, peer, query).await?;
        if let Some(answer) = answer {
            let answer: DhtValueResult = Query::parse(answer, &query.object)?;
            match answer {
                DhtValueResult::Dht_ValueFound(value) => {
                    let value = value.value.only();
                    log::debug!(
                        target: TARGET,
                        "Found value for DHT key ID {}: {:?} / {:?}",
                        base64_encode(&key[..]), value.key, value.value
                    );
                    let object = deserialize_boxed(&value.value)?;
                    if check(&object) {
                        return Ok(Some((value.key, object)));
                    }
                    log::debug!(
                        target: TARGET,
                        "Improper value found, object {:?}",
                        object
                    );
                }
                DhtValueResult::Dht_ValueNotFound(nodes) => {
                    let nodes = nodes.nodes.nodes;
                    log::debug!(
                        target: TARGET,
                        "Value not found on {} for DHT key ID {}, suggested {} other nodes",
                        peer, base64_encode(&key[..]), nodes.len()
                    );
                    for node in nodes.iter() {
                        self.add_peer_to_dht_network(network, node)?;
                    }
                }
            }
        } else {
            log::debug!(
                target: TARGET,
                "No answer from {} to FindValue with DHT key ID {} query",
                peer, base64_encode(&key[..])
            );
        }
        Ok(None)
    }

    fn verify_other_node(node: &Node) -> Result<()> {
        let other_key: Arc<dyn KeyOption> = (&node.id).try_into()?;
        let mut node = node.clone();
        node.verify(&other_key)
    }

    fn verify_value(value: &mut DhtValue) -> Result<()> {
        let other_key: Arc<dyn KeyOption> = (&value.key.id).try_into()?;
        value.verify(&other_key)?;
        value.key.verify(&other_key)
    }
}

#[async_trait::async_trait]
impl Subscriber for DhtNode {
    #[cfg(feature = "telemetry")]
    async fn poll(&self, _start: &Arc<Instant>) {
        self.telemetry.peers.update(self.allocated.peers.load(Ordering::Relaxed));
        self.telemetry.values.update(self.allocated.values.load(Ordering::Relaxed));
    }

    async fn try_consume_query(&self, object: TLObject, _peers: &AdnlPeers) -> Result<QueryResult> {
        self.try_process_query(&self.network, object).await
    }

    async fn try_consume_query_bundle(
        &self,
        mut objects: Vec<TLObject>,
        _peers: &AdnlPeers,
    ) -> Result<QueryResult> {
        if objects.len() != 2 {
            return Ok(QueryResult::RejectedBundle(objects));
        }
        let other_node = match objects.remove(0).downcast::<DhtQuery>() {
            Ok(query) => query.node,
            Err(object) => {
                objects.insert(0, object);
                return Ok(QueryResult::RejectedBundle(objects));
            }
        };
        self.add_peer_to_dht_network(&self.network, &other_node)?;
        let ret = self.try_process_query(&self.network, objects.remove(0)).await?;
        if let QueryResult::Rejected(object) = ret {
            fail!("Unexpected DHT query {:?}", object);
        }
        Ok(ret)
    }
}
