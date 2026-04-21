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
use crate::{common::add_unbound_object_to_map, telemetry::Metric};
use crate::{
    common::{
        add_counted_object_to_map, add_counted_object_to_map_with_update,
        add_unbound_object_to_map_with_update, hash, hash_boxed, AdnlPeers, AsyncReceiver,
        CountedObject, Counter, Query, QueryAnswer, QueryResult, Subscriber, TaggedByteSlice,
        TaggedTlObject, Version,
    },
    declare_counted,
    node::{AddressCache, AddressCacheWithBads, AdnlNode, AdnlSendMethod, BadPolicy, IpAddress},
    quic::QuicNode,
    rldp::{Constraints, RldpNode},
};
use num_traits::pow::Pow;
use std::{
    borrow::Borrow,
    cmp::min,
    collections::{HashMap, HashSet},
    convert::TryInto,
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        Arc, OnceLock,
    },
    time::{Duration, Instant},
};
use ton_api::{
    deserialize_boxed, deserialize_boxed_bundle_with_suffix, deserialize_boxed_with_suffix,
    serialize_boxed, serialize_boxed_append,
    ton::{
        adnl::id::short::Short as AdnlShortId,
        catchain::{
            block::inner::{
                catchain::block::data::data::Fork as CatchainFork,
                Data as CatchainBlockInnerDataBoxed,
            },
            blockupdate::BlockUpdate as CatchainBlockUpdate,
            FirstBlock as CatchainFirstBlock, Update as CatchainBlockUpdateBoxed,
        },
        overlay::{
            broadcast::BroadcastFec,
            membercertificate::MemberCertificate,
            membercertificateid::MemberCertificateId,
            message::{Message as OverlayMessage, MessageWithExtra as OverlayMessageWithExtra},
            messageextra::MessageExtra,
            node::{
                tosign::{ToSign as NodeToSign, ToSignEx as NodeToSignEx},
                Node as NodeV1, ToSign as NodeToSignBoxed,
            },
            nodes::Nodes as NodesV1,
            nodesv2::NodesV2,
            nodev2::NodeV2,
            Broadcast, MemberCertificate as MemberCertificateBoxed, Message as OverlayMessageBoxed,
            Nodes as NodesV1Boxed, NodesV2 as NodesV2Boxed, Pong,
        },
        pub_::publickey::Overlay as OverlayKey,
        rpc::overlay::{
            GetRandomPeers, GetRandomPeersV2, Ping, Query as OverlayQuery,
            QueryWithExtra as OverlayQueryWithExtra,
        },
        ton_node::{
            customoverlayid::CustomOverlayId, fastsyncoverlayid::FastSyncOverlayId,
            shardid::ShardId as TonNodeShardId, shardpublicoverlayid::ShardPublicOverlayId,
        },
        validator_session::{
            blockupdate::BlockUpdate as ValidatorSessionBlockUpdate,
            BlockUpdate as ValidatorSessionBlockUpdateBoxed,
        },
        PublicKey,
    },
    AnyBoxedSerialize, IntoBoxed, TLObject,
};
#[cfg(feature = "telemetry")]
use ton_api::{BoxedSerialize, Constructor};
use ton_block::{base64_encode, error, fail, KeyId, KeyOption, Result, UInt256, UnixTime};

mod broadcast;
use broadcast::{
    BroadcastData, BroadcastFecProtocol, BroadcastId, BroadcastProtocol, BroadcastRecvContext,
    BroadcastSendContext, BroadcastSendMethod, BroadcastSimpleProtocol,
    BroadcastStreamSimpleProtocol, BroadcastTwostepFecProtocol, BroadcastTwostepSimpleProtocol,
    OwnedBroadcast,
};
pub use broadcast::{BroadcastRecvInfo, BroadcastSendInfo};

const TARGET: &str = "overlay";
const TARGET_BROADCAST: &str = "overlay_broadcast";

/*
pub fn build_overlay_node_info(
    overlay: &Arc<OverlayShortId>,
    version: i32,
    key: &str,
    signature: &str
) -> Result<Node<NodeV1, NodeV2Boxed>> {
    let key = base64_decode(key)?;
    if key.len() != 32 {
        fail!("Bad public key length")
    }
    let key: [u8; 32] = key.as_slice().try_into()?;
    let signature = base64_decode(signature)?;
    let node = NodeV1 {
        id: Ed25519 {
            key: UInt256::with_array(key)
        }.into_boxed(),
        overlay: UInt256::with_array(*overlay.data()),
        version,
        signature: signature.into(),
    };
    Ok(Node::V1(node))
}
*/

pub enum CatchainData {
    Catchain(CatchainFork),
    ValidatorSession(ValidatorSessionBlockUpdate),
}

pub type OverlayId = [u8; 32];

#[derive(Debug)]
pub enum OverlayNodeInfo<N1: Borrow<NodeV1>, N2: Borrow<NodeV2>> {
    V1(N1),
    V2(N2),
}

impl<N1: Borrow<NodeV1>, N2: Borrow<NodeV2>> OverlayNodeInfo<N1, N2> {
    pub fn id(&self) -> &PublicKey {
        match self {
            OverlayNodeInfo::V1(node) => &node.borrow().id,
            OverlayNodeInfo::V2(node) => &node.borrow().id,
        }
    }
    pub fn version(&self) -> i32 {
        match self {
            OverlayNodeInfo::V1(node) => node.borrow().version,
            OverlayNodeInfo::V2(node) => node.borrow().version,
        }
    }
    pub fn signature(&self) -> &[u8] {
        match self {
            OverlayNodeInfo::V1(node) => &node.borrow().signature,
            OverlayNodeInfo::V2(node) => &node.borrow().signature,
        }
    }
    pub fn to_owned(&self) -> OverlayNodeInfo<NodeV1, NodeV2> {
        match self {
            OverlayNodeInfo::V1(node) => OverlayNodeInfo::V1(node.borrow().clone()),
            OverlayNodeInfo::V2(node) => OverlayNodeInfo::V2(node.borrow().clone()),
        }
    }
    pub fn key(&self) -> Result<Arc<dyn KeyOption>> {
        match self {
            OverlayNodeInfo::V1(node) => node.borrow().get_key(),
            OverlayNodeInfo::V2(node) => node.borrow().get_key(),
        }
    }
}

pub type OverlayShortId = KeyId;
pub type PrivateOverlayShortId = KeyId;

/// Overlay utilities
pub struct OverlayUtils;

impl OverlayUtils {
    /// Calculate overlay full ID for public shard overlay
    /// SHA256(TL(tonNode.shardPublicOverlayId))
    pub fn calc_overlay_id(
        workchain: i32,
        shard: i64,
        zero_state_file_hash: &[u8; 32],
    ) -> Result<OverlayId> {
        let overlay = ShardPublicOverlayId {
            shard,
            workchain,
            zero_state_file_hash: UInt256::with_array(*zero_state_file_hash),
        };
        hash(overlay)
    }

    /// Calculate overlay short ID for public shard overlay
    /// SHA256(TL(pub.overlay { name: SHA256(TL(tonNode.shardPublicOverlayId)) }))
    pub fn calc_overlay_short_id(
        workchain: i32,
        shard: i64,
        zero_state_file_hash: &[u8; 32],
    ) -> Result<Arc<OverlayShortId>> {
        let overlay_key = OverlayKey {
            name: Self::calc_overlay_id(workchain, shard, zero_state_file_hash)?.to_vec(),
        };
        Ok(OverlayShortId::from_data(hash(overlay_key)?))
    }

    /// Calculate overlay short ID for catchain private overlay
    /// SHA256(TL(pub.overlay { name: SHA256(TL(catchain.firstblock)) }))
    pub fn calc_private_overlay_short_id(
        first_block: &CatchainFirstBlock,
    ) -> Result<Arc<PrivateOverlayShortId>> {
        let serialized_first_block = serialize_boxed(first_block)?;
        let overlay_key = OverlayKey { name: serialized_first_block.into() };
        Ok(PrivateOverlayShortId::from_data(hash_boxed(&overlay_key.into_boxed())?))
    }

    /// Calculate overlay short ID for custom overlay
    /// SHA256(TL(pub.overlay { name: SHA256(TL(tonNode.customOverlayId)) }))
    pub fn calc_custom_overlay_short_id(
        zero_state_file_hash: &[u8; 32],
        name: &str,
        nodes: &[[u8; 32]],
    ) -> Result<Arc<OverlayShortId>> {
        let mut sorted: Vec<UInt256> = nodes.iter().map(|n| UInt256::with_array(*n)).collect();
        sorted.sort();
        sorted.dedup();
        let id = CustomOverlayId {
            zero_state_file_hash: UInt256::with_array(*zero_state_file_hash),
            name: name.to_string(),
            nodes: sorted.into_iter().collect(),
        };
        let overlay_key = OverlayKey { name: hash(id)?.to_vec() };
        Ok(OverlayShortId::from_data(hash(overlay_key)?))
    }

    /// Calculate overlay short ID for fast sync overlay
    /// SHA256(TL(pub.overlay { name: SHA256(TL(tonNode.fastSyncOverlayId)) }))
    pub fn calc_fast_sync_overlay_short_id(
        zero_state_file_hash: &[u8; 32],
        workchain: i32,
        shard: i64,
    ) -> Result<Arc<OverlayShortId>> {
        let id = FastSyncOverlayId {
            zero_state_file_hash: UInt256::with_array(*zero_state_file_hash),
            shard: TonNodeShardId { workchain, shard },
        };
        let overlay_key = OverlayKey { name: hash(id)?.to_vec() };
        Ok(OverlayShortId::from_data(hash(overlay_key)?))
    }

    /// Verify node info
    pub(crate) fn verify_node<T: NodeData>(
        overlay_id: &Arc<OverlayShortId>,
        node: &T,
    ) -> Result<()> {
        let key: Arc<dyn KeyOption> = node.get_key()?;
        if node.overlay().as_slice() != overlay_id.data() {
            fail!(
                "Got peer {} with wrong overlay {}, expected {overlay_id}",
                key.id(),
                base64_encode(node.overlay().as_slice())
            )
        }
        let to_sign = Self::get_node_to_sign(&key, node.overlay(), node.flags(), node.version());
        if let Err(e) = key.verify(&serialize_boxed(&to_sign)?, node.signature()) {
            fail!("Got peer {} with bad signature: {}", key.id(), e)
        }
        Ok(())
    }

    fn calc_message_prefix(overlay_id: &OverlayShortId) -> Result<Vec<u8>> {
        serialize_boxed(
            &OverlayMessage { overlay: UInt256::with_array(*overlay_id.data()) }.into_boxed(),
        )
    }

    fn get_node_to_sign(
        key: &Arc<dyn KeyOption>,
        overlay_id: &UInt256,
        flags: i32,
        version: i32,
    ) -> NodeToSignBoxed {
        let id = AdnlShortId { id: UInt256::with_array(*key.id().data()) };
        if flags != 0 {
            NodeToSignEx { flags, id, overlay: overlay_id.clone(), version }.into_boxed()
        } else {
            NodeToSign { id, overlay: overlay_id.clone(), version }.into_boxed()
        }
    }
}

type CatchainReceiver = AsyncReceiver<(CatchainBlockUpdate, CatchainData, Arc<KeyId>)>;

pub(crate) trait NodeData {
    fn flags(&self) -> i32;
    fn get_key(&self) -> Result<Arc<dyn KeyOption>>;
    fn overlay(&self) -> &UInt256;
    fn signature(&self) -> &[u8];
    fn version(&self) -> i32;
}

impl NodeData for NodeV1 {
    fn flags(&self) -> i32 {
        0
    }
    fn get_key(&self) -> Result<Arc<dyn KeyOption>> {
        (&self.id).try_into()
    }
    fn overlay(&self) -> &UInt256 {
        &self.overlay
    }
    fn signature(&self) -> &[u8] {
        &self.signature
    }
    fn version(&self) -> i32 {
        self.version
    }
}

impl NodeData for NodeV2 {
    fn flags(&self) -> i32 {
        self.flags
    }
    fn get_key(&self) -> Result<Arc<dyn KeyOption>> {
        (&self.id).try_into()
    }
    fn overlay(&self) -> &UInt256 {
        &self.overlay
    }
    fn signature(&self) -> &[u8] {
        &self.signature
    }
    fn version(&self) -> i32 {
        self.version
    }
}

declare_counted!(
    struct NodeObject {
        v1: Option<Arc<NodeV1>>,
        v2: Option<Arc<NodeV2>>,
    }
);

enum Nodes {
    V1(NodesV1),
    V2(NodesV2),
}

struct SlaveInfo {
    node_id: Arc<KeyId>,
    expire_at: u32, // utime in seconds
}

enum OverlayType {
    Public,
    // Overlay with fixed members set
    Private {
        key: Arc<dyn KeyOption>,
        use_quic: bool,
    },
    // Overlay with externally certified members
    CertifiedMembers {
        // Prefix to use in broadcasts instead of Overlay::message_prefix
        bcast_prefix: Vec<u8>,
        certificate: Option<MemberCertificate>,
        key: Option<Arc<dyn KeyOption>>,
        max_slaves: usize,
        // Validator ADNL IDs, bypassing the certificate check
        root_adnl_ids: HashSet<Arc<KeyId>>,
        // Validator Signing KeyId -> (Slot -> SlaveInfo)
        root_public_keys: HashMap<Arc<KeyId>, lockfree::map::Map<u32, SlaveInfo>>,
        use_quic: bool,
    },
}

impl OverlayType {
    fn bcast_prefix(&self) -> Option<Vec<u8>> {
        match self {
            OverlayType::CertifiedMembers { bcast_prefix, .. } => Some(bcast_prefix.clone()),
            OverlayType::Public | OverlayType::Private { .. } => None,
        }
    }

    fn quic_requested(&self) -> bool {
        matches!(
            self,
            OverlayType::Private { use_quic: true, .. } 
                | OverlayType::CertifiedMembers { use_quic: true, .. }
        )
    }

    fn calc_message_prefix(&self, overlay_id: &OverlayShortId) -> Result<Vec<u8>> {
        match self {
            Self::CertifiedMembers { certificate, .. } => serialize_boxed(
                &OverlayMessageWithExtra {
                    overlay: UInt256::with_array(*overlay_id.data()),
                    extra: MessageExtra {
                        certificate: certificate.as_ref().map(|cert| cert.clone().into_boxed()),
                    },
                }
                .into_boxed(),
            ),
            OverlayType::Public | OverlayType::Private { .. } => {
                OverlayUtils::calc_message_prefix(overlay_id)
            }
        }
    }

    fn calc_query_prefix(&self, overlay_id: &OverlayShortId) -> Result<Vec<u8>> {
        match self {
            Self::CertifiedMembers { certificate, .. } => serialize_boxed(&OverlayQueryWithExtra {
                overlay: UInt256::with_array(*overlay_id.data()),
                extra: MessageExtra {
                    certificate: certificate.as_ref().map(|cert| cert.clone().into_boxed()),
                },
            }),
            OverlayType::Public | OverlayType::Private { .. } => {
                serialize_boxed(&OverlayQuery { overlay: UInt256::with_array(*overlay_id.data()) })
            }
        }
    }

    fn certificate(&self) -> Option<&MemberCertificate> {
        match self {
            OverlayType::CertifiedMembers { certificate, .. } => certificate.as_ref(),
            OverlayType::Public | OverlayType::Private { .. } => None,
        }
    }

    fn is_public(&self) -> bool {
        matches!(self, OverlayType::Public)
    }

    fn is_private(&self) -> bool {
        matches!(self, OverlayType::Private { .. })
    }

    fn has_certified_members(&self) -> bool {
        matches!(self, OverlayType::CertifiedMembers { .. })
    }
}

#[cfg(feature = "telemetry")]
declare_counted!(
    struct PeerStats {
        count: AtomicU64,
    }
);

struct OverlayAlloc {
    consumers: Arc<AtomicU64>,
    overlays: Arc<AtomicU64>,
    peers: Arc<AtomicU64>,
    recv_transfers: Arc<AtomicU64>,
    send_transfers: Arc<AtomicU64>,
    #[cfg(feature = "telemetry")]
    stats_peer: Arc<AtomicU64>,
    #[cfg(feature = "telemetry")]
    stats_transfer: Arc<AtomicU64>,
}

#[cfg(feature = "telemetry")]
struct OverlayTelemetry {
    consumers: Arc<Metric>,
    overlays: Arc<Metric>,
    peers: Arc<Metric>,
    recv_transfers: Arc<Metric>,
    send_transfers: Arc<Metric>,
    stats_peer: Arc<Metric>,
    stats_transfer: Arc<Metric>,
}

#[cfg(feature = "telemetry")]
declare_counted!(
    struct TransferStats {
        income: AtomicU64,
        passed: AtomicU64,
        resent: AtomicU64,
    }
);

declare_counted!(
    struct Overlay {
        adnl: Arc<AdnlNode>,
        quic: Option<Arc<QuicNode>>,
        rldp: Option<Arc<RldpNode>>,
        options: Arc<AtomicU32>,
        overlay_type: OverlayType,
        flags: u8,
        hops: Option<u8>,
        // All known peers in overlay (for private overlay - all peers except ourself)
        known_peers: AddressCacheWithBads,
        message_prefix: Vec<u8>,
        // Subset of `known_peers`. Active peers. Used for pings and queries
        neighbours: AddressCache,
        // Info about all known peers. Used for `GetRandomPeers`
        nodes: lockfree::map::Map<Arc<KeyId>, NodeObject>,
        overlay_id: Arc<OverlayShortId>,
        owned_broadcasts: lockfree::map::Map<BroadcastId, OwnedBroadcast>,
        // Peers waiting for ADNL address resolution before being added to known_peers
        pending_peers: lockfree::queue::Queue<Arc<KeyId>>,
        purge_broadcasts: lockfree::queue::Queue<BroadcastId>,
        purge_broadcasts_count: AtomicU32,
        queue_one_time_broadcasts: tokio::sync::mpsc::UnboundedSender<(BroadcastId, Instant)>,
        #[cfg(feature = "xp25")]
        queue_repeated_broadcasts: tokio::sync::mpsc::UnboundedSender<(BroadcastId, Instant)>,
        query_prefix: Vec<u8>,
        // random_peers: AddressCache
        received_catchain: Option<Arc<CatchainReceiver>>,
        received_peers: Arc<AsyncReceiver<Vec<OverlayNodeInfo<NodeV1, NodeV2>>>>,
        received_rawbytes: Arc<AsyncReceiver<BroadcastRecvInfo>>,
        #[cfg(feature = "telemetry")]
        messages_recv: AtomicU64,
        #[cfg(feature = "telemetry")]
        messages_send: AtomicU64,
        #[cfg(feature = "telemetry")]
        print: AtomicU64,
        #[cfg(feature = "telemetry")]
        start: Instant,
        #[cfg(feature = "telemetry")]
        stats_per_peer_recv: lockfree::map::Map<Arc<KeyId>, lockfree::map::Map<u32, PeerStats>>,
        #[cfg(feature = "telemetry")]
        stats_per_peer_send: lockfree::map::Map<Arc<KeyId>, lockfree::map::Map<u32, PeerStats>>,
        #[cfg(feature = "telemetry")]
        stats_per_transfer: lockfree::map::Map<BroadcastId, Arc<TransferStats>>,
        #[cfg(feature = "telemetry")]
        telemetry: Arc<OverlayTelemetry>,
        allocated: Arc<OverlayAlloc>,
        // For debug
        debug_trace: AtomicU32,
    }
);

impl Overlay {
    const FLAG_OVERLAY_OTHER_WORKCHAIN: u8 = 0x01;
    const MAX_HOPS: u8 = 15;
    const MAX_BROADCAST_LOG: u32 = 1000;
    const MAX_RANDOM_PEERS: u32 = 4;
    const OPTION_DISABLE_BROADCAST_RETRANSMIT: u32 = 0x01;
    const SIZE_NEIGHBOURS_LONG_BROADCAST: u8 = 5;
    const SIZE_NEIGHBOURS_SHORT_BROADCAST: u8 = 3;
    const SIZE_NEIGHBOURS_STREAM_BROADCAST: u8 = 20;
    const TIMEOUT_ONE_TIME_BROADCAST_SEC: u64 = 60;
    #[cfg(feature = "xp25")]
    const TIMEOUT_REPEATED_BROADCAST_SEC: u64 = 2;

    pub(crate) fn calc_broadcast_neighbours(
        &self,
        hops: Option<u8>,
        default_neighbours: u8,
    ) -> Result<(Option<u8>, u32)> {
        if let Some(mut hops) = hops {
            let initial = (hops >> 4) == 0;
            if initial {
                hops |= hops << 4;
            }
            let hops_org = hops >> 4;
            let hops_cur = hops & 0x0F;
            if hops_org > Overlay::MAX_HOPS {
                fail!("Too big hops count requested ({hops_org})")
            }
            if (hops_cur > hops_org) || (hops_cur <= 1) {
                fail!("Bad hops counter ({hops:x}), initial {initial}")
            }
            // Heuristics with zero loss, where n is the number of nodes in network
            // M3(n) = 3.42 * n^0.287
            // M4(n) = 3.86 * n^0.181
            let n = self.known_peers.all().count() as f64;
            let n = if hops_org <= 3 {
                (n.pow(0.287f64) * 3.42f64).ceil() as u32
            } else {
                (n.pow(0.181f64) * 3.86f64).ceil() as u32
            };
            if n > self.neighbours.count() {
                if initial {
                    if hops_org < Overlay::MAX_HOPS {
                        hops = ((hops_org + 1) << 4) | (hops_org + 1)
                    }
                    Ok((Some(hops), default_neighbours as u32))
                } else {
                    Ok((None, default_neighbours as u32))
                }
            } else {
                if !initial {
                    hops -= 1;
                }
                Ok((Some(hops), n))
            }
        } else {
            Ok((None, default_neighbours as u32))
        }
    }

    pub(crate) fn calc_broadcast_twostep_neighbours(&self) -> u32 {
        self.neighbours.count() as u32
    }

    pub(crate) fn select_broadcast_neighbours(
        &self,
        count: u32,
        skip: Option<&Arc<KeyId>>,
    ) -> Vec<Arc<KeyId>> {
        self.neighbours.random_vec(skip, count)
    }

    pub(crate) fn select_broadcast_twostep_neighbours(
        &self,
        skip: Option<&Arc<KeyId>>,
    ) -> Vec<Arc<KeyId>> {
        let root_adnl_ids = match &self.overlay_type {
            OverlayType::CertifiedMembers { root_adnl_ids, .. } => Some(root_adnl_ids),
            _ => None,
        };
        let mut neighbours = Vec::new();
        let (mut iter, mut neighbour) = self.neighbours.first();
        while let Some(node) = neighbour {
            let skipped = if let Some(skip) = &skip { &node == *skip } else { false };
            // Skip CertifiedMembers: only send twostep to root members (validators).
            let root = if let Some(roots) = root_adnl_ids { roots.contains(&node) } else { true };
            if !skipped && root {
                neighbours.push(node);
            }
            neighbour = self.neighbours.next(&mut iter);
        }
        neighbours
    }

    pub(crate) fn serialize_broadcast(&self, bcast: &Broadcast) -> Result<Vec<u8>> {
        // In semi-private overlays we use special message prefix (without certificate)
        let mut buf = self.overlay_type.bcast_prefix().unwrap_or(self.message_prefix.clone());
        serialize_boxed_append(&mut buf, bcast)?;
        Ok(buf)
    }

    fn check_peer(&self, peer: &Arc<KeyId>, certificate: Option<&MemberCertificate>) -> Result<()> {
        match &self.overlay_type {
            OverlayType::Public => Ok(()),
            OverlayType::Private { key, .. } => {
                if !(peer == key.id() || self.known_peers.all().contains(peer)) {
                    fail!("Peer {peer} is not a member of the private overlay {}", self.overlay_id)
                }
                Ok(())
            }
            OverlayType::CertifiedMembers { root_adnl_ids, .. } => {
                if root_adnl_ids.contains(peer) {
                    return Ok(());
                }
                // Bcasts are sent without a certificate, hoping the target already knows
                // the sender's one.
                if let Some(guard) = self.nodes.get(peer) {
                    if let Some(node) = &guard.val().v2 {
                        let MemberCertificateBoxed::Overlay_MemberCertificate(cert) =
                            &node.certificate
                        else {
                            fail!(
                                "Empty certificate for known peer {peer} in overlay {}",
                                self.overlay_id
                            )
                        };
                        return self.validate_certificate(peer, cert).map_err(|e| {
                            error!(
                                "Certificate validation for known peer {peer} in overlay {}: {e}",
                                self.overlay_id
                            )
                        });
                    }
                }
                let Some(certificate) = certificate else {
                    fail!(
                        "Cannot validate {peer} with empty certificate in the overlay {}",
                        self.overlay_id
                    )
                };
                self.validate_certificate(peer, certificate).map_err(|e| {
                    error!(
                        "Certificate validation for new peer {peer} in overlay {}: {e}",
                        self.overlay_id
                    )
                })
            }
        }
    }

    async fn distribute_broadcast(
        &self,
        data: &TaggedByteSlice<'_>,
        bcast_id: &BroadcastId,
        seqno: u32,
        key: &Arc<KeyId>,
        neighbours: &[Arc<KeyId>],
        method: &BroadcastSendMethod,
    ) -> Result<()> {
        log::trace!(
            target: TARGET,
            "Broadcast {} part {seqno} via {method} {} bytes to overlay {}, {} neighbours",
            base64_encode(bcast_id),
            data.object.len(),
            self.overlay_id,
            neighbours.len()
        );
        let mut peers: Option<AdnlPeers> = None;
        #[cfg(feature = "telemetry")]
        let mut addrs = Vec::new();
        for neighbour in neighbours.iter() {
            #[cfg(feature = "telemetry")]
            if let Err(e) = self.update_stats(neighbour, data.tag, true) {
                log::warn!(
                    target: TARGET,
                    "Cannot update statistics in overlay {} for {neighbour} during broadcast: {e}",
                    self.overlay_id
                )
            }
            let peers = if let Some(peers) = &mut peers {
                peers.set_other(neighbour.clone());
                peers
            } else {
                peers.get_or_insert_with(|| AdnlPeers::with_keys(key.clone(), neighbour.clone()))
            };
            #[cfg(feature = "telemetry")]
            addrs.push(peers.other().to_string());
            let err = match method {
                BroadcastSendMethod::Fast => {
                    self.adnl.send_custom_get_status(data, peers, AdnlSendMethod::Fast).await.err()
                }
                BroadcastSendMethod::QuicOrRldp => {
                    if let Some(quic) = self.quic.as_ref() {
                        quic.message(data.object.to_vec(), Some(&self.adnl), peers).await.err()
                    } else {
                        let Some(_rldp) = self.rldp.as_ref() else {
                            fail!(
                                "Neither QUIC nor RLDP sender is set in overlay {}",
                                self.overlay_id
                            );
                        };
                        //rldp.message(data, peers, true, None).await.err()
                        None
                    }
                }
                BroadcastSendMethod::Safe => {
                    self.adnl.send_custom_get_status(data, peers, AdnlSendMethod::Safe).await.err()
                }
            };
            if let Some(e) = err {
                log::warn!(
                    target: TARGET,
                    "Cannot distribute broadcast {} part {seqno} via {method} in overlay {} \
                    to {neighbour}: {e}",
                    base64_encode(bcast_id),
                    self.overlay_id
                )
            }
        }
        #[cfg(feature = "telemetry")]
        log::info!(
            target: TARGET_BROADCAST,
            "Broadcast trace: distributed {} part {seqno} via {method} {} bytes to overlay {}, \
            peers {addrs:?}",
            base64_encode(bcast_id),
            data.object.len(),
            self.overlay_id,
        );
        Ok(())
    }

    async fn get_random_peers(
        &self,
        dst: &Arc<KeyId>,
        default_key: &Arc<dyn KeyOption>,
        v2: bool,
        timeout_ms: Option<u64>,
    ) -> Result<()> {
        log::trace!(target: TARGET, "Get random peers from {dst}");
        let query: TaggedTlObject = match self.prepare_random_peers(default_key, v2)? {
            Nodes::V1(peers) => GetRandomPeers { peers }.into_tl_object().into(),
            Nodes::V2(peers) => GetRandomPeersV2 { peers }.into_tl_object().into(),
        };
        let peers = self
            .prepare_to_send(
                dst,
                default_key,
                #[cfg(feature = "telemetry")]
                query.tag,
            )
            .await?;
        let answer = self
            .adnl
            .query_with_prefix(Some(&self.query_prefix), &query, &peers, timeout_ms)
            .await?;
        if let Some(answer) = answer {
            log::trace!(target: TARGET, "Got random peers from {dst}");
            let ret = if v2 {
                self.process_random_peers(
                    default_key,
                    Query::parse::<_, NodesV2Boxed>(answer, &query.object)?.only().nodes,
                    |node| OverlayNodeInfo::V2(node),
                )?
            } else {
                self.process_random_peers(
                    default_key,
                    Query::parse::<_, NodesV1Boxed>(answer, &query.object)?.only().nodes,
                    |node| OverlayNodeInfo::V1(node),
                )?
            };
            self.received_peers.push(ret);
        } else {
            log::warn!(target: TARGET, "No random peers from {dst}");
        }
        Ok(())
    }

    fn get_signed_node(
        &self,
        default_key: &Arc<dyn KeyOption>,
        v2: bool,
    ) -> Result<OverlayNodeInfo<NodeV1, NodeV2>> {
        let key = self.overlay_key().unwrap_or(default_key);
        let overlay_id = UInt256::with_array(*self.overlay_id.data());
        let flags = 0;
        let version = Version::get();
        let local_node = OverlayUtils::get_node_to_sign(key, &overlay_id, flags, version);
        let node = if v2 {
            let node = NodeV2 {
                id: key.try_into()?,
                certificate: self
                    .overlay_type
                    .certificate()
                    .map(|cert| cert.clone().into_boxed())
                    .unwrap_or_default(),
                flags,
                overlay: overlay_id,
                signature: key.sign(&serialize_boxed(&local_node)?)?.into(),
                version,
            };
            OverlayNodeInfo::V2(node)
        } else {
            let node = NodeV1 {
                id: key.try_into()?,
                overlay: overlay_id,
                signature: key.sign(&serialize_boxed(&local_node)?)?.into(),
                version,
            };
            OverlayNodeInfo::V1(node)
        };
        Ok(node)
    }

    fn overlay_key(&self) -> Option<&Arc<dyn KeyOption>> {
        match &self.overlay_type {
            OverlayType::Private { key, .. } => Some(key),
            OverlayType::CertifiedMembers { key, .. } => key.as_ref(),
            _ => None,
        }
    }

    async fn ping_peer(
        &self,
        default_key: &Arc<dyn KeyOption>,
        peer: &Arc<KeyId>,
        timeout_ms: u64,
    ) -> Result<()> {
        let peers = self
            .prepare_to_send(
                peer,
                default_key,
                #[cfg(feature = "telemetry")]
                Ping::constructor_const(),
            )
            .await?;
        let query: TaggedTlObject = Ping.into_tl_object().into();
        let pong = self
            .adnl
            .query_with_prefix(Some(&self.query_prefix), &query, &peers, Some(timeout_ms))
            .await?;
        let elapsed_sec = self.adnl.elapsed_sec();
        if pong.is_some() {
            self.known_peers.amnesty(peer, elapsed_sec);
        } else {
            self.known_peers.penalty(peer, elapsed_sec)?;
        }
        Ok(())
    }

    fn prepare_random_peers(&self, default_key: &Arc<dyn KeyOption>, v2: bool) -> Result<Nodes> {
        let local_node = self.get_signed_node(default_key, v2)?;
        let nodes = AddressCache::with_limit(Self::MAX_RANDOM_PEERS);
        self.neighbours.random_set(&nodes, Self::MAX_RANDOM_PEERS)?;
        let (mut iter, mut current) = nodes.first();
        let ret = match local_node {
            OverlayNodeInfo::V1(node) => {
                let mut ret = vec![node];
                while let Some(node) = current {
                    match self.nodes.get(&node).map(|node| node.val().v1.clone()) {
                        Some(Some(node)) => ret.push(node.as_ref().clone()),
                        _ => (),
                    }
                    current = nodes.next(&mut iter)
                }
                Nodes::V1(NodesV1 { nodes: ret })
            }
            OverlayNodeInfo::V2(node) => {
                let mut ret = vec![node];
                while let Some(node) = current {
                    match self.nodes.get(&node).map(|node| node.val().v2.clone()) {
                        Some(Some(node)) => ret.push(node.as_ref().clone()),
                        _ => (),
                    }
                    current = nodes.next(&mut iter)
                }
                Nodes::V2(NodesV2 { nodes: ret })
            }
        };
        Ok(ret)
    }

    async fn prepare_to_send(
        &self,
        dst: &Arc<KeyId>,
        default_key: &Arc<dyn KeyOption>,
        #[cfg(feature = "telemetry")] tag: u32,
    ) -> Result<AdnlPeers> {
        let src = self.overlay_key().unwrap_or(default_key).id();
        let peers = AdnlPeers::with_keys(src.clone(), dst.clone());
        #[cfg(feature = "telemetry")]
        self.update_stats(dst, tag, true)?;
        Ok(peers)
    }

    fn process_random_peers<T: NodeData>(
        &self,
        default_key: &Arc<dyn KeyOption>,
        mut peers: Vec<T>,
        convert: impl Fn(T) -> OverlayNodeInfo<NodeV1, NodeV2>,
    ) -> Result<Vec<OverlayNodeInfo<NodeV1, NodeV2>>> {
        let self_id = self.overlay_key().unwrap_or(default_key).id();
        let mut ret = Vec::new();
        log::trace!(target: TARGET, "-------- Got random peers:");
        while let Some(peer) = peers.pop() {
            let other_key = peer.get_key()?;
            if self_id.data() == other_key.id().data() {
                continue;
            }
            log::trace!(target: TARGET, "{}", other_key.id());
            if let Err(e) = OverlayUtils::verify_node(&self.overlay_id, &peer) {
                log::warn!(target: TARGET, "Error when verifying overlay peer: {e}");
                continue;
            }
            ret.push((&convert)(peer));
        }
        Ok(ret)
    }

    async fn purge_broadcasts(
        &self,
        last_one_time_broadcast: &mut Option<(BroadcastId, Instant)>,
        recv_one_time: &mut tokio::sync::mpsc::UnboundedReceiver<(BroadcastId, Instant)>,
        #[cfg(feature = "xp25")] last_repeated_broadcast: &mut Option<(BroadcastId, Instant)>,
        #[cfg(feature = "xp25")] recv_repeated: &mut tokio::sync::mpsc::UnboundedReceiver<(
            BroadcastId,
            Instant,
        )>,
    ) {
        #[cfg(feature = "xp25")]
        loop {
            if last_repeated_broadcast.is_none() && !recv_repeated.is_empty() {
                *last_repeated_broadcast = recv_repeated.recv().await;
            }
            let Some((bcast_id, start)) = last_repeated_broadcast else {
                break;
            };
            if start.elapsed().as_secs() >= Overlay::TIMEOUT_REPEATED_BROADCAST_SEC {
                self.owned_broadcasts.remove(bcast_id);
                #[cfg(feature = "telemetry")]
                self.stats_per_transfer.remove(bcast_id);
                *last_repeated_broadcast = None
            } else {
                break;
            }
        }
        loop {
            if last_one_time_broadcast.is_none() && !recv_one_time.is_empty() {
                *last_one_time_broadcast = recv_one_time.recv().await;
            }
            let Some((bcast_id, start)) = last_one_time_broadcast else {
                break;
            };
            if start.elapsed().as_secs() >= Overlay::TIMEOUT_ONE_TIME_BROADCAST_SEC {
                self.purge_broadcasts_count.fetch_add(1, Ordering::Relaxed);
                self.purge_broadcasts.push(*bcast_id);
                *last_one_time_broadcast = None
            } else {
                break;
            }
        }
        let upto = Self::MAX_BROADCAST_LOG;
        while self.purge_broadcasts_count.load(Ordering::Relaxed) > upto {
            if let Some(bcast_id) = self.purge_broadcasts.pop() {
                self.owned_broadcasts.remove(&bcast_id);
                #[cfg(feature = "telemetry")]
                self.stats_per_transfer.remove(&bcast_id);
            }
            self.purge_broadcasts_count.fetch_sub(1, Ordering::Relaxed);
        }
    }

    fn setup_broadcast_purge(
        &self,
        bcast_id: BroadcastId,
        #[cfg(feature = "xp25")] flags: u32,
    ) -> Result<()> {
        #[cfg(feature = "xp25")]
        if (flags & OverlayNode::FLAG_BCAST_REPEATED) != 0 {
            return self.queue_repeated_broadcasts.send((bcast_id, Instant::now())).map_err(|e| {
                error!("Error putting repeated broadcast into monitoring queue: {e}")
            });
        }
        self.queue_one_time_broadcasts
            .send((bcast_id, Instant::now()))
            .map_err(|e| error!("Error putting one time broadcast into monitoring queue: {e}"))
    }

    fn try_add_peer(&self, our_key: &Arc<KeyId>, peer: &Arc<KeyId>) -> Result<bool> {
        if self.adnl.have_peer(our_key, peer)? {
            self.known_peers.add(peer)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn update_neighbours(&self, n: u32) -> Result<()> {
        if self.overlay_type.is_private() {
            let n = min(self.known_peers.all().count(), n);
            loop {
                self.known_peers.all().random_set(&self.neighbours, n)?;
                if self.neighbours.count() >= n {
                    break Ok(());
                }
            }
        } else {
            self.known_peers.random_set(&self.neighbours, n)
            // self.random_peers.random_set(&self.neighbours, Some(&self.bad_peers), n)
        }
    }

    // fn update_random_peers(&self, n: u32) -> Result<()> {
    //     self.known_peers.random_set(&self.random_peers, Some(&self.bad_peers), n)?;
    //     self.update_neighbours(OverlayNode::MAX_OVERLAY_NEIGHBOURS)
    // }

    fn validate_certificate(&self, peer: &Arc<KeyId>, cert: &MemberCertificate) -> Result<()> {
        let utime = UnixTime::now() as u32;
        let (max_slaves, root_public_keys) =
            if let OverlayType::CertifiedMembers { max_slaves, root_public_keys, .. } =
                &self.overlay_type
            {
                (*max_slaves, root_public_keys)
            } else {
                fail!("Overlay type is not certificated members")
            };

        // 1) Expire check
        // "3" is a magic gap from cpp implementation
        if (cert.expire_at as u32) < utime - 3 {
            fail!("Certificate is expired, expire_at: {}, current time: {}", cert.expire_at, utime);
        }

        // 2) Slot (each root member has fixed number of slots)
        if cert.slot < 0 || cert.slot as usize >= max_slaves {
            fail!("Certificate has invalid slot: {}", cert.slot);
        }

        // 3) Issuer
        let issuer: Arc<dyn KeyOption> = (&cert.issued_by).try_into()?;
        let Some(slaves_info) = root_public_keys.get(issuer.id()) else {
            fail!("Certificate is issued by unknown member: {}", cert.issued_by);
        };

        // If previously used slot
        if let Some(guard) = slaves_info.get(&(cert.slot as u32)) {
            let slave_info = guard.val();

            // 4) Check for newer certificate at the same slot
            if (cert.expire_at as u32) < slave_info.expire_at {
                fail!("Certificate rejected, because we know of newer one at the same slot");
            }

            // 5) Check sender adnl id
            if (cert.expire_at as u32) == slave_info.expire_at {
                // In cpp code "if (node < el.node)", it is strange, because
                // it is not clear why node with bigger id can replace
                // node with smaller id, but node with smaller id can not.
                // I think it should be "!="
                if UInt256::from_slice(peer.data()) < UInt256::from_slice(slave_info.node_id.data())
                {
                    fail!("Certificate rejected, because we know another one at the same slot");
                }

                // 6) Check peer. Same peer means that certificate was already checked
                //    (slave_info is saved only after successfull check)
                if *peer == slave_info.node_id {
                    return Ok(());
                }
            }
        }

        // 6) Verify signature
        let data_to_sign = MemberCertificateId {
            node: AdnlShortId { id: UInt256::with_array(peer.data().clone()) },
            flags: cert.flags,
            slot: cert.slot,
            expire_at: cert.expire_at,
        }
        .into_boxed();
        issuer.verify(&serialize_boxed(&data_to_sign)?, &cert.signature)?;

        // 7) Create/update slot info
        add_unbound_object_to_map_with_update(slaves_info, cert.slot as u32, |_| {
            Ok(Some(SlaveInfo { node_id: peer.clone(), expire_at: cert.expire_at as u32 }))
        })?;
        Ok(())
    }

    #[cfg(feature = "telemetry")]
    fn get_per_transfer_stats(&self, bcast_id: &BroadcastId) -> Result<Arc<TransferStats>> {
        let stats = if let Some(stats) = self.stats_per_transfer.get(bcast_id) {
            stats.val().clone()
        } else {
            let stats = Arc::new(TransferStats {
                income: AtomicU64::new(0),
                passed: AtomicU64::new(0),
                resent: AtomicU64::new(0),
                counter: self.allocated.stats_transfer.clone().into(),
            });
            add_counted_object_to_map(&self.stats_per_transfer, bcast_id.clone(), || {
                self.telemetry
                    .stats_transfer
                    .update(self.allocated.stats_transfer.load(Ordering::Relaxed));
                Ok(stats.clone())
            })?;
            self.stats_per_transfer
                .get(bcast_id)
                .ok_or_else(|| error!("INTERNAL ERROR: Cannot count transfer statistics"))?
                .val()
                .clone()
        };
        Ok(stats)
    }

    #[cfg(feature = "telemetry")]
    fn print_stats(&self) -> Result<()> {
        let elapsed = self.start.elapsed().as_secs();
        if elapsed == 0 {
            // Too early to print stats
            return Ok(());
        }
        let messages_recv = self.messages_recv.load(Ordering::Relaxed);
        let messages_send = self.messages_send.load(Ordering::Relaxed);
        log::info!(
            target: TARGET,
            "------- OVERLAY STAT send {}: {messages_send} messages, {} messages/sec average load",
            self.overlay_id,
            messages_send / elapsed
        );
        for dst in self.stats_per_peer_send.iter() {
            log::info!(
                target: TARGET,
                "  -- OVERLAY STAT send {} to {}",
                self.overlay_id,
                dst.key()
            );
            for tag in dst.val().iter() {
                let count = tag.val().count.load(Ordering::Relaxed);
                if count / elapsed < 1 {
                    continue;
                }
                log::info!(
                    target: TARGET,
                    "  OVERLAY STAT send {} tag {:x}: {count}, {} per sec average load",
                    self.overlay_id,
                    tag.key(),
                    count / elapsed
                );
            }
        }
        log::info!(
            target: TARGET,
            "------- OVERLAY STAT recv {}: {messages_recv} messages, {} messages/sec average load",
            self.overlay_id,
            messages_recv / elapsed
        );
        for dst in self.stats_per_peer_recv.iter() {
            log::info!(
                target: TARGET,
                "  -- OVERLAY STAT recv {} from {}",
                self.overlay_id,
                dst.key()
            );
            for tag in dst.val().iter() {
                let count = tag.val().count.load(Ordering::Relaxed);
                if count / elapsed < 1 {
                    continue;
                }
                log::info!(
                    target: TARGET,
                    "  OVERLAY STAT recv {} tag {:x}: {count}, {} per sec average load",
                    self.overlay_id,
                    tag.key(),
                    count / elapsed
                );
            }
        }
        let mut inc = 0;
        let mut pas = 0;
        let mut res = 0;
        for transfer in self.stats_per_transfer.iter() {
            inc += transfer.val().income.load(Ordering::Relaxed);
            pas += transfer.val().passed.load(Ordering::Relaxed);
            res += transfer.val().resent.load(Ordering::Relaxed);
            /*
                log::info!(
                    target: TARGET,
                    "  ** OVERLAY STAT resend transfer {}: -> {} / {} -> {}",
                    base64_encode(transfer.key()),
                    transfer.val().income.load(Ordering::Relaxed),
                    transfer.val().passed.load(Ordering::Relaxed),
                    transfer.val().resent.load(Ordering::Relaxed)
                )
            */
        }
        log::info!(target: TARGET, "  ** OVERLAY STAT resend {inc} / {pas} -> {res}");
        let map = lockfree::map::Map::new();
        for transfer in self.owned_broadcasts.iter() {
            if let OwnedBroadcast::RecvFec(transfer) = transfer.val() {
                let Some((tag, len)) = transfer.get_telemetry_tag_and_len() else {
                    continue;
                };
                add_unbound_object_to_map(&map, tag, || {
                    Ok((AtomicU32::new(0), AtomicU32::new(0)))
                })?;
                if let Some(item) = map.get(&tag) {
                    let (cnt, total_len) = item.val();
                    cnt.fetch_add(1, Ordering::Relaxed);
                    total_len.fetch_add(len, Ordering::Relaxed);
                }
            }
        }
        for item in map.iter() {
            let (cnt, len) = item.val();
            let cnt = cnt.load(Ordering::Relaxed);
            let len = len.load(Ordering::Relaxed) / cnt;
            log::info!(
                target: TARGET,
                "  ** OVERLAY STAT resend by tag {:08x}: {cnt}, {len} bytes avg",
                item.key()
            )
        }
        Ok(())
    }

    #[cfg(feature = "telemetry")]
    fn update_stats(&self, dst: &Arc<KeyId>, tag: u32, is_send: bool) -> Result<()> {
        let stats = if is_send { &self.stats_per_peer_send } else { &self.stats_per_peer_recv };
        let stats = if let Some(stats) = stats.get(dst) {
            stats
        } else {
            add_unbound_object_to_map(stats, dst.clone(), || Ok(lockfree::map::Map::new()))?;
            if let Some(stats) = stats.get(dst) {
                stats
            } else {
                fail!(
                    "INTERNAL ERROR: cannot add overlay statistics for {}:{dst}",
                    self.overlay_id
                );
            }
        };
        let stats = if let Some(stats) = stats.val().get(&tag) {
            stats
        } else {
            add_counted_object_to_map(stats.val(), tag, || {
                let ret = PeerStats {
                    count: AtomicU64::new(0),
                    counter: self.allocated.stats_peer.clone().into(),
                };
                self.telemetry.stats_peer.update(self.allocated.stats_peer.load(Ordering::Relaxed));
                Ok(ret)
            })?;
            if let Some(stats) = stats.val().get(&tag) {
                stats
            } else {
                fail!(
                    "INTERNAL ERROR: cannot add overlay statistics for {}:{dst}:{tag}",
                    self.overlay_id
                );
            }
        };
        stats.val().count.fetch_add(1, Ordering::Relaxed);
        if is_send {
            self.messages_send.fetch_add(1, Ordering::Relaxed);
        } else {
            self.messages_recv.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }

    #[cfg(feature = "telemetry")]
    fn try_print_stats(&self) {
        let elapsed = self.start.elapsed().as_secs();
        let printed = self.print.load(Ordering::Relaxed);
        if elapsed > printed {
            if self
                .print
                .compare_exchange(printed, elapsed + 5, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                if let Err(e) = self.print_stats() {
                    log::warn!(target: TARGET, "Error printing overlay stats: {e}");
                }
            }
        }
    }
}

declare_counted!(
    struct ConsumerObject {
        object: Arc<dyn Subscriber>,
    }
);

/// Overlay parameters
pub struct OverlayParams<'a> {
    pub flags: u8,
    pub hops: Option<u8>,
    pub overlay_id: &'a Arc<OverlayShortId>,
    pub runtime: Option<tokio::runtime::Handle>,
}

impl<'a> OverlayParams<'a> {
    pub fn with_id_only(overlay_id: &'a Arc<OverlayShortId>) -> Self {
        Self { flags: 0, hops: None, overlay_id, runtime: None }
    }
}

/// Overlay Node
pub struct OverlayNode {
    adnl: Arc<AdnlNode>,
    consumers: lockfree::map::Map<Arc<OverlayShortId>, ConsumerObject>,
    node_key: Arc<dyn KeyOption>,
    options: Arc<AtomicU32>,
    overlays: lockfree::map::Map<Arc<OverlayShortId>, Arc<Overlay>>,
    quic: OnceLock<Arc<QuicNode>>,
    rldp: OnceLock<Arc<RldpNode>>,
    zero_state_file_hash: [u8; 32],
    #[cfg(feature = "telemetry")]
    telemetry: Arc<OverlayTelemetry>,
    allocated: Arc<OverlayAlloc>,
}

impl OverlayNode {
    pub const FLAG_BCAST_ANY_SENDER: u32 = 0x01;
    #[cfg(feature = "xp25")]
    pub const FLAG_BCAST_REPEATED: u32 = 0x80;

    const MAX_FAIL_COUNT: u8 = 3;
    const MAX_PEERS: u32 = 65536;
    const MAX_OVERLAY_NEIGHBOURS: u32 = 200;
    //    const MAX_OVERLAY_PEERS: u32 = 20;
    const MAX_SIZE_ORDINARY_BROADCAST: usize = 768;
    const MIN_BYTES_FEC_TWO_STEPS_BROADCAST: usize = 513;
    const MIN_NODES_FEC_TWO_STEPS_BROADCAST: u32 = 4;
    const PEER_BLOCK_LATENCY_SEC: u32 = 10;
    const TIMEOUT_GC_MS: u64 = 1000; // Milliseconds
    const TIMEOUT_PEERS_MS: u64 = 60000; // Milliseconds

    /// Constructor
    pub fn with_params(
        adnl: Arc<AdnlNode>,
        zero_state_file_hash: &[u8; 32],
        key_tag: usize,
    ) -> Result<Arc<Self>> {
        let node_key = adnl.key_by_tag(key_tag)?;
        #[cfg(feature = "telemetry")]
        let telemetry = OverlayTelemetry {
            consumers: adnl.add_metric("Alloc OVRL consumers"),
            overlays: adnl.add_metric("Alloc OVRL overlays"),
            peers: adnl.add_metric("Alloc OVRL peers"),
            recv_transfers: adnl.add_metric("Alloc OVRL recv transfers"),
            send_transfers: adnl.add_metric("Alloc OVRL send transfers"),
            stats_peer: adnl.add_metric("Alloc OVRL peer stats"),
            stats_transfer: adnl.add_metric("Alloc OVRL transfer stats"),
        };
        let allocated = OverlayAlloc {
            consumers: Arc::new(AtomicU64::new(0)),
            overlays: Arc::new(AtomicU64::new(0)),
            peers: Arc::new(AtomicU64::new(0)),
            recv_transfers: Arc::new(AtomicU64::new(0)),
            send_transfers: Arc::new(AtomicU64::new(0)),
            #[cfg(feature = "telemetry")]
            stats_peer: Arc::new(AtomicU64::new(0)),
            #[cfg(feature = "telemetry")]
            stats_transfer: Arc::new(AtomicU64::new(0)),
        };
        let ret = Self {
            adnl,
            consumers: lockfree::map::Map::new(),
            node_key,
            options: Arc::new(AtomicU32::new(0)),
            overlays: lockfree::map::Map::new(),
            quic: OnceLock::new(),
            rldp: OnceLock::new(),
            zero_state_file_hash: *zero_state_file_hash,
            #[cfg(feature = "telemetry")]
            telemetry: Arc::new(telemetry),
            allocated: Arc::new(allocated),
        };
        Ok(Arc::new(ret))
    }

    /// Add overlay data consumer
    pub fn add_consumer(
        &self,
        overlay_id: &Arc<OverlayShortId>,
        consumer: Arc<dyn Subscriber>,
    ) -> Result<bool> {
        log::debug!(target: TARGET, "Add consumer {} to overlay", overlay_id);
        add_counted_object_to_map(&self.consumers, overlay_id.clone(), || {
            let ret = ConsumerObject {
                object: consumer.clone(),
                counter: self.allocated.consumers.clone().into(),
            };
            #[cfg(feature = "telemetry")]
            self.telemetry.consumers.update(self.allocated.consumers.load(Ordering::Relaxed));
            Ok(ret)
        })
    }

    /// Add overlay for local workchain
    pub fn add_local_workchain_overlay(&self, params: OverlayParams) -> Result<bool> {
        self.add_overlay(OverlayType::Public, params)
    }

    /// Add overlay for other workchain
    pub fn add_other_workchain_overlay(&self, mut params: OverlayParams) -> Result<bool> {
        params.flags |= Overlay::FLAG_OVERLAY_OTHER_WORKCHAIN;
        self.add_overlay(OverlayType::Public, params)
    }

    /// Add private_overlay
    pub fn add_private_overlay(
        &self,
        params: OverlayParams,
        overlay_key: &Arc<dyn KeyOption>,
        peers: &[Arc<KeyId>],
        use_quic: bool,
    ) -> Result<bool> {
        let overlay_type = OverlayType::Private { key: overlay_key.clone(), use_quic };
        self.add_typed_private_overlay(overlay_type, params, peers)
    }

    /// Add private peers to ADNL layer
    pub fn add_private_peers_to_adnl(
        &self,
        local_adnl_key: &Arc<KeyId>,
        peers: Vec<(IpAddress, Option<IpAddress>, Arc<dyn KeyOption>)>,
    ) -> Result<Vec<Arc<KeyId>>> {
        let mut ret = Vec::new();
        for (ip, quic_ip, key) in peers {
            if let Some(peer) = self.adnl.add_peer(local_adnl_key, &ip, quic_ip.as_ref(), &key)? {
                ret.push(peer)
            }
        }
        Ok(ret)
    }

    /// Add private peers to the overlay
    pub fn add_private_peers_to_overlay(
        &self,
        overlay_id: &Arc<OverlayShortId>,
        peers: &[Arc<KeyId>],
    ) -> Result<usize> {
        self.add_peers_to_overlay(overlay_id, peers, "Cannot get overlay to add peers")
    }

    /// Add public overlay peer
    pub fn add_public_peer<N1: Borrow<NodeV1>, N2: Borrow<NodeV2>>(
        &self,
        peer_ip_address: &IpAddress,
        peer: &OverlayNodeInfo<N1, N2>,
        overlay_id: &Arc<OverlayShortId>,
    ) -> Result<Option<Arc<KeyId>>> {
        let overlay = self.get_overlay(overlay_id, "Trying add peer to unknown public overlay")?;
        if overlay.overlay_type.is_private() {
            fail!("Trying to add public peer to private overlay {overlay_id}")
        }
        let (key, verify) = match peer {
            OverlayNodeInfo::V1(peer) => {
                if overlay.overlay_type.has_certified_members() {
                    fail!(
                        "Trying to add peer without certificate \
                        to semiprivate overlay {overlay_id}"
                    )
                } else {
                    let peer = peer.borrow();
                    (peer.get_key()?, OverlayUtils::verify_node(overlay_id, peer))
                }
            }
            OverlayNodeInfo::V2(peer) => {
                let peer = peer.borrow();
                let key = peer.get_key()?;
                if overlay.overlay_type.has_certified_members() {
                    let cert = match &peer.certificate {
                        MemberCertificateBoxed::Overlay_MemberCertificate(cert) => Some(cert),
                        _ => None,
                    };
                    overlay.check_peer(key.id(), cert)?;
                }
                (key, OverlayUtils::verify_node(overlay_id, peer))
            }
        };
        if let Err(e) = verify {
            log::warn!(target: TARGET, "Error when verifying overlay peer {}: {e}", key.id());
            return Ok(None);
        }
        let Some(ret) = self.adnl.add_peer(self.node_key.id(), peer_ip_address, None, &key)? else {
            return Ok(None);
        };
        overlay.known_peers.add(&ret)?;
        // if overlay.random_peers.count() < Self::MAX_OVERLAY_PEERS {
        //     overlay.random_peers.put(ret.clone())?;
        // }
        if overlay.neighbours.count() < Self::MAX_OVERLAY_NEIGHBOURS {
            overlay.neighbours.put(ret.clone())?;
        }
        add_counted_object_to_map_with_update(&overlay.nodes, ret.clone(), |old_node| {
            if let Some(old_node) = old_node {
                let (old_version, new_version) = match peer {
                    OverlayNodeInfo::V1(peer) => {
                        (old_node.v1.as_ref().map(|node| node.version), peer.borrow().version)
                    }
                    OverlayNodeInfo::V2(peer) => {
                        (old_node.v2.as_ref().map(|node| node.version), peer.borrow().version)
                    }
                };
                if let Some(old_version) = old_version {
                    if old_version >= new_version {
                        return Ok(None);
                    }
                }
            }
            let ret = match peer {
                OverlayNodeInfo::V1(peer) => NodeObject {
                    v1: Some(Arc::new(peer.borrow().clone())),
                    v2: old_node.map(|node| node.v2.clone()).flatten(),
                    counter: self.allocated.peers.clone().into(),
                },
                OverlayNodeInfo::V2(peer) => NodeObject {
                    v1: old_node.map(|node| node.v1.clone()).flatten(),
                    v2: Some(Arc::new(peer.borrow().clone())),
                    counter: self.allocated.peers.clone().into(),
                },
            };
            #[cfg(feature = "telemetry")]
            self.telemetry.peers.update(self.allocated.peers.load(Ordering::Relaxed));
            Ok(Some(ret))
        })?;
        Ok(Some(ret))
    }

    /// Add semiprivate overlay
    pub fn add_semiprivate_overlay(
        &self,
        params: OverlayParams,
        overlay_key: Option<&Arc<dyn KeyOption>>,
        root_adnl_ids: &[Arc<KeyId>],
        root_public_keys: &[Arc<KeyId>], // Can be empty if overlay created by non-validator
        certificate: Option<MemberCertificate>,
        max_slaves: usize,
        use_quic: bool,
    ) -> Result<bool> {
        let root_adnl_set: HashSet<Arc<KeyId>> = root_adnl_ids.iter().cloned().collect();
        let mut root_public_keys_map = HashMap::with_capacity(root_public_keys.len());
        for pk in root_public_keys {
            root_public_keys_map.insert(pk.clone(), lockfree::map::Map::new());
        }
        let overlay_type = OverlayType::CertifiedMembers {
            bcast_prefix: OverlayUtils::calc_message_prefix(params.overlay_id)?,
            certificate,
            key: overlay_key.cloned(),
            max_slaves,
            root_adnl_ids: root_adnl_set,
            root_public_keys: root_public_keys_map,
            use_quic,
        };
        self.add_typed_private_overlay(overlay_type, params, root_adnl_ids)
    }

    /// Broadcast message
    pub async fn broadcast(
        &self,
        overlay_id: &Arc<OverlayShortId>,
        data: &TaggedByteSlice<'_>,
        src_key: Option<&Arc<dyn KeyOption>>,
        flags: u32,
        method: AdnlSendMethod,
    ) -> Result<BroadcastSendInfo> {
        log::trace!(target: TARGET, "Broadcast {} bytes", data.object.len());
        let overlay = self.get_overlay(overlay_id, "Trying broadcast to unknown overlay")?;
        let mut ctx = BroadcastSendContext {
            data,
            flags,
            overlay: &overlay,
            src_key: self.calc_src_key_for_broadcast(&overlay, src_key),
            src_adnl_key_id: overlay.overlay_key().unwrap_or(&self.node_key).id(),
        };
        if let AdnlSendMethod::Fast = &method {
            if data.object.len() > Self::MAX_SIZE_ORDINARY_BROADCAST {
                return BroadcastFecProtocol::for_send(&ctx).send(ctx).await;
            }
        }
        // Ignore ANY_SENDER flag for non-FEC broadcasts
        ctx.flags &= !Self::FLAG_BCAST_ANY_SENDER;
        match method {
            AdnlSendMethod::Fast => BroadcastSimpleProtocol.send(ctx).await,
            AdnlSendMethod::Safe => BroadcastStreamSimpleProtocol.send(ctx).await,
        }
    }

    /// Two-step message broadcast
    pub async fn broadcast_twostep(
        &self,
        overlay_id: &Arc<OverlayShortId>,
        data: &TaggedByteSlice<'_>,
        src_key: Option<&Arc<dyn KeyOption>>,
        flags: u32,
        extra: Vec<u8>,
    ) -> Result<BroadcastSendInfo> {
        log::trace!(target: TARGET, "Two-step broadcast {} bytes", data.object.len());
        let overlay =
            self.get_overlay(overlay_id, "Trying two-step broadcast to unknown overlay")?;
        let ctx = BroadcastSendContext {
            data,
            flags,
            overlay: &overlay,
            src_key: self.calc_src_key_for_broadcast(&overlay, src_key),
            src_adnl_key_id: overlay.overlay_key().unwrap_or(&self.node_key).id(),
        };
        let neighbours = overlay.calc_broadcast_twostep_neighbours();
        let big_data = data.object.len() >= Self::MIN_BYTES_FEC_TWO_STEPS_BROADCAST;
        let reliable = big_data || overlay.overlay_type.quic_requested();
        if big_data && (neighbours >= Self::MIN_NODES_FEC_TWO_STEPS_BROADCAST) {
            BroadcastTwostepFecProtocol::for_send(data.object, neighbours, extra)?.send(ctx).await
        } else {
            BroadcastTwostepSimpleProtocol::for_send(reliable, extra).send(ctx).await
        }
    }

    /// Calculate overlay ID for public overlay
    pub fn calc_overlay_id(&self, workchain: i32, shard: i64) -> Result<OverlayId> {
        OverlayUtils::calc_overlay_id(workchain, shard, &self.zero_state_file_hash)
    }

    /// Calculate overlay short ID for public overlay
    pub fn calc_overlay_short_id(&self, workchain: i32, shard: i64) -> Result<Arc<OverlayShortId>> {
        OverlayUtils::calc_overlay_short_id(workchain, shard, &self.zero_state_file_hash)
    }

    /// Delete private_overlay
    pub fn delete_private_overlay(&self, overlay_id: &Arc<OverlayShortId>) -> Result<bool> {
        self.delete_overlay(overlay_id, true)
    }

    /// Delete public_overlay
    pub fn delete_public_overlay(&self, overlay_id: &Arc<OverlayShortId>) -> Result<bool> {
        self.delete_overlay(overlay_id, false)
    }

    /// Delete private overlay peers
    pub fn delete_private_peers(
        &self,
        local_key: &Arc<KeyId>,
        peers: &[Arc<KeyId>],
    ) -> Result<bool> {
        let mut ret = false;
        for peer in peers {
            ret = self.adnl.delete_peer(local_key, peer)? || ret
        }
        Ok(ret)
    }

    /// Delete public overlay peer
    pub fn delete_public_peer(
        &self,
        peer: &Arc<KeyId>,
        overlay_id: &Arc<OverlayShortId>,
    ) -> Result<bool> {
        let overlay =
            self.get_overlay(overlay_id, "Trying to delete peer from unknown public overlay")?;
        if overlay.overlay_type.is_private() {
            fail!("Trying to delete public peer from private overlay {}", overlay_id)
        }
        if !overlay.known_peers.block(peer)? {
            return Ok(false);
        }
        // if overlay.random_peers.contains(peer) {
        //     overlay.update_random_peers(Self::MAX_OVERLAY_PEERS)?
        // }
        if overlay.neighbours.contains(peer) {
            overlay.update_neighbours(1)?
        }
        // DO NOT DELETE from ADNL, because it may be shared between overlays
        // self.adnl.delete_peer(self.node_key.id(), peer)
        Ok(true)
    }

    /// Get debug trace
    pub fn get_debug_trace(&self, overlay_id: &Arc<OverlayShortId>) -> Result<u32> {
        let overlay = self.get_overlay(overlay_id, "Getting trace from unknown overlay")?;
        Ok(overlay.debug_trace.load(Ordering::Relaxed))
    }

    /// Get locally cached random peers
    pub fn get_cached_random_peers(
        &self,
        dst: &AddressCache,
        overlay_id: &Arc<OverlayShortId>,
        n: u32,
    ) -> Result<()> {
        let overlay =
            self.get_overlay(overlay_id, "Getting cached random peers from unknown overlay")?;
        overlay.known_peers.random_set(dst, n)
    }

    /// Get query prefix
    pub fn get_query_prefix(&self, overlay_id: &Arc<OverlayShortId>) -> Result<Vec<u8>> {
        let overlay = self.get_overlay(overlay_id, "Getting query prefix of unknown overlay")?;
        Ok(overlay.query_prefix.clone())
    }

    /// Get signed node
    pub fn get_signed_node(
        &self,
        overlay_id: &Arc<OverlayShortId>,
        v2: bool,
    ) -> Result<OverlayNodeInfo<NodeV1, NodeV2>> {
        let overlay = self.get_overlay(
            overlay_id,
            format!("Signing local {} node for unknown overlay", Self::version_str(v2)).as_str(),
        )?;
        overlay.get_signed_node(&self.node_key, v2)
    }

    /// Check whether peer is known
    pub fn have_peer(&self, local_key: Option<&Arc<KeyId>>, peer: &Arc<KeyId>) -> Result<bool> {
        let local_key = local_key.unwrap_or(self.node_key.id());
        self.adnl.have_peer(local_key, peer)
    }

    /// Send message via ADNL
    pub async fn message(
        &self,
        dst: &Arc<KeyId>,
        data: &TaggedByteSlice<'_>,
        overlay_id: &Arc<OverlayShortId>,
    ) -> Result<()> {
        #[cfg(feature = "telemetry")]
        let tag = data.tag;
        let (peers, data) = self
            .prepare_to_send_message(
                dst,
                data,
                overlay_id,
                "Sending ADNL message to unknown overlay",
            )
            .await?;
        self.adnl
            .send_custom(
                &TaggedByteSlice {
                    object: &data,
                    #[cfg(feature = "telemetry")]
                    tag,
                },
                &peers,
            )
            .await
    }

    /// Send message via QUIC
    pub async fn message_via_quic(
        &self,
        dst: &Arc<KeyId>,
        data: &TaggedByteSlice<'_>,
        overlay_id: &Arc<OverlayShortId>,
    ) -> Result<()> {
        let Some(quic) = self.quic.get() else {
            fail!("QUIC sender is not set in overlay node");
        };
        let (peers, data) = self
            .prepare_to_send_message(
                dst,
                data,
                overlay_id,
                "Sending QUIC message to unknown overlay",
            )
            .await?;
        quic.message(data, Some(&self.adnl), &peers).await?;
        Ok(())
    }

    /// Send message via RLDP
    pub async fn message_via_rldp(
        &self,
        dst: &Arc<KeyId>,
        data: &TaggedByteSlice<'_>,
        overlay_id: &Arc<OverlayShortId>,
        v2: bool,
        roundtrip: Option<u64>,
    ) -> Result<u64> {
        #[cfg(feature = "telemetry")]
        let tag = data.tag;
        let (peers, data) = self
            .prepare_to_send_message(
                dst,
                data,
                overlay_id,
                "Sending RLDP message to unknown overlay",
            )
            .await?;
        let Some(rldp) = self.rldp.get() else {
            fail!("RLDP sender is not set in overlay node");
        };
        rldp.message(
            &TaggedByteSlice {
                object: &data,
                #[cfg(feature = "telemetry")]
                tag,
            },
            &peers,
            v2,
            roundtrip,
        )
        .await
    }

    /// Send query via ADNL
    pub async fn query(
        &self,
        dst: &Arc<KeyId>,
        query: &TaggedTlObject,
        overlay_id: &Arc<OverlayShortId>,
        timeout_ms: Option<u64>,
    ) -> Result<Option<TLObject>> {
        let (peers, overlay) = self
            .prepare_to_send(
                dst,
                overlay_id,
                "Sending ADNL query to unknown overlay",
                #[cfg(feature = "telemetry")]
                query.tag,
            )
            .await?;
        self.adnl.query_with_prefix(Some(&overlay.query_prefix), query, &peers, timeout_ms).await
    }

    /// Send query via QUIC
    pub async fn query_via_quic(
        &self,
        dst: &Arc<KeyId>,
        query: &TaggedTlObject,
        overlay_id: &Arc<OverlayShortId>,
        timeout_ms: Option<u64>,
    ) -> Result<Option<TLObject>> {
        let Some(quic) = self.quic.get() else {
            fail!("QUIC sender is not set in overlay node");
        };
        let (peers, overlay) = self
            .prepare_to_send(
                dst,
                overlay_id,
                "Sending QUIC query to unknown overlay",
                #[cfg(feature = "telemetry")]
                query.tag,
            )
            .await?;
        let mut data = overlay.query_prefix.clone();
        serialize_boxed_append(&mut data, &query.object)?;
        match quic.query(data, Some(&self.adnl), &peers, timeout_ms).await? {
            Some(raw) => Ok(Some(deserialize_boxed(&raw)?)),
            None => Ok(None),
        }
    }

    /// Send query via RLDP
    pub async fn query_via_rldp(
        &self,
        dst: &Arc<KeyId>,
        data: &TaggedByteSlice<'_>,
        overlay_id: &Arc<OverlayShortId>,
        max_answer_size: Option<u64>,
        v2: bool,
        roundtrip: Option<u64>,
    ) -> Result<(Option<Vec<u8>>, u64)> {
        let (peers, _) = self
            .prepare_to_send(
                dst,
                overlay_id,
                "Sending RLDP query to unknown overlay",
                #[cfg(feature = "telemetry")]
                data.tag,
            )
            .await?;
        let Some(rldp) = self.rldp.get() else {
            fail!("RLDP sender is not set in overlay node");
        };
        rldp.query(data, max_answer_size, &peers, v2, roundtrip).await
    }

    /// Enable/disable broadcast retransmit
    pub fn set_broadcast_retransmit(&self, enabled: bool) {
        if enabled {
            self.options
                .fetch_and(!Overlay::OPTION_DISABLE_BROADCAST_RETRANSMIT, Ordering::Relaxed);
        } else {
            self.options.fetch_or(Overlay::OPTION_DISABLE_BROADCAST_RETRANSMIT, Ordering::Relaxed);
        }
    }

    /// Set RLDP sender
    pub fn set_quic(&self, quic: Arc<QuicNode>) -> Result<()> {
        self.quic.set(quic).map_err(|_| error!("QUIC sender already set in overlay node"))
    }

    pub fn set_rldp(&self, rldp: Arc<RldpNode>) -> Result<()> {
        self.rldp.set(rldp).map_err(|_| error!("RLDP sender already set in overlay node"))
    }

    /// Statistics
    #[cfg(feature = "telemetry")]
    pub fn stats(&self) -> Result<()> {
        for overlay in self.overlays.iter() {
            overlay.val().print_stats()?
        }
        Ok(())
    }

    /// Wait for broadcast
    pub async fn wait_for_broadcast(
        &self,
        overlay_id: &Arc<OverlayShortId>,
    ) -> Result<Option<BroadcastRecvInfo>> {
        let overlay = self.get_overlay(overlay_id, "Waiting for broadcast in unknown overlay")?;
        if (overlay.flags & Overlay::FLAG_OVERLAY_OTHER_WORKCHAIN) != 0 {
            fail!("Waiting for broadcast in overlay from other workchain")
        }
        overlay.received_rawbytes.pop().await
    }

    /// Wait for catchain
    pub async fn wait_for_catchain(
        &self,
        overlay_id: &Arc<OverlayShortId>,
    ) -> Result<Option<(CatchainBlockUpdate, CatchainData, Arc<KeyId>)>> {
        self.get_overlay(overlay_id, "Waiting for catchain in unknown overlay")?
            .received_catchain
            .as_ref()
            .ok_or_else(|| error!("Waiting for catchain in public overlay {}", overlay_id))?
            .pop()
            .await
    }

    /// Wait for peers
    pub async fn wait_for_peers(
        &self,
        overlay_id: &Arc<OverlayShortId>,
    ) -> Result<Option<Vec<OverlayNodeInfo<NodeV1, NodeV2>>>> {
        self.get_overlay(overlay_id, "Waiting for peers in unknown overlay")?
            .received_peers
            .pop()
            .await
    }

    fn add_overlay(&self, overlay_type: OverlayType, params: OverlayParams) -> Result<bool> {
        log::debug!(target: TARGET, "Add overlay {} to node", params.overlay_id);
        if !overlay_type.is_public()
            && ((params.flags & Overlay::FLAG_OVERLAY_OTHER_WORKCHAIN) != 0)
        {
            fail!("Cannot create non-public overlay {} for other workchain", params.overlay_id)
        }
        let received_catchain =
            if overlay_type.is_private() { Some(AsyncReceiver::new()) } else { None };
        let (sender_one_time, mut receiver_one_time) = tokio::sync::mpsc::unbounded_channel();
        #[cfg(feature = "xp25")]
        let (sender_repeated, mut receiver_repeated) = tokio::sync::mpsc::unbounded_channel();
        let policy = BadPolicy {
            amnesty: Self::MAX_FAIL_COUNT,
            latency: Self::PEER_BLOCK_LATENCY_SEC,
            penalty: 1,
            to_block: Self::MAX_FAIL_COUNT,
        };
        let quic = if overlay_type.quic_requested() { self.quic.get().cloned() } else { None };
        let overlay = Overlay {
            adnl: self.adnl.clone(),
            rldp: self.rldp.get().cloned(),
            quic,
            flags: params.flags,
            hops: params.hops,
            known_peers: AddressCacheWithBads::with_params(Self::MAX_PEERS, policy),
            message_prefix: overlay_type.calc_message_prefix(params.overlay_id)?,
            query_prefix: overlay_type.calc_query_prefix(params.overlay_id)?,
            neighbours: AddressCache::with_limit(Self::MAX_OVERLAY_NEIGHBOURS),
            nodes: lockfree::map::Map::new(),
            options: self.options.clone(),
            overlay_id: params.overlay_id.clone(),
            overlay_type,
            owned_broadcasts: lockfree::map::Map::new(),
            pending_peers: lockfree::queue::Queue::new(),
            purge_broadcasts: lockfree::queue::Queue::new(),
            purge_broadcasts_count: AtomicU32::new(0),
            queue_one_time_broadcasts: sender_one_time,
            #[cfg(feature = "xp25")]
            queue_repeated_broadcasts: sender_repeated,
            // random_peers: AddressCache::with_limit(Self::MAX_OVERLAY_PEERS),
            received_catchain,
            received_peers: AsyncReceiver::new(),
            received_rawbytes: AsyncReceiver::new(),
            #[cfg(feature = "telemetry")]
            start: Instant::now(),
            #[cfg(feature = "telemetry")]
            print: AtomicU64::new(0),
            #[cfg(feature = "telemetry")]
            messages_recv: AtomicU64::new(0),
            #[cfg(feature = "telemetry")]
            messages_send: AtomicU64::new(0),
            #[cfg(feature = "telemetry")]
            stats_per_peer_recv: lockfree::map::Map::new(),
            #[cfg(feature = "telemetry")]
            stats_per_peer_send: lockfree::map::Map::new(),
            #[cfg(feature = "telemetry")]
            stats_per_transfer: lockfree::map::Map::new(),
            #[cfg(feature = "telemetry")]
            telemetry: self.telemetry.clone(),
            allocated: self.allocated.clone(),
            debug_trace: AtomicU32::new(0),
            counter: self.allocated.overlays.clone().into(),
        };
        #[cfg(feature = "telemetry")]
        self.telemetry.overlays.update(self.allocated.overlays.load(Ordering::Relaxed));
        overlay.update_neighbours(Self::MAX_OVERLAY_NEIGHBOURS)?;
        let overlay = Arc::new(overlay);

        let added = add_counted_object_to_map(&self.overlays, params.overlay_id.clone(), || {
            Ok(overlay.clone())
        })?;
        if added {
            let default_key = self.node_key.clone();
            let overlay = self.get_overlay(params.overlay_id, "Cannot add overlay")?;
            let handle = params.runtime.unwrap_or_else(tokio::runtime::Handle::current);
            handle.spawn(async move {
                let local_adnl_key = if overlay.overlay_type.is_private() {
                    Some(overlay.overlay_key().unwrap_or(&default_key).id())
                } else {
                    None
                };
                let mut timeout_peers = 0;
                let mut last_one_time_broadcast = None;
                let mut next_ping = None;
                #[cfg(feature = "xp25")]
                let mut last_repeated_broadcast = None;
                while Arc::strong_count(&overlay) > 1 {
                    overlay
                        .purge_broadcasts(
                            &mut last_one_time_broadcast,
                            &mut receiver_one_time,
                            #[cfg(feature = "xp25")]
                            &mut last_repeated_broadcast,
                            #[cfg(feature = "xp25")]
                            &mut receiver_repeated,
                        )
                        .await;
                    timeout_peers += Self::TIMEOUT_GC_MS;
                    if timeout_peers > Self::TIMEOUT_PEERS_MS {
                        // let result = if overlay.overlay_type.is_private() {
                        //     overlay.update_neighbours(1)
                        // } else {
                        //     overlay.update_random_peers(1)
                        // };
                        // if let Err(e) = result {
                        if let Err(e) = overlay.update_neighbours(1) {
                            log::error!(target: TARGET, "Error: {}", e)
                        }
                        if let Some(key) = &local_adnl_key {
                            let mut pending = Vec::new();
                            while let Some(peer) = overlay.pending_peers.pop() {
                                pending.push(peer);
                            }
                            for peer in pending {
                                match overlay.try_add_peer(key, &peer) {
                                    Ok(true) => {
                                        log::info!(
                                            target: TARGET,
                                            "Resolved pending peer {peer} in overlay {}",
                                            overlay.overlay_id
                                        );
                                        continue;
                                    }
                                    Err(e) => log::warn!(
                                        target: TARGET,
                                        "Error resolving pending peer {peer} in overlay {}: {e}",
                                        overlay.overlay_id
                                    ),
                                    _ => (),
                                }
                                overlay.pending_peers.push(peer);
                            }
                        }
                        timeout_peers = 0;
                    }
                    let peer = if let Some(iter) = next_ping.as_mut() {
                        overlay.known_peers.all().next(iter)
                    } else {
                        let (iter, peer) = overlay.known_peers.all().first();
                        next_ping.replace(iter);
                        peer
                    };
                    let sleep_ms = if let Some(peer) = peer {
                        let query_start = std::time::Instant::now();
                        let ping_task = overlay.ping_peer(&default_key, &peer, Self::TIMEOUT_GC_MS);
                        let (ping_res, peers_res) = if overlay.overlay_type.is_private() {
                            (ping_task.await, None)
                        } else {
                            let v2 = overlay.overlay_type.has_certified_members();
                            let peers_task = overlay.get_random_peers(
                                &peer,
                                &default_key,
                                v2,
                                Some(Self::TIMEOUT_GC_MS),
                            );
                            let (ping_res, peers_res) = tokio::join!(ping_task, peers_task);
                            (ping_res, Some(peers_res))
                        };
                        if let Err(e) = ping_res {
                            log::info!(target: TARGET, "Error in overlay ping {peer}: {e}");
                        }
                        if let Some(Err(e)) = peers_res {
                            log::info!(target: TARGET, "Error get random peers from {peer}: {e}");
                        }
                        let elapsed_ms = query_start.elapsed().as_millis() as u64;
                        Self::TIMEOUT_GC_MS.saturating_sub(elapsed_ms)
                    } else {
                        next_ping = None;
                        Self::TIMEOUT_GC_MS
                    };
                    if sleep_ms > 0 {
                        tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
                    }
                }
            });
        }
        Ok(added)
    }

    fn add_peers_to_overlay(
        &self,
        overlay_id: &Arc<OverlayShortId>,
        peers: &[Arc<KeyId>],
        msg: &str,
    ) -> Result<usize> {
        let overlay = self.get_overlay(overlay_id, msg)?;
        let our_key = overlay.overlay_key().unwrap_or(&self.node_key).id();
        let mut ret = 0;
        for peer in peers {
            if peer == our_key {
                continue;
            }
            if overlay.try_add_peer(our_key, peer)? {
                ret += 1;
            } else {
                log::info!(
                    target: TARGET,
                    "Peer {peer} has no ADNL address yet in overlay {}, queued for later",
                    overlay.overlay_id
                );
                overlay.pending_peers.push(peer.clone());
            }
        }
        overlay.update_neighbours(Self::MAX_OVERLAY_NEIGHBOURS)?;
        Ok(ret)
    }

    fn add_typed_private_overlay(
        &self,
        overlay_type: OverlayType,
        params: OverlayParams,
        peers: &[Arc<KeyId>],
    ) -> Result<bool> {
        let overlay_id = params.overlay_id;
        if self.add_overlay(overlay_type, params)? {
            self.add_peers_to_overlay(overlay_id, peers, "Cannot add the private overlay")?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn calc_src_key_for_broadcast<'a>(
        &'a self,
        overlay: &'a Overlay,
        src_key: Option<&'a Arc<dyn KeyOption>>,
    ) -> &'a Arc<dyn KeyOption> {
        if let Some(source) = src_key {
            source
        } else if let Some(key) = overlay.overlay_key() {
            key
        } else {
            &self.node_key
        }
    }

    fn check_overlay_adnl_address(&self, overlay: &Arc<Overlay>, adnl: &Arc<KeyId>) -> bool {
        let local_adnl = overlay.overlay_key().unwrap_or(&self.node_key).id();
        if local_adnl != adnl {
            log::debug!(
                target: TARGET,
                "Bad destination ADNL address in overlay {}: expected {local_adnl}, got {adnl}",
                overlay.overlay_id
            );
            false
        } else {
            true
        }
    }

    fn check_fec_broadcast_message(message: &BroadcastFec) -> Result<()> {
        const CONSTRAINTS: Constraints = Constraints {
            data_size: 32 << 20, // NOTE: 32 MB is the max reasonable data size due to
                                 // the default decoder block count assumption.
        };
        CONSTRAINTS.check_fec_type(&message.fec)?;
        CONSTRAINTS.check_data_size(message.data_size)?;
        Constraints::check_seqno(message.seqno as u32)
    }

    fn delete_overlay(&self, overlay_id: &Arc<OverlayShortId>, is_private: bool) -> Result<bool> {
        let type_of = if is_private { "private" } else { "public" };
        log::info!(target: TARGET, "Delete {} overlay {}", type_of, overlay_id);
        if let Some(overlay) = self.overlays.get(overlay_id) {
            let overlay = overlay.val();
            if is_private {
                if !overlay.overlay_type.is_private() {
                    fail!("Try to delete non-private overlay {} as private", overlay_id)
                }
                if let Some(received_catchain) = overlay.received_catchain.as_ref() {
                    received_catchain.stop();
                }
            } else if overlay.overlay_type.is_private() {
                fail!("Try to delete private overlay {} as public", overlay_id)
            }
            overlay.received_peers.stop();
            overlay.received_rawbytes.stop();
            self.overlays.remove(overlay_id);
            log::debug!(target: TARGET, "Delete consumer {} from {} overlay", overlay_id, type_of);
            self.consumers.remove(overlay_id);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn get_overlay(&self, overlay_id: &Arc<OverlayShortId>, msg: &str) -> Result<Arc<Overlay>> {
        let ret = self
            .overlays
            .get(overlay_id)
            .ok_or_else(|| error!("{} {}", msg, overlay_id))?
            .val()
            .clone();
        Ok(ret)
    }

    async fn prepare_to_send(
        &self,
        dst: &Arc<KeyId>,
        overlay_id: &Arc<OverlayShortId>,
        err_msg: &str,
        #[cfg(feature = "telemetry")] tag: u32,
    ) -> Result<(AdnlPeers, Arc<Overlay>)> {
        let overlay = self.get_overlay(overlay_id, err_msg)?;
        let peers = overlay
            .prepare_to_send(
                dst,
                &self.node_key,
                #[cfg(feature = "telemetry")]
                tag,
            )
            .await?;
        Ok((peers, overlay))
    }

    async fn prepare_to_send_message(
        &self,
        dst: &Arc<KeyId>,
        data: &TaggedByteSlice<'_>,
        overlay_id: &Arc<OverlayShortId>,
        err_msg: &str,
    ) -> Result<(AdnlPeers, Vec<u8>)> {
        let (peers, overlay) = self
            .prepare_to_send(
                dst,
                overlay_id,
                err_msg,
                #[cfg(feature = "telemetry")]
                data.tag,
            )
            .await?;
        let mut buf = overlay.message_prefix.clone();
        buf.extend_from_slice(data.object);
        Ok((peers, buf))
    }

    fn process_get_random_peers<T: NodeData>(
        &self,
        overlay: &Overlay,
        peers: Vec<T>,
        convert: impl Fn(T) -> OverlayNodeInfo<NodeV1, NodeV2>,
        v2: bool,
    ) -> Result<Option<Nodes>> {
        log::trace!(target: TARGET, "Got random peers {} request", Self::version_str(v2));
        let peers = overlay.process_random_peers(&self.node_key, peers, convert)?;
        overlay.received_peers.push(peers);
        if (overlay.flags & Overlay::FLAG_OVERLAY_OTHER_WORKCHAIN) != 0 {
            Ok(None)
        } else {
            Ok(Some(overlay.prepare_random_peers(&self.node_key, v2)?))
        }
    }

    fn version_str(v2: bool) -> &'static str {
        if v2 {
            "V2"
        } else {
            "V1"
        }
    }
}

#[async_trait::async_trait]
impl Subscriber for OverlayNode {
    #[cfg(feature = "telemetry")]
    async fn poll(&self, _start: &Arc<Instant>) {
        self.telemetry.consumers.update(self.allocated.consumers.load(Ordering::Relaxed));
        self.telemetry.overlays.update(self.allocated.overlays.load(Ordering::Relaxed));
        self.telemetry.peers.update(self.allocated.peers.load(Ordering::Relaxed));
        self.telemetry.recv_transfers.update(self.allocated.recv_transfers.load(Ordering::Relaxed));
        self.telemetry.send_transfers.update(self.allocated.send_transfers.load(Ordering::Relaxed));
        self.telemetry.stats_peer.update(self.allocated.stats_peer.load(Ordering::Relaxed));
        self.telemetry.stats_transfer.update(self.allocated.stats_transfer.load(Ordering::Relaxed));
        for overlay in self.overlays.iter() {
            overlay.val().try_print_stats();
        }
    }

    async fn try_consume_custom(&self, data: &[u8], peers: &AdnlPeers) -> Result<bool> {
        let Ok((prefix, suffix_offset)) = deserialize_boxed_with_suffix(&data) else {
            return Ok(false); // Not a TL-prefixed message — let next subscriber try
        };
        let (overlay_id, certificate) = match prefix.downcast::<OverlayMessageBoxed>() {
            Ok(msg) => {
                let id = OverlayShortId::from_data(msg.overlay().as_slice().clone());
                let certificate = if let OverlayMessageBoxed::Overlay_MessageWithExtra(msg) = msg {
                    match msg.extra.certificate {
                        Some(MemberCertificateBoxed::Overlay_MemberCertificate(cert)) => Some(cert),
                        _ => None,
                    }
                } else {
                    None
                };
                (id, certificate)
            }
            Err(_) => return Ok(false),
        };
        if suffix_offset == data.len() {
            log::warn!("Empty message in overlay {overlay_id}");
            return Ok(true);
        }
        let overlay = self.get_overlay(&overlay_id, "Message to unknown overlay")?;
        if (overlay.flags & Overlay::FLAG_OVERLAY_OTHER_WORKCHAIN) != 0 {
            return Ok(true);
        }
        if !self.check_overlay_adnl_address(&overlay, peers.local()) {
            return Ok(true);
        }
        if let Err(e) = overlay.check_peer(peers.other(), certificate.as_ref()) {
            log::warn!("Error checking peer {}: {e}", peers.other());
            return Ok(true);
        }

        let suffix = &data[suffix_offset..];
        let consumer = if let Some(consumer) = self.consumers.get(&overlay_id) {
            let consumer = consumer.val().object.clone();
            if overlay.overlay_type.is_private() {
                match consumer.try_consume_custom(suffix, peers).await {
                    Err(e) => {
                        log::warn!("Unsupported custom data in overlay {overlay_id}: {e}");
                        return Ok(true);
                    }
                    Ok(true) => {
                        #[cfg(feature = "telemetry")]
                        let tag = if suffix.len() < 4 {
                            0
                        } else {
                            u32::from_le_bytes([suffix[0], suffix[1], suffix[2], suffix[3]])
                        };
                        #[cfg(feature = "telemetry")]
                        overlay.update_stats(peers.other(), tag, false)?;
                        return Ok(true);
                    }
                    _ => (),
                }
            }
            Some(consumer)
        } else {
            None
        };

        let (mut bundle, postfix_offset) = deserialize_boxed_bundle_with_suffix(suffix)?;
        if bundle.len() > 2 {
            return Ok(false);
        }
        let have_postfix = postfix_offset < suffix.len();

        #[cfg(feature = "telemetry")]
        overlay.update_stats(peers.other(), bundle[0].bare_object().constructor(), false)?;
        if bundle.len() == 2 {
            // Catchain/validator session messages in private overlay
            let catchain_update = match bundle.remove(0).downcast::<CatchainBlockUpdateBoxed>() {
                Ok(CatchainBlockUpdateBoxed::Catchain_BlockUpdate(upd)) => upd,
                Err(msg) => fail!("Unsupported private overlay message {:?}", msg),
            };
            let inner_update = match bundle.remove(0).downcast::<ValidatorSessionBlockUpdateBoxed>()
            {
                Ok(ValidatorSessionBlockUpdateBoxed::ValidatorSession_BlockUpdate(upd)) => {
                    CatchainData::ValidatorSession(upd)
                }
                Err(msg) => match msg.downcast::<CatchainBlockInnerDataBoxed>() {
                    Ok(CatchainBlockInnerDataBoxed::Catchain_Block_Data_Fork(upd)) => {
                        CatchainData::Catchain(upd)
                    }
                    Ok(msg) => fail!("Unsupported private overlay message {:?}", msg),
                    Err(msg) => fail!("Unsupported private overlay message {:?}", msg),
                },
            };
            let receiver = overlay
                .received_catchain
                .as_ref()
                .ok_or_else(|| error!("No catchain receiver in private overlay {}", overlay_id))?;
            receiver.push((catchain_update, inner_update, peers.other().clone()));
            Ok(true)
        } else {
            let message = bundle.remove(0);
            let (data, hops) = if have_postfix {
                (&data[..suffix_offset + postfix_offset + 1], Some(suffix[postfix_offset]))
            } else {
                (&data[..suffix_offset + postfix_offset], None)
            };
            // Broadcasts maybe
            let ctx = BroadcastRecvContext {
                data: BroadcastData::Raw(data),
                hops,
                overlay: &overlay,
                peers,
            };
            let message = match message.downcast::<Broadcast>() {
                Ok(Broadcast::Overlay_BroadcastFec(bcast)) => {
                    if let Err(e) = Self::check_fec_broadcast_message(&bcast) {
                        // Ignore invalid messages as early as possible
                        log::warn!(target: TARGET, "Received bad FEC broadcast. {e}");
                        return Ok(true);
                    }
                    BroadcastFecProtocol::for_recv().recv(bcast, ctx).await?;
                    return Ok(true);
                }
                Ok(Broadcast::Overlay_Broadcast(bcast)) => {
                    BroadcastSimpleProtocol.recv(bcast, ctx).await?;
                    return Ok(true);
                }
                Ok(Broadcast::Overlay_BroadcastStream(bcast)) => {
                    BroadcastStreamSimpleProtocol.recv(bcast, ctx).await?;
                    return Ok(true);
                }
                Ok(Broadcast::Overlay_BroadcastTwostepFec(bcast)) => {
                    BroadcastTwostepFecProtocol::for_recv().recv(bcast, ctx).await?;
                    return Ok(true);
                }
                Ok(Broadcast::Overlay_BroadcastTwostepSimple(bcast)) => {
                    let big_data = bcast.data.len() >= Self::MIN_BYTES_FEC_TWO_STEPS_BROADCAST;
                    let reliable = big_data || ctx.overlay.overlay_type.quic_requested();
                    BroadcastTwostepSimpleProtocol::for_recv(reliable).recv(bcast, ctx).await?;
                    return Ok(true);
                }
                Ok(bcast) => fail!("Unsupported overlay broadcast message {:?}", bcast),
                Err(message) => message,
            };
            // Messages to custom cosnumers
            let Some(consumer) = consumer else {
                fail!("No dedicated consumer for message {message:?} in overlay {overlay_id}")
            };
            match consumer.try_consume_object(message, peers).await {
                Err(message) => fail!("Unsupported message {message} in overlay {overlay_id}"),
                r => r,
            }
        }
    }

    async fn try_consume_query_bundle(
        &self,
        mut objects: Vec<TLObject>,
        peers: &AdnlPeers,
    ) -> Result<QueryResult> {
        if objects.len() != 2 {
            return Ok(QueryResult::RejectedBundle(objects));
        }
        let (overlay_id, certificate) = match objects.remove(0).downcast::<OverlayQuery>() {
            Ok(query) => (OverlayShortId::from_data(query.overlay.inner()), None),
            Err(query) => match query.downcast::<OverlayQueryWithExtra>() {
                Ok(query) => {
                    let cert = match query.extra.certificate {
                        Some(MemberCertificateBoxed::Overlay_MemberCertificate(cert)) => Some(cert),
                        _ => None,
                    };
                    (OverlayShortId::from_data(query.overlay.inner()), cert)
                }
                Err(query) => {
                    objects.insert(0, query);
                    return Ok(QueryResult::RejectedBundle(objects));
                }
            },
        };
        let overlay = if let Some(overlay) = self.overlays.get(&overlay_id) {
            overlay.val().clone()
        } else {
            fail!("Query to unknown overlay {}", overlay_id)
        };
        if !self.check_overlay_adnl_address(&overlay, peers.local()) {
            return Ok(QueryResult::Consumed(QueryAnswer::Ready(None)));
        }
        overlay.check_peer(peers.other(), certificate.as_ref())?;

        let other_workchain = (overlay.flags & Overlay::FLAG_OVERLAY_OTHER_WORKCHAIN) != 0;
        #[cfg(feature = "telemetry")]
        if !other_workchain {
            overlay.update_stats(peers.other(), objects[0].bare_object().constructor(), false)?;
        }
        let object = match objects.remove(0).downcast::<GetRandomPeers>() {
            Ok(query) => {
                return match self.process_get_random_peers(
                    &overlay,
                    query.peers.nodes,
                    |node| OverlayNodeInfo::V1(node),
                    false,
                )? {
                    Some(Nodes::V1(answer)) => QueryResult::consume(
                        answer,
                        #[cfg(feature = "telemetry")]
                        None,
                    ),
                    None => Ok(QueryResult::Consumed(QueryAnswer::Ready(None))),
                    _ => fail!("Unexpected V2 answer in V1 query"),
                };
            }
            Err(object) => object,
        };
        let object = match object.downcast::<GetRandomPeersV2>() {
            Ok(query) => {
                return match self.process_get_random_peers(
                    &overlay,
                    query.peers.nodes,
                    |node| OverlayNodeInfo::V2(node),
                    true,
                )? {
                    Some(Nodes::V2(answer)) => QueryResult::consume(
                        answer,
                        #[cfg(feature = "telemetry")]
                        None,
                    ),
                    None => Ok(QueryResult::Consumed(QueryAnswer::Ready(None))),
                    _ => fail!("Unexpected V1 answer in V2 query"),
                };
            }
            Err(object) => object,
        };
        let object = match object.downcast::<Ping>() {
            Ok(_query) => {
                return QueryResult::consume_boxed(
                    Pong::Overlay_Pong,
                    #[cfg(feature = "telemetry")]
                    None,
                );
            }
            Err(object) => object,
        };

        if other_workchain {
            return Ok(QueryResult::Consumed(QueryAnswer::Ready(None)));
        }
        let consumer = if let Some(consumer) = self.consumers.get(&overlay_id) {
            consumer.val().object.clone()
        } else {
            fail!(
                "No dedicated consumer for query {object:?} from {} in overlay {overlay_id}",
                peers.other()
            )
        };
        match consumer.try_consume_query(object, peers).await {
            Err(msg) => fail!("Unsupported query {msg} in overlay {overlay_id}"),
            r => r,
        }
    }
}
