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
#[cfg(feature = "dump")]
use crate::dump;
#[cfg(feature = "telemetry")]
use crate::telemetry::{TelemetryItem, TelemetryPrinter};
use crate::{
    common::{
        add_counted_object_to_map, add_counted_object_to_map_with_update,
        add_unbound_object_to_map, add_unbound_object_to_map_with_update, hash, AdnlCryptoUtils,
        AdnlHandshake, AdnlPeers, AdnlPingSubscriber, AtomicPair, CountedObject, Counter, Custom,
        Query, QueryAdnlAnswer, QueryCache, QueryId, Stopper, Subscriber, TaggedAdnlMessage,
        TaggedByteSlice, TaggedTlObject, UpdatedAt, Version, TARGET, TARGET_QUERY,
    },
    declare_counted,
    telemetry::{Metric, MetricBuilder},
    transport::{udp_sender_receiver, udp_tcp_sender_receiver, AdnlReceiver, AdnlSender},
};
use rand::Rng;
use std::{
    borrow::Cow,
    cmp::{max, min},
    convert::TryInto,
    fmt::{self, Debug, Display, Formatter},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        atomic::{AtomicI32, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering},
        Arc, Condvar, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
#[cfg(feature = "dump")]
use std::{
    fs::{create_dir_all, rename, OpenOptions},
    io::Write,
    path::PathBuf,
};
#[cfg(feature = "telemetry")]
use ton_api::tag_from_data;
use ton_api::{
    deserialize_boxed, deserialize_typed, serialize_boxed,
    ton::{
        adnl::{
            address::address::{Quic, Udp},
            addresslist::AddressList,
            id::short::Short as AdnlIdShort,
            message::message::{
                Answer as AdnlAnswerMessage, ConfirmChannel, CreateChannel,
                Custom as AdnlCustomMessage, Part as AdnlPartMessage, Query as AdnlQueryMessage,
            },
            packetcontents::PacketContents as AdnlPacketContents,
            Address, Message as AdnlMessage, PacketContents as AdnlPacketContentsBoxed,
        },
        pub_::publickey::Aes as AesKey,
    },
    IntoBoxed, TLObject,
};
use ton_block::{
    base64_encode, error, fail, lz4_compress, lz4_decompress, sha256_digest, Ed25519KeyOption,
    KeyId, KeyOption, KeyOptionJson, Lz4DecompressMode, Result, UInt256,
};

#[macro_export]
macro_rules! adnl_node_test_key {
    ($tag: expr, $key: expr) => {
        format!(
            "{{
                \"tag\": {},
                \"data\": {{
                    \"type_id\": 1209251014,
                    \"pvt_key\": \"{}\"
                }}
            }}",
            $tag, $key
        )
        .as_str()
    };
}

#[macro_export]
macro_rules! adnl_node_test_config {
    ($ip: expr, $key:expr) => {
        format!(
            "{{
                \"ip_address\": \"{}\",
                \"keys\": [
                    {}
                ]
            }}",
            $ip, $key
        )
        .as_str()
    };
    ($ip: expr, $key1:expr, $key2:expr) => {
        format!(
            "{{
                \"ip_address\": \"{}\",
                \"keys\": [
                    {},
                    {}
                ]
            }}",
            $ip, $key1, $key2
        )
        .as_str()
    };
}

/// ADNL addresses cache iterator
#[derive(Debug)]
pub struct AddressCacheIterator(u32);

/// ADNL addresses cache
pub struct AddressCache {
    cache: lockfree::map::Map<Arc<KeyId>, u32>,
    index: lockfree::map::Map<u32, Arc<KeyId>>,
    limit: u32,
    upper: AtomicU32,
}

impl AddressCache {
    pub fn with_limit(limit: u32) -> Self {
        Self {
            cache: lockfree::map::Map::new(),
            index: lockfree::map::Map::new(),
            limit,
            upper: AtomicU32::new(0),
        }
    }

    pub fn contains(&self, address: &Arc<KeyId>) -> bool {
        self.cache.get(address).is_some()
    }

    pub fn count(&self) -> u32 {
        min(self.upper.load(Ordering::Relaxed), self.limit)
    }

    pub fn dump(&self) {
        let (mut iter, mut current) = self.first();
        log::debug!(target: TARGET, "ADDRESS CACHE:");
        while let Some(peer) = current {
            log::debug!(target: TARGET, "{}", peer);
            current = self.next(&mut iter)
        }
    }

    pub fn first(&self) -> (AddressCacheIterator, Option<Arc<KeyId>>) {
        (AddressCacheIterator(0), self.find_by_index(0))
    }

    pub fn given(&self, iter: &AddressCacheIterator) -> Option<Arc<KeyId>> {
        let AddressCacheIterator(ref index) = iter;
        self.find_by_index(*index)
    }

    pub fn next(&self, iter: &mut AddressCacheIterator) -> Option<Arc<KeyId>> {
        let AddressCacheIterator(ref mut index) = iter;
        loop {
            let ret = self.find_by_index({
                *index += 1;
                *index
            });
            if ret.is_some() {
                return ret;
            }
            let limit = self.upper.load(Ordering::Relaxed);
            if *index >= min(limit, self.limit) {
                return None;
            }
        }
    }

    pub fn put(&self, address: Arc<KeyId>) -> Result<bool> {
        let mut index = 0;
        let ret = add_unbound_object_to_map(&self.cache, address.clone(), || {
            let upper = self.upper.fetch_add(1, Ordering::Relaxed);
            index = upper;
            if index >= self.limit {
                if index >= self.limit * 2 {
                    self.upper
                        .compare_exchange(
                            upper + 1,
                            index - self.limit + 1,
                            Ordering::Relaxed,
                            Ordering::Relaxed,
                        )
                        .ok();
                }
                index %= self.limit;
            }
            Ok(index)
        })?;
        if ret {
            if let Some(index) = self.index.insert(index, address) {
                self.cache.remove_with(index.val(), |&(_, val)| &val == index.key());
            }
        }
        Ok(ret)
    }

    pub fn random_set(&self, dst: &AddressCache, n: u32) -> Result<()> {
        self.random_set_may_skip(dst, None, n)
    }

    pub fn random_vec(&self, skip: Option<&Arc<KeyId>>, n: u32) -> Vec<Arc<KeyId>> {
        let max = self.count();
        let mut ret = Vec::new();
        let mut check = false;
        let mut i = min(max, n);
        while i > 0 {
            if let Some(key_id) = self.index.get(&rand::thread_rng().gen_range(0..max)) {
                let key_id = key_id.val();
                if let Some(skip) = skip {
                    if skip == key_id {
                        // If there are not enough items in cache,
                        // reduce limit for skipped element
                        if (n >= max) && !check {
                            check = true;
                            i -= 1;
                        }
                        continue;
                    }
                }
                if ret.contains(key_id) {
                    continue;
                } else {
                    ret.push(key_id.clone());
                    i -= 1;
                }
            }
        }
        ret
    }

    fn find_by_index(&self, index: u32) -> Option<Arc<KeyId>> {
        self.index.get(&index).map(|address| address.val().clone())
    }

    fn random(&self, skip: Option<(&BadPeers, &BadPolicy)>) -> Option<Arc<KeyId>> {
        let max = self.count();
        // We need a finite loop here because we can test skip set only on case-by-case basis
        // due to multithreading. So it is possible that all items shall be skipped, and with
        // infinite loop we will simply hang
        for _ in 0..10 {
            if let Some(ret) = self.index.get(&rand::thread_rng().gen_range(0..max)) {
                let ret = ret.val();
                if let Some((peers, policy)) = skip {
                    if let Some(peer) = peers.get(ret) {
                        if peer.val().score.load(Ordering::Relaxed) >= policy.to_block {
                            continue;
                        }
                    }
                }
                return Some(ret.clone());
            }
        }
        None
    }

    fn random_set_may_skip(
        &self,
        dst: &AddressCache,
        skip: Option<(&BadPeers, &BadPolicy)>,
        n: u32,
    ) -> Result<()> {
        let mut n = min(self.count(), n);
        while n > 0 {
            if let Some(key_id) = self.random(skip) {
                // We do not check success of put due to multithreading
                dst.put(key_id)?;
                n -= 1;
            } else {
                break;
            }
        }
        Ok(())
    }
}

struct BadPeer {
    last_ok_sec: AtomicU32,
    score: AtomicU8,
}

type BadPeers = lockfree::map::Map<Arc<KeyId>, BadPeer>;

pub(crate) struct BadPolicy {
    pub(crate) amnesty: u8,
    pub(crate) latency: u32,
    pub(crate) penalty: u8,
    pub(crate) to_block: u8,
}

/// ADNL addresses cache with bad peers
pub(crate) struct AddressCacheWithBads {
    bads: BadPeers,
    peers: AddressCache,
    policy: BadPolicy,
}

impl AddressCacheWithBads {
    pub(crate) fn with_params(limit: u32, policy: BadPolicy) -> Self {
        Self { bads: lockfree::map::Map::new(), peers: AddressCache::with_limit(limit), policy }
    }

    pub(crate) fn add(&self, peer: &Arc<KeyId>) -> Result<()> {
        self.peers.put(peer.clone())?;
        self.bads.remove(peer);
        Ok(())
    }

    pub(crate) fn all(&self) -> &AddressCache {
        &self.peers
    }

    pub(crate) fn amnesty(&self, peer: &Arc<KeyId>, elapsed_sec: u32) -> Option<u8> {
        loop {
            let Some(peer) = self.bads.get(peer) else {
                break None;
            };
            let peer = peer.val();
            peer.last_ok_sec.store(elapsed_sec, Ordering::Relaxed);
            let old = peer.score.load(Ordering::Relaxed);
            if old > self.policy.to_block / 2 {
                let new = old - self.policy.amnesty.min(old);
                if !Self::update_score(&peer.score, old, new) {
                    continue;
                }
                break Some(new);
            } else {
                break None;
            }
        }
    }

    pub(crate) fn block(&self, peer: &Arc<KeyId>) -> Result<bool> {
        loop {
            if add_unbound_object_to_map(&self.bads, peer.clone(), || {
                Ok(BadPeer {
                    last_ok_sec: AtomicU32::new(0),
                    score: AtomicU8::new(self.policy.to_block),
                })
            })? {
                break Ok(true);
            }
            let Some(peer) = self.bads.get(peer) else {
                continue;
            };
            let peer = peer.val();
            if peer.score.load(Ordering::Relaxed) < self.policy.to_block {
                peer.score.store(self.policy.to_block, Ordering::Relaxed);
                break Ok(true);
            } else {
                break Ok(false);
            }
        }
    }

    pub(crate) fn given(&self, iter: &AddressCacheIterator) -> Option<Arc<KeyId>> {
        self.peers.given(iter)
    }

    pub(crate) fn next(&self, iter: &mut Option<AddressCacheIterator>) -> Option<Arc<KeyId>> {
        loop {
            let ret = if let Some(iter) = iter {
                self.peers.next(iter)
            } else {
                let (new_iter, first) = self.peers.first();
                iter.replace(new_iter);
                first
            };
            if let Some(peer) = &ret {
                if let Some(peer) = self.bads.get(peer) {
                    if peer.val().score.load(Ordering::Relaxed) >= self.policy.to_block {
                        continue;
                    }
                }
            }
            break ret;
        }
    }

    pub(crate) fn penalty(&self, peer: &Arc<KeyId>, elapsed_sec: u32) -> Result<Option<u8>> {
        loop {
            if let Some(peer) = self.bads.get(peer) {
                let peer = peer.val();
                let old = peer.score.load(Ordering::Relaxed);
                if old < self.policy.to_block {
                    let mut new = self.policy.to_block.min(old + self.policy.penalty);
                    let last_ok_sec = peer.last_ok_sec.load(Ordering::Relaxed);
                    if last_ok_sec + self.policy.latency > elapsed_sec {
                        new = new.min(self.policy.to_block - 1);
                        if new == old {
                            break Ok(None);
                        }
                    }
                    if !Self::update_score(&peer.score, old, new) {
                        continue;
                    }
                    break Ok(Some(new));
                } else {
                    break Ok(None);
                }
            }
            add_unbound_object_to_map(&self.bads, peer.clone(), || {
                Ok(BadPeer { last_ok_sec: AtomicU32::new(elapsed_sec), score: AtomicU8::new(0) })
            })?;
        }
    }

    pub(crate) fn random_set(&self, dst: &AddressCache, n: u32) -> Result<()> {
        self.peers.random_set_may_skip(dst, Some((&self.bads, &self.policy)), n)
    }

    pub(crate) fn score(&self, peer: &Arc<KeyId>) -> Option<u8> {
        self.bads.get(peer).map(|peer| peer.val().score.load(Ordering::Relaxed))
    }

    fn update_score(score: &AtomicU8, old: u8, new: u8) -> bool {
        score.compare_exchange(old, new, Ordering::Relaxed, Ordering::Relaxed).is_ok()
    }
}

// ADNL channel
declare_counted!(
    struct AdnlChannel {
        local_key: Arc<KeyId>,
        other_key: Arc<KeyId>,
        flags: AtomicU64,
        recv: ChannelSide,
        send: ChannelSide,
    }
);

struct ChannelSide {
    normal: SubchannelSide,
    stream: SubchannelSide,
    urgent: SubchannelSide,
}

struct SubchannelSide {
    id: ChannelId,
    secret: [u8; 32],
}

impl AdnlChannel {
    const CHANNEL_RESET: u64 = 0x8000000000000000;
    const ESTABLISHED: u64 = 0x4000000000000000;
    const SEQNO_RESET: u64 = 0x2000000000000000;
    const MASK_TIMESTAMP: u64 = 0x0FFFFFFFFFFFFFFF;

    fn with_keys(
        local_key: &Arc<KeyId>,
        channel_pvt_key: &Arc<dyn KeyOption>,
        other_key: &Arc<KeyId>,
        channel_pub_key: &[u8; 32],
        counter: Arc<AtomicU64>,
    ) -> Result<Self> {
        let fwd_secret = channel_pvt_key.shared_secret(channel_pub_key)?;
        let cmp = local_key.cmp(other_key);
        let (fwd_secret, rev_secret) = if std::cmp::Ordering::Equal == cmp {
            (fwd_secret, fwd_secret)
        } else {
            #[rustfmt::skip]
            let rev_secret = [
                fwd_secret[31], fwd_secret[30], fwd_secret[29], fwd_secret[28], fwd_secret[27],
                fwd_secret[26], fwd_secret[25], fwd_secret[24], fwd_secret[23], fwd_secret[22],
                fwd_secret[21], fwd_secret[20], fwd_secret[19], fwd_secret[18], fwd_secret[17],
                fwd_secret[16], fwd_secret[15], fwd_secret[14], fwd_secret[13], fwd_secret[12],
                fwd_secret[11], fwd_secret[10], fwd_secret[ 9], fwd_secret[ 8], fwd_secret[ 7],
                fwd_secret[ 6], fwd_secret[ 5], fwd_secret[ 4], fwd_secret[ 3], fwd_secret[ 2],
                fwd_secret[ 1], fwd_secret[ 0]
            ];
            if std::cmp::Ordering::Less == cmp {
                (fwd_secret, rev_secret)
            } else {
                (rev_secret, fwd_secret)
            }
        };
        let ret = Self {
            local_key: local_key.clone(),
            other_key: other_key.clone(),
            flags: AtomicU64::new(0),
            recv: Self::build_side(fwd_secret)?,
            send: Self::build_side(rev_secret)?,
            counter: counter.into(),
        };
        Ok(ret)
    }

    fn build_side(normal_secret: [u8; 32]) -> Result<ChannelSide> {
        let stream_secret = Self::build_stream_secret(&normal_secret);
        let urgent_secret = Self::build_urgent_secret(&normal_secret);
        let ret = ChannelSide {
            normal: SubchannelSide { id: Self::calc_id(&normal_secret)?, secret: normal_secret },
            stream: SubchannelSide { id: Self::calc_id(&stream_secret)?, secret: stream_secret },
            urgent: SubchannelSide { id: Self::calc_id(&urgent_secret)?, secret: urgent_secret },
        };
        Ok(ret)
    }

    #[rustfmt::skip]
    fn build_stream_secret(normal_secret: &[u8; 32]) -> [u8; 32] {
        [
            normal_secret[ 3], normal_secret[ 2], normal_secret[ 1], normal_secret[ 0],
            normal_secret[ 7], normal_secret[ 6], normal_secret[ 5], normal_secret[ 4],
            normal_secret[11], normal_secret[10], normal_secret[ 9], normal_secret[ 8],
            normal_secret[15], normal_secret[14], normal_secret[13], normal_secret[12],
            normal_secret[19], normal_secret[18], normal_secret[17], normal_secret[16],
            normal_secret[23], normal_secret[22], normal_secret[21], normal_secret[20],
            normal_secret[27], normal_secret[26], normal_secret[24], normal_secret[25],
            normal_secret[31], normal_secret[30], normal_secret[29], normal_secret[28]
        ]
    }

    #[rustfmt::skip]
    fn build_urgent_secret(normal_secret: &[u8; 32]) -> [u8; 32] {
        [
            normal_secret[ 1], normal_secret[ 0], normal_secret[ 3], normal_secret[ 2],
            normal_secret[ 5], normal_secret[ 4], normal_secret[ 7], normal_secret[ 6],
            normal_secret[ 9], normal_secret[ 8], normal_secret[11], normal_secret[10],
            normal_secret[13], normal_secret[12], normal_secret[15], normal_secret[14],
            normal_secret[17], normal_secret[16], normal_secret[19], normal_secret[18],
            normal_secret[21], normal_secret[20], normal_secret[23], normal_secret[22],
            normal_secret[25], normal_secret[24], normal_secret[27], normal_secret[26],
            normal_secret[29], normal_secret[28], normal_secret[31], normal_secret[30]
        ]
    }

    fn calc_id(secret: &[u8; 32]) -> Result<ChannelId> {
        let object = AesKey { key: UInt256::with_array(*secret) };
        hash(object)
    }

    fn decrypt(buf: &mut Vec<u8>, side: &SubchannelSide) -> Result<Option<u16>> {
        fn process(
            buf: &mut Vec<u8>,
            side: &SubchannelSide,
            offset: usize,
            version: &Option<u16>,
        ) -> Result<()> {
            if offset < 32 {
                fail!("INERNAL ERROR: bad offset");
            }
            AdnlChannel::process_data(buf, &side.secret, offset)?;
            if !AdnlCryptoUtils::calc_checksum(version, &buf[offset..])
                .eq(&buf[(offset - 32)..offset])
            {
                fail!("Bad channel message checksum, offset {}", offset);
            }
            buf.drain(0..offset);
            Ok(())
        }

        if buf.len() < 64 {
            fail!("Channel message is too short: {}", buf.len())
        }
        if buf.len() >= 68 {
            if let Some(version) =
                AdnlCryptoUtils::decode_version(&buf[32..36].try_into()?, &buf[..32], &buf[36..68])
            {
                let mut tmp = Vec::with_capacity(buf.len() - 68);
                tmp.extend_from_slice(&buf[68..]);
                let version = Some(version);
                if process(buf, side, 68, &version).is_ok() {
                    return Ok(version);
                }
                buf[68..].copy_from_slice(&tmp);
            }
        }
        process(buf, side, 64, &None)?;
        Ok(None)
    }

    fn decrypt_by_method(
        &self,
        buf: &mut Vec<u8>,
        method: &AdnlSendMethodDetailed,
    ) -> Result<Option<u16>> {
        match method {
            AdnlSendMethodDetailed::FastNormal => Self::decrypt(buf, &self.recv.normal),
            AdnlSendMethodDetailed::FastUrgent => Self::decrypt(buf, &self.recv.urgent),
            AdnlSendMethodDetailed::Safe => Self::decrypt(buf, &self.recv.stream),
        }
    }

    fn encrypt(buf: &mut Vec<u8>, side: &SubchannelSide, version: Option<u16>) -> Result<()> {
        AdnlCryptoUtils::encode_header(buf, &side.id, None, version);
        //dump!(info, TARGET, "secret ->", &side.secret);
        Self::process_data(buf, &side.secret, if version.is_some() { 68 } else { 64 })
    }

    fn encrypt_by_method(
        &self,
        buf: &mut Vec<u8>,
        version: Option<u16>,
        method: &AdnlSendMethodDetailed,
    ) -> Result<()> {
        match method {
            AdnlSendMethodDetailed::FastNormal => Self::encrypt(buf, &self.send.normal, version),
            AdnlSendMethodDetailed::FastUrgent => Self::encrypt(buf, &self.send.urgent, version),
            AdnlSendMethodDetailed::Safe => Self::encrypt(buf, &self.send.stream, version),
        }
    }

    fn get_reset_at(&self) -> u64 {
        self.flags.load(Ordering::Relaxed) & Self::MASK_TIMESTAMP
    }

    fn process_data(buf: &mut [u8], secret: &[u8; 32], offset: usize) -> Result<()> {
        if offset < 32 {
            fail!("INTERNAL ERROR: bad offset of data to process")
        }
        AdnlCryptoUtils::build_cipher_secure(secret, buf[(offset - 32)..offset].try_into()?)
            .apply_keystream(&mut buf[offset..]);
        Ok(())
    }
}

struct AdnlNodeAddress {
    channel_key: Arc<dyn KeyOption>,
    ip_version_address_adnl: AtomicPair,
    ip_version_address_quic: AtomicPair,
    key: Arc<dyn KeyOption>,
}

impl AdnlNodeAddress {
    fn from_ip_addresses_and_key(
        ip_address_adnl: &IpAddress,
        ip_address_quic: Option<&IpAddress>,
        key: &Arc<dyn KeyOption>,
    ) -> Result<Self> {
        let ret = Self {
            channel_key: Ed25519KeyOption::generate()?,
            ip_version_address_adnl: AtomicPair::new(
                ip_address_adnl.version as u64,
                ip_address_adnl.address,
            ),
            ip_version_address_quic: match ip_address_quic {
                Some(q) => AtomicPair::new(q.version as u64, q.address),
                None => AtomicPair::new(0, 0),
            },
            key: key.clone(),
        };
        Ok(ret)
    }

    fn update(&self, ip_address_adnl: &IpAddress, ip_address_quic: Option<&IpAddress>) -> bool {
        if let Some(q) = ip_address_quic {
            self.ip_version_address_quic.update(
                q.version as u64,
                q.address,
                |old_version, new_version| old_version < new_version,
            );
        }
        self.ip_version_address_adnl.update(
            ip_address_adnl.version as u64,
            ip_address_adnl.address,
            |old_version, new_version| old_version < new_version,
        )
    }
}

/// ADNL node configuration
pub struct AdnlNodeConfig {
    ip_address: IpAddress,
    ip_address_quic: Option<IpAddress>,
    keys: lockfree::map::Map<Arc<KeyId>, Arc<dyn KeyOption>>,
    tags: lockfree::map::Map<usize, Arc<KeyId>>,
    recv_pipeline_pool: Option<u8>, // %% of cpu cores to assign for recv workers
    recv_priority_pool: Option<u8>, // %% of workers to assign for priority recv
    #[cfg(feature = "telemetry")]
    telemetry_peer_packets: bool,
    throughput: u32, // Bytes/sec to send at maximum
    #[cfg(feature = "telemetry")]
    timeout_check_packet_processing_mcs: u64,
    timeout_expire_queued_packet_sec: u32,
}

#[derive(serde::Deserialize, serde::Serialize)]
struct AdnlNodeKeyJson {
    tag: usize,
    data: KeyOptionJson,
}

#[derive(serde::Deserialize, serde::Serialize)]
pub struct AdnlNodeConfigJson {
    ip_address: String,
    #[serde(default)]
    ip_address_quic: Option<String>,
    keys: Vec<AdnlNodeKeyJson>,
    recv_pipeline_pool: Option<u8>, // %% of cpu cores to assign for recv workers
    recv_priority_pool: Option<u8>, // %% of workers to assign for priority recv
    #[cfg(feature = "telemetry")]
    telemetry_peer_packets: Option<bool>,
    throughput: Option<u32>,
    #[cfg(feature = "telemetry")]
    timeout_check_packet_processing_mcs: Option<u64>,
    timeout_expire_queued_packet_sec: Option<u32>,
}

impl AdnlNodeConfigJson {
    /// Get IP address
    pub fn ip_address(&self) -> Result<IpAddress> {
        IpAddress::from_versioned_string(&self.ip_address, None)
    }

    /// Get QUIC IP address
    pub fn ip_address_quic(&self) -> Result<Option<IpAddress>> {
        self.ip_address_quic
            .as_deref()
            .map(|s| IpAddress::from_versioned_string(s, None))
            .transpose()
    }

    /// Get key by tag
    pub fn key_by_tag(&self, tag: usize, as_src: bool) -> Result<Arc<dyn KeyOption>> {
        for key in self.keys.iter() {
            if key.tag == tag {
                return if as_src {
                    Ok(Ed25519KeyOption::from_private_key_json(&key.data)?)
                } else {
                    Ok(Ed25519KeyOption::from_public_key_json(&key.data)?)
                };
            }
        }
        fail!("No keys with tag {} in node config", tag)
    }
}

impl AdnlNodeConfig {
    #[cfg(feature = "telemetry")]
    const DEFAULT_TIMEOUT_CHECK_PROCESSING_MCS: u64 = 5000;
    const DEFAULT_TIMEOUT_EXPIRE_QUEUED_PACKET_SEC: u32 = 10;
    const DEFAULT_THROUGHPUT_BYTES_SEC: u32 = 50 * 1024 * 1024;

    /// Construct from IP address and key data
    pub fn from_ip_address_and_keys(
        ip_address: &str,
        ip_address_quic: Option<&str>,
        keys: Vec<(Arc<dyn KeyOption>, usize)>,
    ) -> Result<Self> {
        let ip_address_quic =
            ip_address_quic.map(|s| IpAddress::from_versioned_string(s, None)).transpose()?;
        let ret = AdnlNodeConfig {
            ip_address: IpAddress::from_versioned_string(ip_address, None)?,
            ip_address_quic,
            keys: lockfree::map::Map::new(),
            tags: lockfree::map::Map::new(),
            recv_pipeline_pool: None,
            recv_priority_pool: None,
            #[cfg(feature = "telemetry")]
            telemetry_peer_packets: false,
            throughput: Self::DEFAULT_THROUGHPUT_BYTES_SEC,
            #[cfg(feature = "telemetry")]
            timeout_check_packet_processing_mcs: Self::DEFAULT_TIMEOUT_CHECK_PROCESSING_MCS,
            timeout_expire_queued_packet_sec: Self::DEFAULT_TIMEOUT_EXPIRE_QUEUED_PACKET_SEC,
        };
        for (key, tag) in keys {
            ret.add_key(key, tag)?;
        }
        Ok(ret)
    }

    /// Construct from IP address and private key data
    pub fn from_ip_address_and_private_keys(
        ip_address: &str,
        keytags: Vec<([u8; 32], usize)>,
    ) -> Result<(AdnlNodeConfigJson, Self)> {
        let mut keys = Vec::new();
        for (key, tag) in keytags {
            let (json, key) = Ed25519KeyOption::from_private_key_with_json(&key)?;
            keys.push((json, key as Arc<dyn KeyOption>, tag))
        }
        Self::create_configs(ip_address, keys)
    }

    /// Construct from JSON data
    pub fn from_json(json: &str) -> Result<Self> {
        let json_config: AdnlNodeConfigJson = serde_json::from_str(json)?;
        Self::from_json_config(&json_config)
    }

    /// Construct from JSON config structure
    pub fn from_json_config(json_config: &AdnlNodeConfigJson) -> Result<Self> {
        let ret = AdnlNodeConfig {
            ip_address: json_config.ip_address()?,
            ip_address_quic: json_config.ip_address_quic()?,
            keys: lockfree::map::Map::new(),
            tags: lockfree::map::Map::new(),
            recv_pipeline_pool: json_config.recv_pipeline_pool,
            recv_priority_pool: json_config.recv_priority_pool,
            #[cfg(feature = "telemetry")]
            telemetry_peer_packets: json_config.telemetry_peer_packets.unwrap_or(false),
            throughput: json_config.throughput.unwrap_or(Self::DEFAULT_THROUGHPUT_BYTES_SEC),
            #[cfg(feature = "telemetry")]
            timeout_check_packet_processing_mcs: json_config
                .timeout_check_packet_processing_mcs
                .unwrap_or(Self::DEFAULT_TIMEOUT_CHECK_PROCESSING_MCS),
            timeout_expire_queued_packet_sec: json_config
                .timeout_expire_queued_packet_sec
                .unwrap_or(Self::DEFAULT_TIMEOUT_EXPIRE_QUEUED_PACKET_SEC),
        };
        for key in json_config.keys.iter() {
            let data = Ed25519KeyOption::from_private_key_json(&key.data)?;
            ret.add_key(data, key.tag)?;
        }
        Ok(ret)
    }

    /// Construct with given IP address (new key pair will be generated)
    pub fn with_ip_address_and_private_key_tags(
        ip_address: &str,
        tags: Vec<usize>,
    ) -> Result<(AdnlNodeConfigJson, Self)> {
        let mut keys = Vec::new();
        for tag in tags {
            let (json, key) = Ed25519KeyOption::generate_with_json()?;
            keys.push((json, key as Arc<dyn KeyOption>, tag))
        }
        Self::create_configs(ip_address, keys)
    }

    /// Node IP address
    pub fn ip_address(&self) -> &IpAddress {
        &self.ip_address
    }

    /// Node key by ID
    pub fn key_by_id(&self, id: &Arc<KeyId>) -> Result<Arc<dyn KeyOption>> {
        if let Some(key) = self.keys.get(id) {
            Ok(key.val().clone())
        } else {
            fail!("Bad key id {}", id)
        }
    }

    /// Node key by tag
    pub fn key_by_tag(&self, tag: usize) -> Result<Arc<dyn KeyOption>> {
        if let Some(id) = self.tags.get(&tag) {
            self.key_by_id(id.val())
        } else {
            fail!("Bad key tag {}", tag)
        }
    }

    /// Set port number
    pub fn set_port(&mut self, port: u16) {
        self.ip_address.set_port(port)
    }

    /// Set QUIC address
    pub fn set_ip_address_quic(&mut self, addr: SocketAddr) {
        if let IpAddr::V4(ipv4) = addr.ip() {
            self.ip_address_quic =
                Some(IpAddress::from_versioned_parts(u32::from(ipv4), addr.port(), None));
        } else {
            log::warn!(target: TARGET, "IPv6 QUIC address {} ignored, only IPv4 is supported", addr);
        }
    }

    /// Set worker pools
    pub fn set_recv_worker_pools(
        &mut self,
        pipeline_pool: Option<u8>,
        priority_pool: Option<u8>,
    ) -> Result<()> {
        self.recv_pipeline_pool = Self::check_percentage_pool(pipeline_pool, "pipeline")?;
        self.recv_priority_pool = Self::check_percentage_pool(priority_pool, "priority")?;
        Ok(())
    }

    /// Set throughput (packets / ms)
    pub fn set_throughput(&mut self, throughput: u32) {
        self.throughput = throughput
    }

    fn add_key(&self, key: Arc<dyn KeyOption>, tag: usize) -> Result<Arc<KeyId>> {
        let mut ret = key.id().clone();
        let added = add_unbound_object_to_map_with_update(&self.tags, tag, |found| {
            if let Some(found) = found {
                if found != &ret {
                    fail!("Duplicated key tag {} in node", tag)
                } else {
                    ret = found.clone();
                    Ok(None)
                }
            } else {
                Ok(Some(ret.clone()))
            }
        })?;
        if added {
            add_unbound_object_to_map(&self.keys, ret.clone(), || Ok(key.clone()))?;
        }
        Ok(ret)
    }

    fn create_configs(
        ip_address: &str,
        keys: Vec<(KeyOptionJson, Arc<dyn KeyOption>, usize)>,
    ) -> Result<(AdnlNodeConfigJson, Self)> {
        let mut json_keys = Vec::new();
        let mut tags_keys = Vec::new();
        for (json, key, tag) in keys {
            json_keys.push(AdnlNodeKeyJson { tag, data: json });
            tags_keys.push((key, tag));
        }
        let json = AdnlNodeConfigJson {
            ip_address: ip_address.to_string(),
            ip_address_quic: None,
            keys: json_keys,
            recv_pipeline_pool: None,
            recv_priority_pool: None,
            #[cfg(feature = "telemetry")]
            telemetry_peer_packets: None,
            throughput: None,
            #[cfg(feature = "telemetry")]
            timeout_check_packet_processing_mcs: None,
            timeout_expire_queued_packet_sec: None,
        };
        Ok((json, Self::from_ip_address_and_keys(ip_address, None, tags_keys)?))
    }

    fn delete_key(&self, key: &Arc<KeyId>, tag: usize) -> Result<bool> {
        let removed_key = self.keys.remove(key);
        if let Some(removed) = self.tags.remove(&tag) {
            if removed.val() != key {
                fail!("Expected {key} key with tag {tag} but got {}", removed.val())
            }
        }
        Ok(removed_key.is_some())
    }

    pub fn check_percentage_pool(pool: Option<u8>, msg: &str) -> Result<Option<u8>> {
        if let Some(pool) = pool {
            if pool == 0 {
                Ok(None)
            } else if pool >= 100 {
                fail!("Bad {} pool ({} %)", msg, pool)
            } else {
                Ok(Some(pool))
            }
        } else {
            Ok(None)
        }
    }
}

pub struct DataCompression;

impl DataCompression {
    const MAX_DECOMPRESSED_SIZE: usize = 16 << 20; // 16 MB
    const SIZE_COMPRESSION_THRESHOLD: usize = 256;
    const TAG_COMPRESSED: u8 = 0x80;

    pub fn compress_raw(data: &[u8]) -> Result<Vec<u8>> {
        if data.len() <= Self::SIZE_COMPRESSION_THRESHOLD {
            Ok(data.to_vec())
        } else {
            // Heuristic detection of compressed format for RAW data:
            // - data is compressed,
            // - then original length of the data is prepended to already compressed data,
            // - then tag byte is appended to already compressed data.
            // Data considered to be compressed if their last byte is the same as tag byte.
            // If decompression succeeded, the length of the original data must be checked.
            let mut ret = lz4_compress(data, true)?;
            ret.push(Self::TAG_COMPRESSED);
            log::trace!(target: TARGET, "Compress: {} -> {}", data.len(), ret.len());
            Ok(ret)
        }
    }

    pub fn decompress_raw(data: &[u8]) -> Option<Vec<u8>> {
        let len = data.len();
        if len <= 5 {
            return None;
        }
        if data[len - 1] != Self::TAG_COMPRESSED {
            return None;
        }
        let src_len = ((data[3] as usize) << 24)
            | ((data[2] as usize) << 16)
            | ((data[1] as usize) << 8)
            | (data[0] as usize);
        let max = Self::MAX_DECOMPRESSED_SIZE;
        if src_len > max {
            log::trace!(
                target: TARGET,
                "Decompress rejected: prepended size {src_len} exceeds cap {max}"
            );
            return None;
        }
        match lz4_decompress(&data[4..len - 1], Lz4DecompressMode::WithMaxSize(src_len as i32)) {
            Err(e) => {
                log::trace!(target: TARGET, "Decompress error: {e}");
                None
            }
            Ok(ret) => {
                if src_len != ret.len() {
                    None
                } else {
                    log::trace!(target: TARGET, "Decompress: {len} -> {src_len}");
                    Some(ret)
                }
            }
        }
    }
}

/// IP address internal representation
#[derive(PartialEq)]
pub struct IpAddress {
    address: u64,
    version: i32,
}

impl IpAddress {
    /// Construct from string
    pub fn from_versioned_string(src: &str, version: Option<i32>) -> Result<Self> {
        let addr: SocketAddr = src.parse()?;
        if let IpAddr::V4(ip) = addr.ip() {
            Ok(Self::from_versioned_parts(u32::from_be_bytes(ip.octets()), addr.port(), version))
        } else {
            fail!("IPv6 addressed are not supported")
        }
    }

    /// Get IP
    pub fn ip(&self) -> u32 {
        (self.address >> 16) as u32
    }

    /// Get port
    pub fn port(&self) -> u16 {
        self.address as u16
    }

    /// Convert to UDP TL struct
    pub fn to_udp(&self) -> Udp {
        Udp { ip: self.ip() as i32, port: self.port() as i32 }
    }

    fn from_versioned_parts(ip: u32, port: u16, version: Option<i32>) -> Self {
        Self {
            address: ((ip as u64) << 16) | port as u64,
            version: version.unwrap_or_else(|| Version::get()),
        }
    }

    fn set_ip(&mut self, ip: u32) {
        self.address = ((ip as u64) << 16) | (self.address & 0xFFFF);
        self.version = Version::get();
    }

    fn set_port(&mut self, port: u16) {
        self.address = (self.address & 0xFFFFFFFF0000u64) | port as u64;
        self.version = Version::get();
    }
}

impl Debug for IpAddress {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{} version {}", self, self.version)
    }
}

impl Display for IpAddress {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(
            f,
            "{}.{}.{}.{}:{}",
            (self.address >> 40) as u8,
            (self.address >> 32) as u8,
            (self.address >> 24) as u8,
            (self.address >> 16) as u8,
            self.address as u16
        )
    }
}

declare_counted!(
    struct Peer {
        address: AdnlNodeAddress,
        recv_state: PeerState,
        send_state: PeerState,
    }
);

impl Peer {
    #[cfg(feature = "telemetry")]
    fn print_state_stats(&self, state: &PeerState, local: &Arc<KeyId>) {
        let elapsed = state.telemetry.start.elapsed().as_secs();
        let bytes = state.telemetry.bytes.load(Ordering::Relaxed);
        log::info!(
            target: TARGET,
            "ADNL STAT {} {}-{}: {} bytes, {} bytes/sec average load",
            state.telemetry.name,
            local,
            self.address.key.id(),
            bytes,
            bytes / elapsed
        )
    }

    async fn try_reinit(&self, reinit_date: i32) -> Result<bool> {
        let old_reinit_date = self.send_state.reinit_date();
        match reinit_date.cmp(&old_reinit_date) {
            std::cmp::Ordering::Equal => Ok(true),
            std::cmp::Ordering::Greater => {
                // Refresh reinit state
                self.send_state.reset_reinit_date(reinit_date);
                if old_reinit_date != 0 {
                    self.send_state.reset_seqno().await?;
                    self.recv_state.reset_seqno().await?;
                }
                Ok(true)
            }
            std::cmp::Ordering::Less => Ok(reinit_date == 0),
        }
    }

    #[cfg(feature = "telemetry")]
    fn update_recv_stats(&self, bytes: u64, local: &Arc<KeyId>) {
        self.update_stats(&self.recv_state, bytes, local)
    }

    #[cfg(feature = "telemetry")]
    fn update_send_stats(&self, bytes: u64, local: &Arc<KeyId>) {
        self.update_stats(&self.send_state, bytes, local)
    }

    #[cfg(feature = "telemetry")]
    fn update_stats(&self, state: &PeerState, bytes: u64, local: &Arc<KeyId>) {
        if state.update_stats(bytes) {
            self.print_state_stats(state, local);
        }
    }
}

const HISTORY_BITS: usize = 512;
const HISTORY_SIZE: usize = HISTORY_BITS / 64;

struct HistoryLog {
    index: AtomicU64,
    masks: [AtomicU64; HISTORY_SIZE],
}

#[derive(Debug, PartialEq)]
enum MessageRepeat {
    NotNeeded,
    Required,
    Unapplicable,
}

declare_counted!(
    struct PacketBuffer {
        buf: Vec<u8>,
        expired_at: u32,
    }
);

pub struct PeerHistory {
    log: Option<HistoryLog>,
    seqno: AtomicU64,
}

impl PeerHistory {
    const INDEX_MASK: u64 = HISTORY_BITS as u64 / 2 - 1;
    const IN_TRANSIT: u64 = 0xFFFFFFFFFFFFFFFF;

    /// Construct for send
    pub fn for_send() -> Self {
        Self { log: None, seqno: AtomicU64::new(0) }
    }

    /// Construct for recv
    pub fn for_recv() -> Self {
        #[rustfmt::skip]
        let log = HistoryLog {
            index: AtomicU64::new(0),
            masks: [
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)
            ]
        };
        Self { log: Some(log), seqno: AtomicU64::new(0) }
    }

    /// Print stats
    pub fn print_stats(&self) {
        let seqno = self.seqno.load(Ordering::Relaxed);
        if let Some(log) = &self.log {
            log::info!(
                target: TARGET,
                "Peer history: seqno {}/{:x}, mask {:x} [{:x} {:x} {:x} {:x} {:x} {:x} {:x} {:x}]",
                seqno, seqno,
                log.index.load(Ordering::Relaxed),
                log.masks[0].load(Ordering::Relaxed),
                log.masks[1].load(Ordering::Relaxed),
                log.masks[2].load(Ordering::Relaxed),
                log.masks[3].load(Ordering::Relaxed),
                log.masks[4].load(Ordering::Relaxed),
                log.masks[5].load(Ordering::Relaxed),
                log.masks[6].load(Ordering::Relaxed),
                log.masks[7].load(Ordering::Relaxed)
            )
        } else {
            log::info!(target: TARGET, "Peer history: seqno {}/{:x}", seqno, seqno)
        }
    }

    /// Update with specified SEQ number
    pub async fn update(&self, seqno: u64, target: &str) -> Result<bool> {
        if let Some(log) = &self.log {
            self.update_log(log, seqno, target).await
        } else {
            loop {
                let last_seqno = self.seqno.load(Ordering::Relaxed);
                if last_seqno < seqno {
                    if self
                        .seqno
                        .compare_exchange(last_seqno, seqno, Ordering::Relaxed, Ordering::Relaxed)
                        .is_err()
                    {
                        tokio::task::yield_now().await;
                        continue;
                    }
                }
                return Ok(true);
            }
        }
    }

    async fn update_log(&self, log: &HistoryLog, seqno: u64, target: &str) -> Result<bool> {
        let seqno_masked = seqno & Self::INDEX_MASK;
        let seqno_normalized = seqno & !Self::INDEX_MASK;
        loop {
            let index = log.index.load(Ordering::Relaxed);
            if index == Self::IN_TRANSIT {
                tokio::task::yield_now().await;
                continue;
            }
            let index_masked = index & Self::INDEX_MASK;
            let index_normalized = index & !Self::INDEX_MASK;
            if index_normalized > seqno_normalized + Self::INDEX_MASK + 1 {
                // Out of the window
                log::trace!(
                    target: target,
                    "Peer packet with seqno {:x} is too old ({:x})",
                    seqno,
                    index_normalized
                );
                return Ok(false);
            }
            // Masks format:
            // lower0, lower1, lower2, lower3, upper0, upper1, upper2, upper3
            let mask = 1 << (seqno_masked % 64);
            #[allow(clippy::comparison_chain)]
            let mask_offset = if index_normalized > seqno_normalized {
                // Lower part of the window
                Some(0)
            } else if index_normalized == seqno_normalized {
                // Upper part of the window
                Some(HISTORY_SIZE / 2)
            } else {
                None
            };
            let next_index = if let Some(mask_offset) = mask_offset {
                let mask_offset = mask_offset + seqno_masked as usize / 64;
                let already_received = log.masks[mask_offset].load(Ordering::Relaxed) & mask;
                if log.index.load(Ordering::Relaxed) != index {
                    continue;
                }
                if already_received != 0 {
                    // Already received
                    log::trace!(
                        target: target,
                        "Peer packet with seqno {:x} was already received",
                        seqno
                    );
                    return Ok(false);
                }
                if log
                    .index
                    .compare_exchange(index, Self::IN_TRANSIT, Ordering::Relaxed, Ordering::Relaxed)
                    .is_err()
                {
                    continue;
                }
                log.masks[mask_offset].fetch_or(mask, Ordering::Relaxed);
                index
            } else {
                if log
                    .index
                    .compare_exchange(index, Self::IN_TRANSIT, Ordering::Relaxed, Ordering::Relaxed)
                    .is_err()
                {
                    continue;
                }
                if index_normalized + Self::INDEX_MASK + 1 == seqno_normalized {
                    for i in 0..HISTORY_SIZE / 2 {
                        log.masks[i].store(
                            log.masks[i + HISTORY_SIZE / 2].load(Ordering::Relaxed),
                            Ordering::Relaxed,
                        )
                    }
                    for i in HISTORY_SIZE / 2..HISTORY_SIZE {
                        log.masks[i].store(0, Ordering::Relaxed)
                    }
                } else {
                    for i in 0..HISTORY_SIZE {
                        log.masks[i].store(0, Ordering::Relaxed)
                    }
                }
                seqno_normalized
            };
            let last_seqno = self.seqno.load(Ordering::Relaxed);
            if last_seqno < seqno {
                self.seqno.store(seqno, Ordering::Relaxed)
            }
            let index_masked = (index_masked + 1) & !Self::INDEX_MASK;
            if log
                .index
                .compare_exchange(
                    Self::IN_TRANSIT,
                    next_index | index_masked,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                )
                .is_err()
            {
                fail!("INTERNAL ERROR: Peer packet seqno sync mismatch ({:x})", seqno)
            }
            break;
        }
        Ok(true)
    }

    async fn reset(&self, seqno: u64) -> Result<()> {
        if let Some(log) = &self.log {
            loop {
                let index = log.index.load(Ordering::Relaxed);
                if index == Self::IN_TRANSIT {
                    tokio::task::yield_now().await;
                    continue;
                }
                if log
                    .index
                    .compare_exchange(index, Self::IN_TRANSIT, Ordering::Relaxed, Ordering::Relaxed)
                    .is_err()
                {
                    continue;
                }
                break;
            }
            for i in 0..HISTORY_SIZE {
                log.masks[i].store(if i == HISTORY_SIZE / 2 { 1 } else { 0 }, Ordering::Relaxed)
            }
        }
        self.seqno.store(seqno, Ordering::Relaxed);
        if let Some(log) = &self.log {
            if log
                .index
                .compare_exchange(
                    Self::IN_TRANSIT,
                    seqno & !Self::INDEX_MASK,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                )
                .is_err()
            {
                fail!("INTERNAL ERROR: peer packet seqno reset mismatch ({:x})", seqno)
            }
        }
        Ok(())
    }
}

#[cfg(feature = "telemetry")]
struct PeerTelemetry {
    name: &'static str,
    start: Instant,
    print: AtomicU64,
    bytes: AtomicU64,
    packets: Option<Arc<MetricBuilder>>,
}

struct PeerState {
    normal_history: PeerHistory,
    stream_history: PeerHistory,
    urgent_history: PeerHistory,
    reinit_date: AtomicI32,
    #[cfg(feature = "telemetry")]
    telemetry: Arc<PeerTelemetry>,
}

impl PeerState {
    fn for_receive_with_reinit_date(
        reinit_date: i32,
        #[cfg(feature = "telemetry")] node: &AdnlNode,
        #[cfg(feature = "telemetry")] peers: &AdnlPeers,
        #[cfg(feature = "telemetry")] telemetry: Option<Arc<PeerTelemetry>>,
    ) -> Self {
        Self {
            normal_history: PeerHistory::for_recv(),
            stream_history: PeerHistory::for_recv(),
            urgent_history: PeerHistory::for_recv(),
            reinit_date: AtomicI32::new(reinit_date),
            #[cfg(feature = "telemetry")]
            telemetry: telemetry.unwrap_or_else(|| Self::create_telemetry(node, peers, false)),
        }
    }

    fn for_send(
        #[cfg(feature = "telemetry")] node: &AdnlNode,
        #[cfg(feature = "telemetry")] peers: &AdnlPeers,
        #[cfg(feature = "telemetry")] telemetry: Option<Arc<PeerTelemetry>>,
    ) -> Self {
        Self {
            normal_history: PeerHistory::for_send(),
            stream_history: PeerHistory::for_send(),
            urgent_history: PeerHistory::for_send(),
            reinit_date: AtomicI32::new(0),
            #[cfg(feature = "telemetry")]
            telemetry: telemetry.unwrap_or_else(|| Self::create_telemetry(node, peers, true)),
        }
    }

    fn next_seqno(&self, method: &AdnlSendMethodDetailed) -> u64 {
        match method {
            AdnlSendMethodDetailed::FastNormal => {
                self.normal_history.seqno.fetch_add(1, Ordering::Relaxed) + 1
            }
            AdnlSendMethodDetailed::FastUrgent => {
                self.urgent_history.seqno.fetch_add(1, Ordering::Relaxed) + 1
            }
            AdnlSendMethodDetailed::Safe => {
                self.stream_history.seqno.fetch_add(1, Ordering::Relaxed) + 1
            }
        }
    }

    fn reinit_date(&self) -> i32 {
        self.reinit_date.load(Ordering::Relaxed)
    }

    fn reset_reinit_date(&self, reinit_date: i32) {
        self.reinit_date.store(reinit_date, Ordering::Relaxed)
    }

    async fn reset_seqno(&self) -> Result<()> {
        self.normal_history.reset(0).await?;
        self.stream_history.reset(0).await?;
        self.urgent_history.reset(0).await
    }

    fn seqno(&self, method: &AdnlSendMethodDetailed) -> u64 {
        match method {
            AdnlSendMethodDetailed::FastNormal => self.normal_history.seqno.load(Ordering::Relaxed),
            AdnlSendMethodDetailed::FastUrgent => self.urgent_history.seqno.load(Ordering::Relaxed),
            AdnlSendMethodDetailed::Safe => self.stream_history.seqno.load(Ordering::Relaxed),
        }
    }

    async fn save_seqno(&self, seqno: u64, method: &AdnlSendMethodDetailed) -> Result<bool> {
        match method {
            AdnlSendMethodDetailed::FastNormal => self.normal_history.update(seqno, TARGET).await,
            AdnlSendMethodDetailed::FastUrgent => self.urgent_history.update(seqno, TARGET).await,
            AdnlSendMethodDetailed::Safe => self.stream_history.update(seqno, TARGET).await,
        }
    }

    #[cfg(feature = "telemetry")]
    fn add_metric(node: &AdnlNode, peers: &AdnlPeers, tag: &str, send: bool) -> Arc<MetricBuilder> {
        let local = peers.local().to_string();
        let other = peers.other().to_string();
        let name = if send {
            format!("{}->{} {}", &local[..6], &other[..6], tag)
        } else {
            format!("{}<-{} {}", &local[..6], &other[..6], tag)
        };
        let ret = Telemetry::create_metric_builder(name.as_str());
        node.telemetry.printer.add_metric(TelemetryItem::MetricBuilder(ret.clone()));
        ret
    }

    #[cfg(feature = "telemetry")]
    fn create_telemetry(node: &AdnlNode, peers: &AdnlPeers, for_send: bool) -> Arc<PeerTelemetry> {
        let ret = PeerTelemetry {
            name: if for_send { "send" } else { "recv" },
            start: Instant::now(),
            print: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            packets: if node.config.telemetry_peer_packets {
                Some(Self::add_metric(node, peers, "packets/sec", for_send))
            } else {
                None
            },
        };
        Arc::new(ret)
    }

    #[cfg(feature = "telemetry")]
    fn update_stats(&self, bytes: u64) -> bool {
        if let Some(packets) = &self.telemetry.packets {
            packets.update(1)
        }
        self.telemetry.bytes.fetch_add(bytes, Ordering::Relaxed);
        let elapsed = self.telemetry.start.elapsed().as_secs();
        if elapsed > self.telemetry.print.load(Ordering::Relaxed) {
            self.telemetry.print.store(elapsed + 5, Ordering::Relaxed);
            true
        } else {
            false
        }
    }
}

struct Peers {
    channels_send: Arc<ChannelsSend>,
    channels_wait: Arc<ChannelsSend>,
    map_of: lockfree::map::Map<Arc<KeyId>, Peer>,
}

impl Peers {
    fn with_incinerator(
        incinerator: &lockfree::map::SharedIncin<Arc<KeyId>, Arc<AdnlChannel>>,
    ) -> Arc<Self> {
        let ret = Peers {
            map_of: lockfree::map::Map::new(),
            channels_send: Arc::new(lockfree::map::Map::with_incin(incinerator.clone())),
            channels_wait: Arc::new(lockfree::map::Map::with_incin(incinerator.clone())),
        };
        Arc::new(ret)
    }
}

struct Queue<T> {
    queue: lockfree::queue::Queue<T>,
    #[cfg(feature = "telemetry")]
    count: AtomicU64,
    #[cfg(feature = "telemetry")]
    metric: Arc<Metric>,
}

impl<T> Queue<T> {
    fn new(#[cfg(feature = "telemetry")] metric: Arc<Metric>) -> Self {
        Self {
            #[cfg(feature = "telemetry")]
            count: AtomicU64::new(0),
            queue: lockfree::queue::Queue::new(),
            #[cfg(feature = "telemetry")]
            metric,
        }
    }

    fn put(&self, item: T) {
        #[cfg(feature = "telemetry")]
        self.count.fetch_add(1, Ordering::Relaxed);
        self.queue.push(item);
    }

    fn get(&self) -> Option<T> {
        #[allow(clippy::let_and_return)]
        let ret = self.queue.pop();
        #[cfg(feature = "telemetry")]
        if ret.is_some() {
            let count = self.count.fetch_sub(1, Ordering::Relaxed);
            self.metric.update(count)
        }
        ret
    }
}

struct QuerySendContext {
    channel: Option<Arc<AdnlChannel>>,
    method: AdnlSendMethodDetailed,
    query_id: QueryId,
    repeat: MessageRepeat,
    reply_ping: Arc<tokio::sync::Barrier>,
}

impl QuerySendContext {
    fn get_query_print_id(&self, peer: &Arc<KeyId>) -> String {
        AdnlNode::get_query_print_id(&self.query_id, peer, &self.method)
    }
}

struct RecvPipeline {
    adnl: Arc<AdnlNode>,
    // count: AtomicU64,
    max_workers: u64,
    max_normal_workers: u64,
    normal: Queue<(PacketBuffer, Subchannel)>,
    urgent: Queue<(PacketBuffer, Subchannel)>,
    proc_normal_packets: AtomicU64,
    proc_urgent_packets: AtomicU64,
    runtime: tokio::runtime::Handle,
    subscribers: Arc<Vec<Arc<dyn Subscriber>>>,
    #[cfg(feature = "static_workers")]
    sync: lockfree::queue::Queue<tokio::sync::oneshot::Sender<bool>>,
    workers: AtomicU64,
}

impl RecvPipeline {
    fn with_params(
        adnl: Arc<AdnlNode>,
        runtime: tokio::runtime::Handle,
        subscribers: Arc<Vec<Arc<dyn Subscriber>>>,
    ) -> Self {
        let mut max_workers = if let Some(pool) = adnl.config.recv_pipeline_pool {
            max(1, num_cpus::get() as u64 * pool as u64 / 100)
        } else {
            num_cpus::get() as u64
        };
        if max_workers > HISTORY_BITS as u64 {
            max_workers = HISTORY_BITS as u64
        }
        let max_normal_workers = if let Some(pool) = adnl.config.recv_priority_pool {
            let max_normal_workers = max(1, max_workers * (100 - pool as u64) / 100);
            if max_normal_workers == max_workers {
                max_workers += 1
            }
            max_normal_workers
        } else {
            max_workers
        };
        Self {
            // count: AtomicU64::new(0),
            max_workers,
            max_normal_workers,
            normal: Queue::new(
                #[cfg(feature = "telemetry")]
                adnl.telemetry.normal.recv_queue_packets.clone(),
            ),
            urgent: Queue::new(
                #[cfg(feature = "telemetry")]
                adnl.telemetry.urgent.recv_queue_packets.clone(),
            ),
            proc_normal_packets: AtomicU64::new(0),
            proc_urgent_packets: AtomicU64::new(0),
            runtime,
            subscribers,
            #[cfg(feature = "static_workers")]
            sync: lockfree::queue::Queue::new(),
            workers: AtomicU64::new(0),
            adnl,
        }
    }

    async fn get(&self) -> Option<(PacketBuffer, Subchannel)> {
        'again: loop {
            let ret = if let Some((data, subchannel)) = self.urgent.get() {
                if data.expired_at <= Version::get() as u32 {
                    #[cfg(feature = "telemetry")]
                    self.adnl.telemetry.urgent.proc_expired.update(1);
                    continue;
                } else {
                    self.proc_urgent_packets.fetch_add(1, Ordering::Relaxed);
                    Some((data, subchannel))
                }
            } else {
                loop {
                    let normal = self.proc_normal_packets.load(Ordering::Relaxed);
                    if normal >= self.max_normal_workers {
                        break None;
                    }
                    if self
                        .proc_normal_packets
                        .compare_exchange(normal, normal + 1, Ordering::Relaxed, Ordering::Relaxed)
                        .is_err()
                    {
                        continue;
                    }
                    if let Some((data, subchannel)) = self.normal.get() {
                        if data.expired_at <= Version::get() as u32 {
                            #[cfg(feature = "telemetry")]
                            self.adnl.telemetry.normal.proc_expired.update(1);
                            self.proc_normal_packets.fetch_sub(1, Ordering::Relaxed);
                            continue 'again;
                        } else {
                            break Some((data, subchannel));
                        }
                    } else {
                        self.proc_normal_packets.fetch_sub(1, Ordering::Relaxed);
                        break None;
                    }
                }
            };
            if ret.is_some() {
                // self.count.fetch_sub(1, Ordering::Relaxed);
                break ret;
            }
            // if self.count.load(Ordering::Relaxed) > 0 {
            //     continue
            // }
            #[cfg(feature = "static_workers")]
            {
                // tokio::time::sleep(Duration::from_millis(1)).await;
                let (sender, reader) = tokio::sync::oneshot::channel();
                self.sync.push(sender);
                if let Ok(true) = reader.await {
                    continue;
                }
            }
            break None;
        }
    }

    fn put(self: &Arc<Self>, data: Vec<u8>) {
        if self.adnl.stop.is_stopped() {
            return;
        }
        #[cfg(feature = "telemetry")]
        self.adnl.telemetry.recv_bytes.update(data.len() as u64);
        #[cfg(feature = "telemetry")]
        self.adnl.telemetry.recv_packets.update(1);
        let data = PacketBuffer {
            buf: data,
            counter: self.adnl.allocated.packets.clone().into(),
            expired_at: Version::get() as u32 + self.adnl.config.timeout_expire_queued_packet_sec,
        };
        #[cfg(feature = "telemetry")]
        self.adnl
            .telemetry
            .allocated
            .packets
            .update(self.adnl.allocated.packets.load(Ordering::Relaxed));
        let subchannel = self
            .adnl
            .channels_recv
            .get(&data.buf[0..32])
            .map_or(Subchannel::None, |subchannel| subchannel.val().clone());
        if let Subchannel::Urgent(_) = &subchannel {
            self.urgent.put((data, subchannel));
        } else {
            self.normal.put((data, subchannel));
        }
        // self.count.fetch_add(1, Ordering::Relaxed);
        #[cfg(feature = "static_workers")]
        if let Some(sender) = self.sync.pop() {
            sender.send(true).ok();
        }
        self.spawn();
    }

    async fn shutdown(&self) {
        log::warn!(target: TARGET, "Stopping recv pipeline...");
        let start = Instant::now();
        let mut next_log = Duration::from_millis(AdnlNode::TIMEOUT_STOPPING_LOG_MS);
        loop {
            #[cfg(feature = "static_workers")]
            while let Some(sender) = self.sync.pop() {
                sender.send(false).ok();
            }
            let workers = self.workers.load(Ordering::Relaxed);
            if workers == 0 {
                break;
            }
            let elapsed = start.elapsed();
            if elapsed >= next_log {
                let urgent_in_proc = self.proc_urgent_packets.load(Ordering::Relaxed);
                let normal_in_proc = self.proc_normal_packets.load(Ordering::Relaxed);
                log::warn!(
                    target: TARGET,
                    "Still stopping recv pipeline, waited {}ms: \
                    workers={workers}, processing urgent={urgent_in_proc} normal={normal_in_proc}",
                    elapsed.as_millis()
                );
                next_log = elapsed + Duration::from_millis(AdnlNode::TIMEOUT_STOPPING_LOG_MS);
            }
            tokio::task::yield_now().await
        }
        log::warn!(target: TARGET, "Recv pipeline stopped after {}ms", start.elapsed().as_millis());
    }

    fn spawn(self: &Arc<Self>) {
        loop {
            let workers = self.workers.load(Ordering::Relaxed);
            if workers >= self.max_workers {
                return;
            }
            if self
                .workers
                .compare_exchange(workers, workers + 1, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }
        }
        let recv = self.clone();
        self.runtime.spawn(async move {
            loop {
                let (mut packet, subchannel) = match recv.get().await {
                    Some(job) => job,
                    None => break,
                };
                let urgent = match &subchannel {
                    Subchannel::Urgent(_) => true,
                    _ => false,
                };
                #[cfg(feature = "telemetry")]
                recv.update_metric(urgent);
                match recv
                    .adnl
                    .clone()
                    .process_packet(&mut packet, subchannel, &recv.subscribers)
                    .await
                {
                    Err(e) => {
                        log::warn!(target: TARGET, "ERROR in ADNL receive pipeline: {e}");
                        #[cfg(feature = "telemetry")]
                        if urgent {
                            recv.adnl.telemetry.urgent.proc_invalid.update(1)
                        } else {
                            recv.adnl.telemetry.normal.proc_invalid.update(1)
                        }
                    }
                    _ => (),
                }
                if urgent {
                    recv.proc_urgent_packets.fetch_sub(1, Ordering::Relaxed);
                } else {
                    recv.proc_normal_packets.fetch_sub(1, Ordering::Relaxed);
                }
                #[cfg(feature = "telemetry")]
                recv.update_metric(urgent);
                // Yield execution after packet
                tokio::task::yield_now().await
            }
            recv.workers.fetch_sub(1, Ordering::Relaxed);
        });
    }

    #[cfg(feature = "telemetry")]
    fn update_metric(&self, urgent: bool) {
        if urgent {
            self.adnl
                .telemetry
                .urgent
                .proc_packets
                .update(self.proc_urgent_packets.load(Ordering::Relaxed))
        } else {
            self.adnl
                .telemetry
                .normal
                .proc_packets
                .update(self.proc_normal_packets.load(Ordering::Relaxed))
        }
    }
}

pub(crate) struct SendData {
    destination: u64,
    data: Vec<u8>,
    method: AdnlSendMethodDetailed,
}

impl Debug for SendData {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "destination {:x}, data {:x?}", self.destination, self.data)
    }
}

#[derive(Debug)]
enum SendJob {
    Data(SendData),
    Stop,
}

struct SendPipeline {
    // count: AtomicU64,
    normal: Queue<SendJob>,
    urgent: Queue<SendJob>,
    lock: Mutex<()>,
    sync: Condvar,
}

impl SendPipeline {
    const TIMEOUT_WAIT_QUEUE_MS: u64 = 10;

    fn new(
        #[cfg(feature = "telemetry")] normal_metric: Arc<Metric>,
        #[cfg(feature = "telemetry")] urgent_metric: Arc<Metric>,
    ) -> Self {
        Self {
            // count: AtomicU64::new(0),
            normal: Queue::new(
                #[cfg(feature = "telemetry")]
                normal_metric,
            ),
            urgent: Queue::new(
                #[cfg(feature = "telemetry")]
                urgent_metric,
            ),
            lock: Mutex::new(()),
            sync: Condvar::new(),
        }
    }

    fn get(&self) -> Result<SendJob> {
        loop {
            let ret = if let Some(ret) = self.urgent.get() { Some(ret) } else { self.normal.get() };
            if let Some(ret) = ret {
                // self.count.fetch_sub(1, Ordering::Relaxed);
                return Ok(ret);
            }
            // if self.count.load(Ordering::Relaxed) > 0 {
            //     continue
            // }
            let _mux = self
                .sync
                .wait_timeout(
                    self.lock.lock().map_err(|_| error!("Queue mutex poisoned"))?,
                    Duration::from_millis(Self::TIMEOUT_WAIT_QUEUE_MS),
                )
                .map_err(|_| error!("Queue wait failed"))?;
        }
    }

    fn put_normal(&self, job: SendJob) {
        self.put(&self.normal, job)
    }

    fn put_urgent(&self, job: SendJob) {
        self.put(&self.urgent, job)
    }

    fn put(&self, queue: &Queue<SendJob>, job: SendJob) {
        // self.count.fetch_add(1, Ordering::Relaxed);
        queue.put(job);
        self.sync.notify_one();
    }

    fn shutdown(&self) {
        self.put_urgent(SendJob::Stop)
    }
}

#[derive(Clone)]
enum Subchannel {
    None,
    Normal(Arc<AdnlChannel>),
    Stream(Arc<AdnlChannel>),
    Urgent(Arc<AdnlChannel>),
}

impl Subchannel {
    fn get_channel(&self) -> Option<(&Arc<AdnlChannel>, AdnlSendMethodDetailed)> {
        match self {
            Self::None => None,
            Self::Normal(x) => Some((x, AdnlSendMethodDetailed::FastNormal)),
            Self::Stream(x) => Some((x, AdnlSendMethodDetailed::Safe)),
            Self::Urgent(x) => Some((x, AdnlSendMethodDetailed::FastUrgent)),
        }
    }
}

// ADNL transfer
declare_counted!(
    struct Transfer {
        data: lockfree::map::Map<usize, Vec<u8>>,
        received: AtomicUsize,
        total: usize,
        updated: UpdatedAt,
    }
);

type ChannelId = [u8; 32];
type ChannelsRecv = lockfree::map::Map<ChannelId, Subchannel>;
type ChannelsSend = lockfree::map::Map<Arc<KeyId>, Arc<AdnlChannel>>;
type TransferId = [u8; 32];

#[cfg(feature = "telemetry")]
struct TelemetryAlloc {
    channels: Arc<Metric>,
    checkers: Arc<Metric>,
    packets: Arc<Metric>,
    peers: Arc<Metric>,
    transfers: Arc<Metric>,
}

#[cfg(feature = "telemetry")]
struct TelemetryByStage {
    proc_packets: Arc<Metric>,        // Packets in processing
    proc_expired: Arc<MetricBuilder>, // Packets expired in transit
    proc_invalid: Arc<MetricBuilder>, // Packets with errors in processing
    proc_skipped: Arc<MetricBuilder>, // Skipped packets (due to failed checks)
    proc_success: Arc<MetricBuilder>, // Packets successfully processed
    proc_unknown: Arc<MetricBuilder>, // Packets to unknown address
    recv_queue_packets: Arc<Metric>,  // Queued received packets
    send_queue_packets: Arc<Metric>,  // Queued packets to send
    send_tags: lockfree::map::Map<u32, Arc<MetricBuilder>>,
}

#[cfg(feature = "telemetry")]
impl TelemetryByStage {
    fn with_urgency(urgent: bool) -> Self {
        let urgency = if urgent { "urgent" } else { "normal" };
        Self {
            proc_packets: Telemetry::create_metric(
                format!("{} packets, in progress", urgency).as_str(),
            ),
            proc_expired: Telemetry::create_metric_builder(
                format!("proc {} expired, packets/sec", urgency).as_str(),
            ),
            proc_invalid: Telemetry::create_metric_builder(
                format!("proc {} invalid, packets/sec", urgency).as_str(),
            ),
            proc_skipped: Telemetry::create_metric_builder(
                format!("proc {} skipped, packets/sec", urgency).as_str(),
            ),
            proc_success: Telemetry::create_metric_builder(
                format!("proc {} success, packets/sec", urgency).as_str(),
            ),
            proc_unknown: Telemetry::create_metric_builder(
                format!("proc {} unknown, packets/sec", urgency).as_str(),
            ),
            recv_queue_packets: Telemetry::create_metric(
                format!("{} packets, recv queue", urgency).as_str(),
            ),
            send_queue_packets: Telemetry::create_metric(
                format!("{} packets, send queue", urgency).as_str(),
            ),
            send_tags: lockfree::map::Map::new(),
        }
    }
}

#[cfg(feature = "telemetry")]
declare_counted!(
    struct TelemetryCheck {
        start: Instant,
        info: String,
    }
);

#[cfg(feature = "telemetry")]
pub struct Telemetry {
    pub packet_size: Arc<Metric>,
    normal: TelemetryByStage,
    urgent: TelemetryByStage,
    drop_packets: Arc<MetricBuilder>,
    recv_bytes: Arc<MetricBuilder>,
    recv_packets: Arc<MetricBuilder>,
    send_packets: Arc<MetricBuilder>,
    allocated: TelemetryAlloc,
    check_id: AtomicU64,
    check_map: lockfree::map::Map<u64, TelemetryCheck>,
    check_queue: lockfree::queue::Queue<u64>,
    checkers: Arc<AtomicU64>,
    printer: TelemetryPrinter,
}

#[cfg(feature = "telemetry")]
impl Telemetry {
    fn add_check(&self, info: String) -> Result<u64> {
        loop {
            let id = self.check_id.fetch_add(1, Ordering::Relaxed);
            let added = add_counted_object_to_map(&self.check_map, id, || {
                let check = TelemetryCheck {
                    start: Instant::now(),
                    info: info.clone(),
                    counter: self.checkers.clone().into(),
                };
                self.allocated.checkers.update(self.checkers.load(Ordering::Relaxed));
                Ok(check)
            })?;
            if added {
                self.check_queue.push(id);
                break Ok(id);
            }
        }
    }

    fn create_metric(name: &str) -> Arc<Metric> {
        Metric::without_totals(name, AdnlNode::PERIOD_AVERAGE_SEC)
    }

    fn create_metric_with_total(name: &str) -> Arc<Metric> {
        Metric::with_total_amount(name, AdnlNode::PERIOD_AVERAGE_SEC)
    }

    fn create_metric_builder(name: &str) -> Arc<MetricBuilder> {
        MetricBuilder::with_metric_and_period(
            Self::create_metric_with_total(name),
            AdnlNode::PERIOD_MEASURE_NANO,
        )
    }

    fn drop_check(&self, id: u64) {
        self.check_map.remove(&id);
    }

    fn evaluate_checks(&self, config: &AdnlNodeConfig) {
        let mut until = None;
        while let Some(id) = self.check_queue.pop() {
            if let Some(until_id) = &until {
                if *until_id == id {
                    self.check_queue.push(id);
                    break;
                }
            }
            if let Some(check) = self.check_map.get(&id) {
                let check = check.val();
                let elapsed = check.start.elapsed().as_micros() as u64;
                if elapsed >= config.timeout_check_packet_processing_mcs {
                    log::warn!(
                        target: TARGET,
                        "Too long processing of {}: {} micros",
                        check.info,
                        elapsed
                    )
                }
                if until.is_none() {
                    until.replace(id);
                }
                self.check_queue.push(id)
            }
        }
    }

    fn get_message_info(msg: &AdnlMessage) -> String {
        match msg {
            AdnlMessage::Adnl_Message_Part(part) => {
                format!("AdnlMessagePart offset {} of {}", part.offset, part.total_size)
            }
            AdnlMessage::Adnl_Message_Answer(answer) => {
                format!("AdnlMessageAnswer tag {:08x}", tag_from_data(&answer.answer))
            }
            AdnlMessage::Adnl_Message_ConfirmChannel(_) => "AdnlMessageConfirmChannel".to_string(),
            AdnlMessage::Adnl_Message_CreateChannel(_) => "AdnlMessageCreateChannel".to_string(),
            AdnlMessage::Adnl_Message_Custom(custom) => {
                let data = &custom.data;
                let mut tag = tag_from_data(data);
                // Uncover Overlay.Message internal message if possible
                if (tag == 0x75252420) && (data.len() >= 40) {
                    tag = tag_from_data(&data[36..]);
                    format!("AdnlMessageCustom/OverlayMessage tag {:08x}", tag)
                } else {
                    format!("AdnlMessageCustom tag {:08x}", tag)
                }
            }
            AdnlMessage::Adnl_Message_Nop => "AdnlMessageNop".to_string(),
            AdnlMessage::Adnl_Message_Query(query) => {
                let data = &query.query;
                let mut tag = tag_from_data(data);
                // Uncover Overlay.Query internal message if possible
                if (tag == 0xCCFD8443) && (data.len() >= 40) {
                    tag = tag_from_data(&data[36..]);
                    format!("AdnlMessageQuery/OverlayQuery tag {:08x}", tag)
                } else {
                    format!("AdnlMessageQuery tag {:08x}", tag)
                }
            }
            AdnlMessage::Adnl_Message_Reinit(_) => "AdnlMessageReinit".to_string(),
        }
    }
}

#[cfg(feature = "dump")]
#[derive(Debug)]
struct DumpRecord {
    alive: bool,
    key_id: Arc<KeyId>,
    msg: String,
}

#[cfg(feature = "dump")]
struct Dump {
    path: PathBuf,
    reader: lockfree::queue::Queue<tokio::sync::mpsc::UnboundedReceiver<DumpRecord>>,
    sender: tokio::sync::mpsc::UnboundedSender<DumpRecord>,
}

enum LoopbackData {
    Message((AdnlMessage, AdnlPeers)),
    Packet(Vec<u8>),
}

type LoopbackReader = tokio::sync::mpsc::UnboundedReceiver<LoopbackData>;
type LoopbackSender = tokio::sync::mpsc::UnboundedSender<LoopbackData>;

struct AdnlAlloc {
    channels: Arc<AtomicU64>,
    packets: Arc<AtomicU64>,
    peers: Arc<AtomicU64>,
    transfers: Arc<AtomicU64>,
}

/// The way how ADNL should send the data
#[derive(Clone)]
pub enum AdnlSendMethod {
    Fast, // No delivery guarantees
    Safe, // Guaranteed delivery
}

/// The detailed way how ADNL sends the data
#[derive(Clone, PartialEq)]
pub enum AdnlSendMethodDetailed {
    FastNormal, // Low priority, no delivery guarantees
    FastUrgent, // High priority, no delivery guarantees
    Safe,       // Guaranteed delivery, no priorities
}

impl Display for AdnlSendMethodDetailed {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            Self::FastNormal => "normal",
            Self::FastUrgent => "urgent",
            Self::Safe => "stream",
        };
        write!(f, "{msg}")
    }
}

/// ADNL status for operation
pub struct AdnlStatus {
    pub method: AdnlSendMethodDetailed, // How data was sent
    pub reset_at: Option<u64>,          // Expected channel reset time if there is a channel
}

impl AdnlStatus {
    fn with_params(method: AdnlSendMethodDetailed, channel: &Option<Arc<AdnlChannel>>) -> Self {
        Self { method, reset_at: channel.as_ref().map(|ch| ch.get_reset_at()) }
    }
}

/// ADNL reply with status
pub struct AdnlReplyWithStatus {
    pub reply: Option<TLObject>,
    pub status: AdnlStatus,
}

/// ADNL node
pub struct AdnlNode {
    channels_incinerator: lockfree::map::SharedIncin<Arc<KeyId>, Arc<AdnlChannel>>,
    channels_recv: Arc<ChannelsRecv>,
    config: AdnlNodeConfig,
    options: AtomicU32,
    peers: lockfree::map::Map<Arc<KeyId>, Arc<Peers>>,
    queries: Arc<QueryCache>,
    queue_monitor_queries: lockfree::queue::Queue<(u64, QueryId)>,
    queue_send_loopback_packets: LoopbackSender,
    queue_send_loopback_readers: lockfree::queue::Queue<LoopbackReader>,
    send_bytes: Arc<MetricBuilder>, // Needed for throughput management
    send_pipeline: SendPipeline,
    start: Instant,
    start_timestamp: i32,
    stop: Arc<Stopper>,
    transfers: Arc<lockfree::map::Map<TransferId, Arc<Transfer>>>,
    #[cfg(feature = "telemetry")]
    telemetry: Telemetry,
    allocated: AdnlAlloc,
    #[cfg(feature = "dump")]
    dump: Option<Dump>,
}

impl Drop for AdnlNode {
    fn drop(&mut self) {
        log::warn!(target: TARGET, "ADNL node dropped");
    }
}

impl AdnlNode {
    /// ADNL options
    pub const OPTION_FORCE_COMPRESSION: u32 = 0x0100; // Force traffic compression
    pub const OPTION_FORCE_VERSIONING: u32 = 0x0200; // Force ADNL versioning
    pub const OPTION_MASK_TIMEOUT_CHANNEL_RESET_SEC: u32 = 0x00FF; // Timeout value mask
    pub const OPTION_UDP_TCP: u32 = 0x1000; // Mixed UDP/TCP stack

    /// ADNL versions
    const VERSION_INITIAL: u16 = 0x0000;

    const MASK_LOOPBACK: u32 = 0x00000001;
    const MASK_RECV: u32 = 0x00000002;
    const MASK_SEND: u32 = 0x00000004;
    const MASK_SUBSCRIBERS: u32 = 0x00000008;
    pub(crate) const MASK_TRANSPORT: u32 = 0x00000010;
    const MASK_WATCHDOG: u32 = 0x00000020;
    #[cfg(feature = "dump")]
    const MASK_DUMP: u32 = 0x00000040;

    const CLOCK_TOLERANCE_SEC: i32 = 60;
    const DEFAULT_TIMEOUT_CHANNEL_RESET_SEC: u64 = 30;
    const MAX_ADNL_MESSAGE: usize = 1024;
    const MAX_PRIORITY_ATTEMPTS: u64 = 10;
    const PERIOD_AVERAGE_SEC: u64 = 5;
    const PERIOD_MEASURE_NANO: u64 = 1000000000;
    const TIMEOUT_ADDRESS_SEC: i32 = 1000;
    const TIMEOUT_QUERY_MIN_MS: u64 = 500;
    const TIMEOUT_QUERY_MAX_MS: u64 = 5000;
    const TIMEOUT_QUERY_STOP_MS: u64 = 1;
    const TIMEOUT_SHUTDOWN_MS: u64 = 50;
    const TIMEOUT_STOPPING_LOG_MS: u64 = 500;
    const TIMEOUT_TRANSFER_SEC: u64 = 5;

    /// Constructor
    pub async fn with_config(
        mut config: AdnlNodeConfig,
        #[cfg(feature = "dump")] dump_path: Option<PathBuf>,
    ) -> Result<Arc<Self>> {
        let incinerator = lockfree::map::SharedIncin::new();
        let peers = lockfree::map::Map::new();
        let mut added = false;
        for key in config.keys.iter() {
            peers.insert(key.val().id().clone(), Peers::with_incinerator(&incinerator));
            added = true
        }
        if !added {
            fail!("No keys configured for node");
        }
        if config.ip_address.ip() == 0 {
            let ip = external_ip::ConsensusBuilder::new()
                .add_sources(external_ip::get_http_sources::<external_ip::Sources>())
                .build()
                .get_consensus()
                .await;
            if let Some(IpAddr::V4(ip)) = ip {
                config.ip_address.set_ip(u32::from_be_bytes(ip.octets()))
            } else {
                fail!("Cannot obtain own external IP address");
            }
        }
        let (queue_send_loopback_sender, queue_send_loopback_reader) =
            tokio::sync::mpsc::unbounded_channel();
        let send_bytes = MetricBuilder::with_metric_and_period(
            Metric::with_total_amount("socket send, bytes/sec", Self::PERIOD_AVERAGE_SEC),
            Self::PERIOD_MEASURE_NANO,
        );
        #[cfg(feature = "telemetry")]
        let telemetry = {
            let normal = TelemetryByStage::with_urgency(false);
            let urgent = TelemetryByStage::with_urgency(true);
            let packet_size = Telemetry::create_metric("Packet size, bytes");
            let drop_packets = Telemetry::create_metric_builder("socket drop, packets/sec");
            let recv_bytes = Telemetry::create_metric_builder("socket recv, bytes/sec");
            let recv_packets = Telemetry::create_metric_builder("socket recv, packets/sec");
            let send_packets = Telemetry::create_metric_builder("socket send, packets/sec");
            let allocated = TelemetryAlloc {
                channels: Telemetry::create_metric("Alloc ADNL channels"),
                checkers: Telemetry::create_metric("Alloc ADNL checkers"),
                packets: Telemetry::create_metric("Alloc ADNL recv packets"),
                peers: Telemetry::create_metric("Alloc ADNL peers"),
                transfers: Telemetry::create_metric("Alloc ADNL transfers"),
            };
            let printer = TelemetryPrinter::with_params(
                "ADNL node",
                Self::PERIOD_AVERAGE_SEC,
                vec![
                    TelemetryItem::MetricBuilder(recv_bytes.clone()),
                    TelemetryItem::MetricBuilder(recv_packets.clone()),
                    TelemetryItem::Metric(urgent.recv_queue_packets.clone()),
                    TelemetryItem::Metric(normal.recv_queue_packets.clone()),
                    TelemetryItem::Metric(urgent.proc_packets.clone()),
                    TelemetryItem::MetricBuilder(urgent.proc_expired.clone()),
                    TelemetryItem::MetricBuilder(urgent.proc_invalid.clone()),
                    TelemetryItem::MetricBuilder(urgent.proc_unknown.clone()),
                    TelemetryItem::MetricBuilder(urgent.proc_skipped.clone()),
                    TelemetryItem::MetricBuilder(urgent.proc_success.clone()),
                    TelemetryItem::Metric(normal.proc_packets.clone()),
                    TelemetryItem::MetricBuilder(normal.proc_expired.clone()),
                    TelemetryItem::MetricBuilder(normal.proc_invalid.clone()),
                    TelemetryItem::MetricBuilder(normal.proc_unknown.clone()),
                    TelemetryItem::MetricBuilder(normal.proc_skipped.clone()),
                    TelemetryItem::MetricBuilder(normal.proc_success.clone()),
                    TelemetryItem::Metric(urgent.send_queue_packets.clone()),
                    TelemetryItem::Metric(normal.send_queue_packets.clone()),
                    TelemetryItem::MetricBuilder(drop_packets.clone()),
                    TelemetryItem::MetricBuilder(send_bytes.clone()),
                    TelemetryItem::MetricBuilder(send_packets.clone()),
                    TelemetryItem::Metric(packet_size.clone()),
                    TelemetryItem::Metric(allocated.packets.clone()),
                    TelemetryItem::Metric(allocated.channels.clone()),
                    TelemetryItem::Metric(allocated.peers.clone()),
                    TelemetryItem::Metric(allocated.transfers.clone()),
                    TelemetryItem::Metric(allocated.checkers.clone()),
                ],
            );
            Telemetry {
                packet_size,
                normal,
                urgent,
                drop_packets,
                recv_bytes,
                recv_packets,
                send_packets,
                allocated,
                check_id: AtomicU64::new(0),
                check_map: lockfree::map::Map::new(),
                check_queue: lockfree::queue::Queue::new(),
                checkers: Arc::new(AtomicU64::new(0)),
                printer,
            }
        };
        let allocated = AdnlAlloc {
            channels: Arc::new(AtomicU64::new(0)),
            packets: Arc::new(AtomicU64::new(0)),
            peers: Arc::new(AtomicU64::new(0)),
            transfers: Arc::new(AtomicU64::new(0)),
        };
        let ret = Self {
            channels_incinerator: incinerator,
            channels_recv: Arc::new(lockfree::map::Map::new()),
            config,
            options: AtomicU32::new(Self::DEFAULT_TIMEOUT_CHANNEL_RESET_SEC as u32),
            peers,
            queries: Arc::new(lockfree::map::Map::new()),
            queue_monitor_queries: lockfree::queue::Queue::new(),
            queue_send_loopback_packets: queue_send_loopback_sender,
            queue_send_loopback_readers: lockfree::queue::Queue::new(),
            send_bytes,
            send_pipeline: SendPipeline::new(
                #[cfg(feature = "telemetry")]
                telemetry.normal.send_queue_packets.clone(),
                #[cfg(feature = "telemetry")]
                telemetry.urgent.send_queue_packets.clone(),
            ),
            start: Instant::now(),
            start_timestamp: Version::get(),
            stop: Arc::new(Stopper::new()),
            transfers: Arc::new(lockfree::map::Map::new()),
            #[cfg(feature = "telemetry")]
            telemetry,
            allocated,
            #[cfg(feature = "dump")]
            dump: if let Some(dump_path) = dump_path {
                let (sender, reader) = tokio::sync::mpsc::unbounded_channel();
                let dump = Dump { path: dump_path, reader: lockfree::queue::Queue::new(), sender };
                dump.reader.push(reader);
                Some(dump)
            } else {
                None
            },
        };
        ret.queue_send_loopback_readers.push(queue_send_loopback_reader);
        Ok(Arc::new(ret))
    }

    pub fn check(&self) {
        // if self.queue_send_packets.count.load(Ordering::Relaxed) != 0 {
        //     panic!("Problem with queue")
        // }
    }

    /// Start node
    pub async fn start(
        self: &Arc<Self>,
        transport: impl Fn(&Arc<Self>) -> Result<(Box<dyn AdnlSender>, Box<dyn AdnlReceiver>)>,
        mut subscribers: Vec<Arc<dyn Subscriber>>,
    ) -> Result<()> {
        let mut queue_send_loopback_reader = None;
        for _ in 0..1 {
            match self.queue_send_loopback_readers.pop() {
                Some(reader) => queue_send_loopback_reader = Some(reader),
                _ => fail!("ADNL node already started"),
            }
        }
        let mut queue_send_loopback_reader =
            queue_send_loopback_reader.ok_or_else(|| error!("Loopback reader is not set"))?;
        let (sender, mut receiver) = transport(self)?;
        subscribers.push(Arc::new(AdnlPingSubscriber));
        // Subscribers poll
        self.stop.acquire(Self::MASK_SUBSCRIBERS);
        let start = Arc::new(Instant::now());
        let subscribers = Arc::new(subscribers);
        let subscribers_local = subscribers.clone();
        let subscribers_stop = Arc::new(AtomicU32::new(0));
        for subscriber in subscribers.iter() {
            let stop = self.stop.clone();
            let start = start.clone();
            let subscriber = subscriber.clone();
            let subscribers_stop = subscribers_stop.clone();
            subscribers_stop.fetch_add(1, Ordering::Relaxed);
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_millis(Self::TIMEOUT_QUERY_STOP_MS)).await;
                    if stop.is_stopped() {
                        break;
                    }
                    subscriber.poll(&start).await;
                }
                if subscribers_stop.fetch_sub(1, Ordering::Relaxed) == 1 {
                    stop.release(Self::MASK_SUBSCRIBERS);
                    log::warn!(target: TARGET, "Node subscriber poll stopped");
                }
            });
        }
        let recv_pipeline = Arc::new(RecvPipeline::with_params(
            self.clone(),
            tokio::runtime::Handle::current(),
            subscribers,
        ));
        // Monitoring watchdog
        let status = Arc::new(AtomicU64::new(0));
        let stat_mon = status.clone();
        let node = self.clone();
        tokio::spawn(async move {
            let mut check = 0;
            loop {
                if node.stop.is_stopped() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(Self::TIMEOUT_QUERY_STOP_MS)).await;
                check += Self::TIMEOUT_QUERY_STOP_MS;
                if check > 3000 {
                    log::info!(
                        target: TARGET_QUERY,
                        "ADNL watcher, status {:010x}",
                        stat_mon.load(Ordering::Relaxed)
                    );
                    check = 0;
                }
            }
        });
        // Stopping watchdog
        let node = self.clone();
        let recv_stop = recv_pipeline.clone();
        tokio::spawn(async move {
            let mut monitor_queries: Vec<(u128, QueryId)> = Vec::new();
            #[cfg(feature = "telemetry")]
            let mut last_check = Instant::now();
            let ts_start = Instant::now();
            node.stop.acquire(Self::MASK_WATCHDOG);
            loop {
                tokio::time::sleep(Duration::from_millis(Self::TIMEOUT_QUERY_STOP_MS)).await;
                let ts = (ts_start.elapsed().as_secs() << 8) as u64;
                status.store(ts | 1, Ordering::Relaxed);
                #[cfg(feature = "telemetry")]
                {
                    node.telemetry
                        .allocated
                        .channels
                        .update(node.allocated.channels.load(Ordering::Relaxed));
                    node.telemetry
                        .allocated
                        .checkers
                        .update(node.telemetry.checkers.load(Ordering::Relaxed));
                    node.telemetry
                        .allocated
                        .packets
                        .update(node.allocated.packets.load(Ordering::Relaxed));
                    node.telemetry
                        .allocated
                        .peers
                        .update(node.allocated.peers.load(Ordering::Relaxed));
                    node.telemetry
                        .allocated
                        .transfers
                        .update(node.allocated.transfers.load(Ordering::Relaxed));
                    node.telemetry.printer.try_print();
                    if last_check.elapsed().as_secs() >= Self::PERIOD_AVERAGE_SEC {
                        node.telemetry.evaluate_checks(&node.config);
                        last_check = Instant::now();
                    }
                }
                status.store(ts | 2, Ordering::Relaxed);
                if node.stop.is_stopped() {
                    node.send_pipeline.shutdown();
                    let empty = KeyId::from_data([0u8; 32]);
                    let peers = AdnlPeers::with_keys(empty.clone(), empty.clone());
                    let stop = LoopbackData::Message((AdnlMessage::Adnl_Message_Nop, peers));
                    if let Err(e) = node.queue_send_loopback_packets.send(stop) {
                        log::warn!(target: TARGET, "Cannot close node loopback: {}", e);
                    }
                    recv_stop.shutdown().await;
                    #[cfg(feature = "dump")]
                    if let Some(dump) = node.dump.as_ref() {
                        let stop = DumpRecord { alive: false, key_id: empty, msg: String::new() };
                        if let Err(e) = dump.sender.send(stop) {
                            log::warn!(target: TARGET, "Cannot close node dump: {}", e);
                        }
                    }
                    break;
                }
                status.store(ts | 3, Ordering::Relaxed);
                let elapsed = start.elapsed().as_millis();
                let mut drop = monitor_queries.len();
                while drop > 0 {
                    let (timeout, query_id) = monitor_queries[drop - 1];
                    if timeout > elapsed {
                        break;
                    }
                    log::info!(
                        target: TARGET_QUERY,
                        "Try dropping query {:02x}{:02x}{:02x}{:02x}",
                        query_id[0], query_id[1], query_id[2], query_id[3]
                    );
                    match Self::update_query(&node.queries, query_id, None).await {
                        Err(e) => log::info!(
                            target: TARGET_QUERY,
                            "ERROR: {} when dropping query {:02x}{:02x}{:02x}{:02x}",
                            e, query_id[0], query_id[1], query_id[2], query_id[3]
                        ),
                        Ok(true) => log::info!(
                            target: TARGET_QUERY,
                            "Dropped query {:02x}{:02x}{:02x}{:02x}",
                            query_id[0], query_id[1], query_id[2], query_id[3]
                        ),
                        _ => (),
                    }
                    drop -= 1;
                }
                monitor_queries.drain(drop..);
                status.store(ts | 4, Ordering::Relaxed);
                while let Some((timeout, query_id)) = node.queue_monitor_queries.pop() {
                    let deadline = elapsed + timeout as u128;
                    // Binary search in descending-sorted vec: find first index where t <= deadline
                    let insert = monitor_queries.partition_point(|&(t, _)| t > deadline);
                    monitor_queries.insert(insert, (deadline, query_id));
                }
                status.store(ts | 5, Ordering::Relaxed);
            }
            node.stop.release(Self::MASK_WATCHDOG);
            log::warn!(target: TARGET, "Node stopping watchdog stopped");
        });
        // Remote connections
        let node = self.clone();
        let recv = recv_pipeline.clone();
        thread::Builder::new().name("ADNL receiver".into()).spawn(move || {
            node.stop.acquire(Self::MASK_RECV);
            loop {
                if node.stop.is_stopped() {
                    break;
                }
                let data = match receiver.recv() {
                    Ok(None) => continue,
                    Ok(Some(data)) => {
                        if data.len() < 32 {
                            log::warn!(
                                target: TARGET,
                                "ERROR <-- ADNL packet is too short ({})",
                                data.len()
                            );
                            continue;
                        } else {
                            data
                        }
                    }
                    Err(err) => {
                        log::warn!(target: TARGET, "ERROR in ADNL receiver: {err}");
                        continue;
                    }
                };
                #[cfg(feature = "telemetry")]
                node.telemetry.packet_size.update(data.len() as u64);
                recv.put(data.to_vec());
            }
            node.stop.release(Self::MASK_RECV);
            log::warn!(target: TARGET, "Node socket receiver stopped");
        })?;
        let node = self.clone();
        thread::Builder::new().name("ADNL sender".into()).spawn(move || {
            node.stop.acquire(Self::MASK_SEND);
            loop {
                let job = node.send_pipeline.get();
                let (job, stop) = match job {
                    Ok(SendJob::Data(job)) => (job, false),
                    Ok(SendJob::Stop) => (
                        // Send closing packet to 127.0.0.1:port
                        SendData {
                            destination: 0x7F0000010000u64 | node.config.ip_address.port() as u64,
                            data: Vec::new(),
                            method: AdnlSendMethodDetailed::FastNormal,
                        },
                        true,
                    ),
                    Err(e) => {
                        log::error!(target: TARGET, "ERROR in send queue --> {}", e);
                        continue;
                    }
                };
                // Manage the throughput
                let packet_len = job.data.len() as u64;
                let drop =
                    node.send_bytes.ongoing_value() + packet_len > (node.config.throughput as u64);
                #[cfg(feature = "telemetry")]
                node.telemetry.packet_size.update(packet_len);
                if drop {
                    #[cfg(feature = "telemetry")]
                    node.telemetry.drop_packets.update(1);
                } else {
                    let socket_addr = SocketAddr::new(
                        IpAddr::from(((job.destination >> 16) as u32).to_be_bytes()),
                        job.destination as u16,
                    );
                    let addr: socket2::SockAddr = socket_addr.into();
                    let len = job.data.len();
                    if let AdnlSendMethodDetailed::Safe = job.method {
                        match sender.send_safe(job.data, addr, &node) {
                            Ok(None) => log::info!(
                                target: TARGET,
                                "Pending TCP send {len} bytes to {socket_addr}"
                            ),
                            Ok(Some(size)) => node.after_send(Ok(size), len),
                            Err(e) => node.after_send(Err(e), len),
                        }
                    } else {
                        node.after_send(sender.send_fast(&job.data, addr), len)
                    }
                }
                if node.stop.is_stopped() {
                    if stop {
                        break;
                    }
                }
            }
            node.stop.release(Self::MASK_SEND);
            log::warn!(target: TARGET, "Node socket sender stopped");
        })?;
        // Local connections (loopback)
        let node = self.clone();
        tokio::spawn(async move {
            node.stop.acquire(Self::MASK_LOOPBACK);
            while let Some(data) = queue_send_loopback_reader.recv().await {
                if node.stop.is_stopped() {
                    break;
                }
                match data {
                    LoopbackData::Message((msg, peers)) => {
                        match &msg {
                            AdnlMessage::Adnl_Message_Answer(_) => (),
                            AdnlMessage::Adnl_Message_Custom(_) => (),
                            AdnlMessage::Adnl_Message_Query(_) => (),
                            x => {
                                log::warn!(
                                    target: TARGET,
                                    "Unsupported loopback ADNL message {:?}", x
                                );
                                continue;
                            }
                        }
                        let node = node.clone();
                        let peers =
                            AdnlPeers::with_keys(peers.other().clone(), peers.local().clone());
                        let subscribers = subscribers_local.clone();
                        tokio::spawn(async move {
                            match node
                                .process_message(
                                    &subscribers,
                                    msg,
                                    &peers,
                                    &AdnlSendMethodDetailed::FastNormal,
                                )
                                .await
                            {
                                Err(e) => log::warn!(target: TARGET, "ERROR --> {}", e),
                                _ => (),
                            }
                        });
                    }
                    LoopbackData::Packet(buf) => recv_pipeline.put(buf),
                }
            }
            Self::graceful_close(queue_send_loopback_reader).await;
            node.stop.release(Self::MASK_LOOPBACK);
            log::warn!(target: TARGET, "Node loopback stopped");
        });
        // Traffic dump
        #[cfg(feature = "dump")]
        if let Some(dump) = self.dump.as_ref() {
            let stop = self.stop.clone();
            let mut dump_path = PathBuf::from(&dump.path);
            dump_path.push("alive");
            create_dir_all(&dump_path)?;
            dump_path.pop();
            if let Some(mut reader) = dump.reader.pop() {
                tokio::spawn(async move {
                    fn prepare(path: &PathBuf, file: &String, alive: bool) -> Result<PathBuf> {
                        let dst =
                            if alive { path.join("alive").join(file) } else { path.join(file) };
                        if !dst.exists() {
                            let src =
                                if alive { path.join(file) } else { path.join("alive").join(file) };
                            if src.exists() {
                                rename(src, dst.as_path())?
                            }
                        }
                        Ok(dst)
                    }
                    fn print(path: PathBuf, msg: String) -> Result<()> {
                        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
                        writeln!(file, "{}", msg)?;
                        Ok(())
                    }
                    stop.fetch_or(Self::MASK_DUMP, Ordering::Relaxed);
                    while let Some(record) = reader.recv().await {
                        if (stop.load(Ordering::Relaxed) & Self::MASK_STOP) != 0 {
                            break;
                        }
                        let file = format!("{}", record.key_id).replace("/", "_");
                        let file = match prepare(&dump_path, &file, record.alive) {
                            Ok(file) => file,
                            Err(e) => {
                                log::warn!(target: TARGET, "Error during dump: {}", e);
                                continue;
                            }
                        };
                        if let Err(e) = print(file, record.msg) {
                            log::warn!(target: TARGET, "Error during dump: {}", e);
                        }
                    }
                    Self::graceful_close(reader).await;
                    stop.fetch_and(!Self::MASK_DUMP, Ordering::Relaxed);
                    log::warn!(target: TARGET, "Node dump stopped");
                });
            }
        }
        Ok(())
    }

    /// Start node over UDP-only transport
    pub async fn start_over_udp(
        self: &Arc<Self>,
        subscribers: Vec<Arc<dyn Subscriber>>,
    ) -> Result<()> {
        self.start(udp_sender_receiver, subscribers).await
    }

    /// Start node over UDP-TCP hybrid transport
    pub async fn start_over_udp_tcp(
        self: &Arc<Self>,
        subscribers: Vec<Arc<dyn Subscriber>>,
    ) -> Result<()> {
        self.set_options(Self::OPTION_UDP_TCP);
        self.start(udp_tcp_sender_receiver, subscribers).await
    }

    /// Stop node
    pub async fn stop(&self) {
        log::warn!(target: TARGET, "Stopping ADNL node...");
        self.stop.stop();
        let mut elapsed_ms: u64 = 0;
        loop {
            tokio::time::sleep(Duration::from_millis(Self::TIMEOUT_QUERY_STOP_MS)).await;
            elapsed_ms += Self::TIMEOUT_QUERY_STOP_MS;
            let running = self.stop.still_running();
            if running == 0 {
                break;
            }
            if elapsed_ms % Self::TIMEOUT_STOPPING_LOG_MS == 0 {
                log::warn!(
                    target: TARGET,
                    "Still stopping ADNL node ({running:x}), waited {elapsed_ms}ms..."
                );
            }
        }
        tokio::time::sleep(Duration::from_millis(Self::TIMEOUT_SHUTDOWN_MS)).await;
        log::warn!(target: TARGET, "ADNL node stopped");
    }

    /// Add key
    pub fn add_key(&self, key: Arc<dyn KeyOption>, tag: usize) -> Result<Arc<KeyId>> {
        let ret = self.config.add_key(key, tag)?;
        add_unbound_object_to_map(&self.peers, ret.clone(), || {
            Ok(Peers::with_incinerator(&self.channels_incinerator))
        })?;
        Ok(ret)
    }

    /// Add dynamic telemetry metric
    #[cfg(feature = "telemetry")]
    pub fn add_metric(&self, name: &str) -> Arc<Metric> {
        let ret = Telemetry::create_metric(name);
        self.telemetry.printer.add_metric(TelemetryItem::Metric(ret.clone()));
        ret
    }

    /// Add peer
    pub fn add_peer(
        &self,
        local_key: &Arc<KeyId>,
        peer_ip_address: &IpAddress,
        peer_ip_address_quic: Option<&IpAddress>,
        peer_key: &Arc<dyn KeyOption>,
    ) -> Result<Option<Arc<KeyId>>> {
        if peer_key.id() == local_key {
            return Ok(None);
        }
        let mut error = None;
        let mut ret = peer_key.id().clone();
        let result =
            self.peers(local_key)?.map_of.insert_with(ret.clone(), |key, inserted, found| {
                if let Some((_, found)) = found {
                    ret = key.clone();
                    found.address.update(peer_ip_address, peer_ip_address_quic);
                    lockfree::map::Preview::Discard
                } else if inserted.is_some() {
                    ret = key.clone();
                    lockfree::map::Preview::Keep
                } else {
                    let address = AdnlNodeAddress::from_ip_addresses_and_key(
                        peer_ip_address,
                        peer_ip_address_quic,
                        peer_key,
                    );
                    match address {
                        Ok(address) => {
                            #[cfg(feature = "telemetry")]
                            let peers = AdnlPeers::with_keys(local_key.clone(), ret.clone());
                            let peer = Peer {
                                address,
                                recv_state: PeerState::for_receive_with_reinit_date(
                                    self.start_timestamp,
                                    #[cfg(feature = "telemetry")]
                                    self,
                                    #[cfg(feature = "telemetry")]
                                    &peers,
                                    #[cfg(feature = "telemetry")]
                                    None,
                                ),
                                send_state: PeerState::for_send(
                                    #[cfg(feature = "telemetry")]
                                    self,
                                    #[cfg(feature = "telemetry")]
                                    &peers,
                                    #[cfg(feature = "telemetry")]
                                    None,
                                ),
                                counter: self.allocated.peers.clone().into(),
                            };
                            #[cfg(feature = "telemetry")]
                            self.telemetry
                                .allocated
                                .peers
                                .update(self.allocated.peers.load(Ordering::Relaxed));
                            lockfree::map::Preview::New(peer)
                        }
                        Err(err) => {
                            error = Some(err);
                            lockfree::map::Preview::Discard
                        }
                    }
                }
            });
        if let Some(error) = error {
            return Err(error);
        }
        if let lockfree::map::Insertion::Created = result {
            log::debug!(
                target: TARGET,
                "Added ADNL peer with IP {}, keyID {}, key {} to {}",
                peer_ip_address,
                base64_encode(peer_key.id().data()),
                base64_encode(peer_key.pub_key()?),
                base64_encode(local_key.data())
            )
        }
        Ok(Some(ret))
    }

    /// Build address list for given node.
    pub fn build_address_list(&self, expire_at: Option<i32>) -> Result<AddressList> {
        let version = Version::get();
        let mut addrs: Vec<Address> = vec![self.config.ip_address.to_udp().into_boxed()];
        if let Some(quic_addr) = self.ip_address_quic() {
            addrs.push(
                Quic { ip: quic_addr.ip() as i32, port: quic_addr.port() as i32 }.into_boxed(),
            );
        }
        let ret = AddressList {
            addrs: addrs.into(),
            version,
            reinit_date: self.start_timestamp,
            priority: 0,
            expire_at: expire_at.unwrap_or(0),
        };
        Ok(ret)
    }

    /// Calculate timeout from roundtrip, milliseconds
    pub fn calc_timeout(roundtrip_ms: Option<u64>) -> u64 {
        let timeout_ms = roundtrip_ms.unwrap_or(Self::TIMEOUT_QUERY_MAX_MS);
        if timeout_ms < Self::TIMEOUT_QUERY_MIN_MS {
            Self::TIMEOUT_QUERY_MIN_MS
        } else {
            timeout_ms
        }
    }

    /// Check protocol options
    pub fn check_options(&self, options: u32) -> bool {
        (self.options.load(Ordering::Relaxed) & options) == options
    }

    /// Delete key
    pub fn delete_key(&self, key: &Arc<KeyId>, tag: usize) -> Result<bool> {
        let mut ret = false;
        if let Some(removed) = self.peers.remove(key) {
            let removed = removed.val();
            for peer in removed.map_of.iter() {
                ret = self.delete_peer_from_peers(removed, peer.key())? || ret;
            }
        }
        ret = self.config.delete_key(key, tag)? || ret;
        Ok(ret)
    }

    /// Delete peer
    pub fn delete_peer(&self, local_key: &Arc<KeyId>, peer_key: &Arc<KeyId>) -> Result<bool> {
        let peers = self.peers.get(local_key).ok_or_else(|| {
            error!("Try to remove peer {} from unknown local key {}", peer_key, local_key)
        })?;
        self.delete_peer_from_peers(peers.val(), peer_key)
    }

    /// Number of seconds elapsed since node start
    pub fn elapsed_sec(&self) -> u32 {
        self.start.elapsed().as_secs() as u32
    }

    /// Check whether peer is known
    pub fn have_peer(&self, local_key: &Arc<KeyId>, peer: &Arc<KeyId>) -> Result<bool> {
        let peers = self.peers.get(local_key).ok_or_else(|| {
            error!("Try to check peer {} of unknown local key {}", peer, local_key)
        })?;
        let have = peers.val().map_of.get(peer).is_some();
        Ok(have)
    }

    /// Get ADNL channel reset timeout
    pub fn get_channel_reset_timeout(&self) -> u64 {
        (self.options.load(Ordering::Relaxed) & Self::OPTION_MASK_TIMEOUT_CHANNEL_RESET_SEC) as u64
    }

    /// Node ADNL IP address
    pub fn ip_address_adnl(&self) -> &IpAddress {
        &self.config.ip_address
    }

    /// Node QUIC IP address (None = not configured).
    pub fn ip_address_quic(&self) -> Option<&IpAddress> {
        self.config.ip_address_quic.as_ref()
    }

    /// Node key by ID
    pub fn key_by_id(&self, id: &Arc<KeyId>) -> Result<Arc<dyn KeyOption>> {
        self.config.key_by_id(id)
    }

    /// Node key by tag
    pub fn key_by_tag(&self, tag: usize) -> Result<Arc<dyn KeyOption>> {
        self.config.key_by_tag(tag)
    }

    /// Parse other's address list
    pub fn parse_address_list(
        list: &AddressList,
    ) -> Result<Option<(IpAddress, Option<IpAddress>)>> {
        fn parse_addr(
            out: &mut Option<IpAddress>,
            kind: &str,
            ip: i32,
            port: i32,
            version: i32,
        ) -> bool {
            if out.is_some() {
                log::warn!(target: TARGET, "Duplicate {kind} address in address list");
                return false;
            }
            if ip == 0 {
                log::warn!(target: TARGET, "{kind} address with zero IP in address list");
                return false;
            }
            if (port <= 0) || (port > 65535) {
                log::warn!(
                    target: TARGET,
                    "{kind} address with invalid port {port} in address list"
                );
                return false;
            }
            *out = Some(IpAddress::from_versioned_parts(ip as u32, port as u16, Some(version)));
            true
        }

        if list.addrs.is_empty() {
            log::warn!(target: TARGET, "Address list is empty");
            return Ok(None);
        }
        let version = Version::get();
        if list.reinit_date > version + Self::CLOCK_TOLERANCE_SEC {
            log::warn!(
                target: TARGET,
                "Address list is too new: {} vs {}",
                list.reinit_date, version
            );
            return Ok(None);
        }
        // if (list.version > version) || (list.reinit_date > version) {
        //     fail!("Address list version is too high: {} vs {}", list.version, version)
        // }
        if (list.expire_at != 0) && (list.expire_at < version) {
            log::warn!(target: TARGET, "Address list is expired");
            return Ok(None);
        }
        let mut adnl_addr = None;
        let mut quic_addr = None;
        for addr in list.addrs.iter() {
            match addr {
                Address::Adnl_Address_Udp(x) => {
                    if !parse_addr(&mut adnl_addr, "ADNL", x.ip, x.port, list.version) {
                        return Ok(None);
                    }
                }
                Address::Adnl_Address_Quic(x) => {
                    if !parse_addr(&mut quic_addr, "QUIC", x.ip, x.port, list.version) {
                        return Ok(None);
                    }
                }
                _ => {}
            }
        }
        match adnl_addr {
            Some(ip) => Ok(Some((ip, quic_addr))),
            None => {
                log::warn!(target: TARGET, "No IPv4 ADNL address in address list");
                Ok(None)
            }
        }
    }

    /// Get peer's ADNL and QUIC socket addresses if known
    pub fn peer_ip_address(
        &self,
        local_key: &Arc<KeyId>,
        peer_key: &Arc<KeyId>,
    ) -> Result<Option<(SocketAddr, Option<SocketAddr>)>> {
        let peers = self.peers(local_key)?;
        let Some(peer) = peers.map_of.get(peer_key) else {
            return Ok(None);
        };
        let (_, adnl_address) = peer.val().address.ip_version_address_adnl.get();
        if adnl_address == 0 {
            return Ok(None);
        }
        let adnl_ip = (adnl_address >> 16) as u32;
        let adnl_port = adnl_address as u16;
        let adnl_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::from(adnl_ip)), adnl_port);
        let (_, quic_address) = peer.val().address.ip_version_address_quic.get();
        let quic_addr = if quic_address != 0 {
            let quic_ip = (quic_address >> 16) as u32;
            let quic_port = quic_address as u16;
            Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::from(quic_ip)), quic_port))
        } else {
            None
        };
        Ok(Some((adnl_addr, quic_addr)))
    }

    /// Send query
    pub async fn query(
        self: &Arc<AdnlNode>,
        query: &TaggedTlObject,
        peers: &AdnlPeers,
        timeout_ms: Option<u64>,
    ) -> Result<Option<TLObject>> {
        Ok(self
            .query_with_prefix_get_status(None, query, peers, timeout_ms, AdnlSendMethod::Fast)
            .await?
            .reply)
    }

    /// Send query and get connection status
    pub async fn query_get_status(
        self: &Arc<AdnlNode>,
        query: &TaggedTlObject,
        peers: &AdnlPeers,
        timeout_ms: Option<u64>,
        method: AdnlSendMethod,
    ) -> Result<AdnlReplyWithStatus> {
        self.query_with_prefix_get_status(None, query, peers, timeout_ms, method).await
    }

    /// Send query with prefix
    pub async fn query_with_prefix(
        self: &Arc<AdnlNode>,
        prefix: Option<&[u8]>,
        query: &TaggedTlObject,
        peers: &AdnlPeers,
        timeout_ms: Option<u64>,
    ) -> Result<Option<TLObject>> {
        Ok(self
            .query_with_prefix_get_status(prefix, query, peers, timeout_ms, AdnlSendMethod::Fast)
            .await?
            .reply)
    }

    /// Send query with prefix and get connection status
    pub async fn query_with_prefix_get_status(
        self: &Arc<AdnlNode>,
        prefix: Option<&[u8]>,
        query: &TaggedTlObject,
        peers: &AdnlPeers,
        timeout_ms: Option<u64>,
        method: AdnlSendMethod,
    ) -> Result<AdnlReplyWithStatus> {
        async fn wait_query(
            node: &Arc<AdnlNode>,
            context: &Arc<QuerySendContext>,
            peers: &AdnlPeers,
        ) -> Result<AdnlReplyWithStatus> {
            context.reply_ping.wait().await;
            node.process_query_result(context, peers)
        }

        async fn get_reply(
            reader: &mut tokio::sync::mpsc::UnboundedReceiver<(
                Result<AdnlReplyWithStatus>,
                Arc<QuerySendContext>,
            )>,
            peers: &AdnlPeers,
            query: &TaggedTlObject,
        ) -> Result<AdnlReplyWithStatus> {
            if let Some((reply_with_status, context)) = reader.recv().await {
                match reply_with_status {
                    Err(e) => {
                        log::warn!(
                            target: TARGET,
                            "Context setup phase, error with reply to {}: {e}",
                            context.get_query_print_id(peers.other())
                        );
                        let ret = AdnlReplyWithStatus {
                            reply: None,
                            status: AdnlStatus::with_params(context.method.clone(), &None),
                        };
                        Ok(ret)
                    }
                    Ok(reply_with_status) => {
                        if reply_with_status.reply.is_none() {
                            log::info!(
                                target: TARGET,
                                "Context setup phase, no reply to {}",
                                context.get_query_print_id(peers.other())
                            )
                        }
                        Ok(reply_with_status)
                    }
                }
            } else {
                reader.close();
                fail!("INTERNAL ERROR: reply to query {:?} read mismatch", query.object)
            }
        }

        // Send in default context first
        let mut method = match method {
            AdnlSendMethod::Fast => AdnlSendMethodDetailed::FastUrgent,
            AdnlSendMethod::Safe => AdnlSendMethodDetailed::Safe,
        };
        let context =
            self.send_query_with_method(prefix, query, peers, timeout_ms, method.clone())?;

        // Send in backup context if required
        match context.repeat {
            MessageRepeat::Required => {
                method = AdnlSendMethodDetailed::FastNormal;
                match self.send_query_with_method(prefix, query, peers, timeout_ms, method.clone())
                {
                    Err(e) => {
                        log::warn!(
                            target: TARGET,
                            "Error when send query with {method} method: {e}"
                        );
                        wait_query(self, &context, peers).await
                    }
                    Ok(context_backup) => {
                        // Wait for operations result
                        let (sender, mut reader) = tokio::sync::mpsc::unbounded_channel();
                        let sender = Arc::new(sender);
                        for context in [context.clone(), context_backup.clone()] {
                            let peers = peers.clone();
                            let sender = sender.clone();
                            let node = self.clone();
                            tokio::spawn(async move {
                                sender.send((wait_query(&node, &context, &peers).await, context))
                            });
                        }
                        let reply_with_status = get_reply(&mut reader, peers, query).await?;
                        // We occasionally can receive both replies, and ordinary the first,
                        // But return first to avoid timeout.
                        // There is a tolerance based on several priority attempts.
                        if reply_with_status.reply.is_some() {
                            tokio::spawn(async move {
                                if reader.recv().await.is_some() {
                                    Self::graceful_close(reader).await
                                } else {
                                    log::warn!(
                                        target: TARGET,
                                        "INTERNAL ERROR: query reply flush mismatch"
                                    )
                                }
                            });
                            Ok(reply_with_status)
                        } else {
                            get_reply(&mut reader, peers, query).await
                        }
                    }
                }
            }
            MessageRepeat::NotNeeded | MessageRepeat::Unapplicable => {
                wait_query(self, &context, peers).await
            }
        }
    }

    /// Reset peers
    pub fn reset_peers(&self, to_reset: &AdnlPeers) -> Result<()> {
        let local_key = to_reset.local();
        let other_key = to_reset.other();
        let peers = self.peers(local_key)?;
        let peer = peers.map_of.get(other_key).ok_or_else(|| {
            error!("Try to reset unknown peer pair {} -> {}", local_key, other_key)
        })?;
        log::warn!(target: TARGET, "Resetting peer pair {} -> {}", local_key, other_key);
        let peer = peer.val();
        let (_, address) = peer.address.ip_version_address_adnl.get();
        let address = AdnlNodeAddress::from_ip_addresses_and_key(
            &IpAddress { address, version: Version::get() },
            None,
            &peer.address.key,
        )?;
        peers
            .channels_wait
            .remove(other_key)
            .or_else(|| peers.channels_send.remove(other_key))
            .and_then(|removed| {
                let peer = Peer {
                    address,
                    recv_state: PeerState::for_receive_with_reinit_date(
                        peer.recv_state.reinit_date.load(Ordering::Relaxed) + 1,
                        #[cfg(feature = "telemetry")]
                        self,
                        #[cfg(feature = "telemetry")]
                        to_reset,
                        #[cfg(feature = "telemetry")]
                        Some(peer.recv_state.telemetry.clone()),
                    ),
                    send_state: PeerState::for_send(
                        #[cfg(feature = "telemetry")]
                        self,
                        #[cfg(feature = "telemetry")]
                        to_reset,
                        #[cfg(feature = "telemetry")]
                        Some(peer.send_state.telemetry.clone()),
                    ),
                    counter: self.allocated.peers.clone().into(),
                };
                #[cfg(feature = "telemetry")]
                self.telemetry.allocated.peers.update(self.allocated.peers.load(Ordering::Relaxed));
                peers.map_of.insert(other_key.clone(), peer);
                self.drop_receive_subchannels(removed.val());
                Some(())
            });
        self.push_peer_to_refresh(to_reset)
    }

    /// Send custom message
    pub async fn send_custom(&self, data: &TaggedByteSlice<'_>, peers: &AdnlPeers) -> Result<()> {
        self.send_custom_get_status(data, peers, AdnlSendMethod::Fast).await?;
        Ok(())
    }

    /// Send custom message and get connection status
    pub async fn send_custom_get_status(
        &self,
        data: &TaggedByteSlice<'_>,
        peers: &AdnlPeers,
        method: AdnlSendMethod,
    ) -> Result<AdnlStatus> {
        let method = match method {
            AdnlSendMethod::Fast => AdnlSendMethodDetailed::FastNormal,
            AdnlSendMethod::Safe => AdnlSendMethodDetailed::Safe,
        };
        let msg = TaggedAdnlMessage {
            object: AdnlCustomMessage { data: data.object.to_vec().into() }.into_boxed(),
            #[cfg(feature = "telemetry")]
            tag: data.tag,
        };
        let (channel, repeat) = if self.can_send_loopback_message(peers) {
            self.queue_send_loopback_packets
                .send(LoopbackData::Message((msg.object, peers.clone())))
                .map_err(|e| error!("Error when send loopback ADNL custom message: {e}"))?;
            (None, MessageRepeat::Unapplicable)
        } else {
            self.send_message_to_peer(msg, peers, method.clone())?
        };
        // Sending is no-wait operation, so
        // yield execution to prevent thread lock in upper protocols
        tokio::task::yield_now().await;
        match repeat {
            MessageRepeat::Unapplicable => Ok(AdnlStatus::with_params(method, &channel)),
            x => fail!("INTERNAL ERROR: bad repeat {:?} in ADNL custom message", x),
        }
    }

    /// Set ADNL channel reset timeout
    pub async fn set_channel_reset_timeout(&self, timeout_sec: u8) {
        let new = timeout_sec as u32;
        loop {
            let old = self.options.load(Ordering::Relaxed);
            let xor = old ^ ((old & !Self::OPTION_MASK_TIMEOUT_CHANNEL_RESET_SEC) | new);
            if self
                .options
                .compare_exchange(old, old ^ xor, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    }

    /// Set ADNL options
    pub fn set_options(&self, options: u32) {
        self.options.fetch_or(options, Ordering::Relaxed);
    }

    /// Access telemetry
    #[cfg(feature = "telemetry")]
    pub fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    pub(crate) fn after_send(&self, result: Result<usize>, expected_len: usize) {
        match result {
            Ok(len) => {
                if len != expected_len {
                    log::error!(target: TARGET, "Incomplete send: {len} bytes of {expected_len}");
                }
                self.send_bytes.update(len as u64);
                #[cfg(feature = "telemetry")]
                self.telemetry.send_packets.update(1);
            }
            Err(e) => log::error!(target: TARGET, "ERROR --> {e}"),
        }
    }

    pub(crate) fn config(&self) -> &AdnlNodeConfig {
        &self.config
    }

    pub(crate) fn stopper(&self) -> &Arc<Stopper> {
        &self.stop
    }

    async fn add_subchannels(&self, channel: Arc<AdnlChannel>, wait: bool) -> Result<()> {
        let peers = self.peers(&channel.local_key)?;
        let peer = peers.map_of.get(&channel.other_key).ok_or_else(|| {
            error!("Cannot add subchannels to unknown peer {}", channel.other_key)
        })?;
        let peer = peer.val();
        let added = if wait {
            let mut prev = None;
            let added = add_counted_object_to_map_with_update(
                &peers.channels_wait,
                channel.other_key.clone(),
                |found| {
                    prev = if let Some(found) = found {
                        if found.send.normal.id == channel.send.normal.id {
                            return Ok(None);
                        }
                        Some(found.clone())
                    } else {
                        None
                    };
                    Ok(Some(channel.clone()))
                },
            )?;
            if added {
                prev.or_else(|| {
                    peers
                        .channels_send
                        .remove(&channel.other_key)
                        .map(|removed| removed.val().clone())
                })
                .and_then(|removed| {
                    self.drop_receive_subchannels(&removed);
                    Some(())
                });
            }
            added
        } else {
            add_counted_object_to_map_with_update(
                &peers.channels_send,
                channel.other_key.clone(),
                |found| {
                    if let Some(found) = found {
                        if found.send.normal.id == channel.send.normal.id {
                            return Ok(None);
                        }
                    }
                    Ok(Some(channel.clone()))
                },
            )?
        };
        if !added {
            let ch = if wait {
                peers.channels_wait.get(&channel.other_key)
            } else {
                peers.channels_send.get(&channel.other_key)
            };
            if let Some(ch) = ch {
                let ch = ch.val();
                while ch.flags.load(Ordering::Relaxed) & AdnlChannel::SEQNO_RESET == 0 {
                    tokio::task::yield_now().await
                }
                return Ok(());
            } else {
                fail!("INTERNAL ERROR: mismatch in channel adding")
            }
        }
        self.channels_recv
            .insert(channel.recv.normal.id.clone(), Subchannel::Normal(channel.clone()));
        self.channels_recv
            .insert(channel.recv.stream.id.clone(), Subchannel::Stream(channel.clone()));
        self.channels_recv
            .insert(channel.recv.urgent.id.clone(), Subchannel::Urgent(channel.clone()));
        peer.send_state.stream_history.reset(0).await?;
        peer.recv_state.stream_history.reset(0).await?;
        peer.send_state.urgent_history.reset(0).await?;
        peer.recv_state.urgent_history.reset(0).await?;
        let flags = channel.flags.fetch_or(AdnlChannel::SEQNO_RESET, Ordering::Relaxed);
        if flags & AdnlChannel::SEQNO_RESET != 0 {
            fail!("INTERNAL ERROR: mismatch in channel seqno reset")
        }
        Ok(())
    }

    fn can_send_loopback_message(&self, peers: &AdnlPeers) -> bool {
        self.peers.get(peers.other()).is_some()
    }

    async fn check_packet(
        &self,
        packet: &AdnlPacketContents,
        method: &AdnlSendMethodDetailed,
        local_key: &Arc<KeyId>,
        channel: Option<&Arc<AdnlChannel>>,
    ) -> Result<Option<Arc<KeyId>>> {
        fn check_signature(
            packet: &AdnlPacketContents,
            key: &Arc<dyn KeyOption>,
            mandatory: bool,
        ) -> Result<()> {
            if let Some(signature) = &packet.signature {
                let mut to_sign = packet.clone();
                to_sign.signature = None;
                key.verify(&serialize_boxed(&to_sign.into_boxed())?, signature)
            } else if mandatory {
                fail!("No mandatory signature in ADNL packet")
            } else {
                Ok(())
            }
        }

        let dst_reinit_date = &packet.dst_reinit_date;
        let reinit_date = &packet.reinit_date;
        if dst_reinit_date.is_some() != reinit_date.is_some() {
            fail!("Destination and source reinit dates mismatch")
        }

        let (ret, disposition, address_reinit_date, check) = if let Some(channel) = &channel {
            if packet.from.is_some() || packet.from_short.is_some() {
                fail!("Explicit source address inside channel packet")
            }
            (channel.other_key.clone(), Cow::Borrowed("channel"), None, true)
        } else if let Some(pub_key) = &packet.from {
            let key: Arc<dyn KeyOption> = pub_key.try_into()?;
            let other_key = key.id().clone();
            if let Some(id) = &packet.from_short {
                if other_key.data() != id.id.as_slice() {
                    fail!("Mismatch between ID and key inside packet")
                }
            }
            check_signature(packet, &key, true)?;
            if let Some(address) = &packet.address {
                let address_reinit_date = if let Some(reinit_date) = reinit_date {
                    if &address.reinit_date > reinit_date {
                        fail!(
                            "Address and source reinit dates mismatch on {local_key}: {} vs {}",
                            address.reinit_date,
                            reinit_date
                        )
                    }
                    None
                } else {
                    Some(address.reinit_date)
                };
                if let Some((adnl_addr, quic_addr)) = Self::parse_address_list(address)? {
                    self.add_peer(local_key, &adnl_addr, quic_addr.as_ref(), &key)?;
                    (other_key, Cow::Borrowed("from-with-ip-address"), address_reinit_date, false)
                } else {
                    let disposition = Cow::Owned(format!("from-without-ip-address {address:?}"));
                    (other_key, disposition, address_reinit_date, false)
                }
            } else {
                (other_key, Cow::Borrowed("from-without-address"), None, false)
            }
        } else if let Some(id) = &packet.from_short {
            (KeyId::from_data(id.id.as_slice().clone()), Cow::Borrowed("from-short"), None, true)
        } else {
            fail!("No other key data inside packet: {:?}", packet)
        };

        let peers = self.peers(local_key)?;
        let peer = if let Some(channel) = channel {
            peers.map_of.get(&channel.other_key)
        } else {
            peers.map_of.get(&ret)
        };
        let Some(peer) = peer else {
            fail!("Unknown peer {ret}, {disposition}");
        };
        let peer = peer.val();
        if check {
            check_signature(packet, &peer.address.key, false)?
        }
        if let (Some(dst_reinit_date), Some(reinit_date)) = (dst_reinit_date, reinit_date) {
            let local_reinit_date = peer.recv_state.reinit_date();
            let cmp_dst = dst_reinit_date.cmp(&local_reinit_date);
            if let std::cmp::Ordering::Greater = cmp_dst {
                fail!(
                    "Destination reinit date is too new: {} vs {}, {:?}",
                    dst_reinit_date,
                    local_reinit_date,
                    packet
                )
            }
            if *reinit_date > Version::get() + Self::CLOCK_TOLERANCE_SEC {
                fail!("Source reinit date is too new: {}", reinit_date)
            }
            //let other_reinit_date = peer.send_state.reinit_date();
            if !peer.try_reinit(*reinit_date).await? {
                fail!("Source reinit date is too old: {}", reinit_date)
            }
            // let other_reinit_date = peer.send_state.reinit_date();
            // match reinit_date.cmp(&other_reinit_date) {
            //     Ordering::Equal => (),
            //     Ordering::Greater => {
            //         // Refresh reinit state
            //         peer.send_state.reset_reinit_date(*reinit_date);
            //         if other_reinit_date != 0 {
            //             peer.send_state.reset_seqno().await?;
            //             peer.recv_state.reset_seqno().await?;
            //         }
            //     },
            //     Ordering::Less => if *reinit_date != 0 {
            //         fail!("Source reinit date is too old: {}", reinit_date)
            //     }
            // }
            if *dst_reinit_date != 0 {
                if let std::cmp::Ordering::Less = cmp_dst {
                    // Push peer to refresh reinit date
                    self.push_peer_to_refresh(&AdnlPeers::with_keys(
                        local_key.clone(),
                        ret.clone(),
                    ))?;
                    fail!(
                        "Destination reinit date is too old: {} vs {} from {}, {:?}",
                        dst_reinit_date,
                        local_reinit_date,
                        ret,
                        packet
                    )
                }
            }
            // if dst_reinit_date != &0 {
            //     match dst_reinit_date.cmp(&peer.recv_state.reinit_date()) {
            //         Ordering::Equal => (),
            //         Ordering::Greater =>
            //             fail!("Destination reinit date is too new: {} vs {}, {:?}",
            //                 dst_reinit_date,
            //                 peer.recv_state.reinit_date(),
            //                 packet
            //             ),
            //         Ordering::Less => {
            //             // Push peer to refresh reinit date
            //             self.send_message_to_peer(
            //                 TaggedAdnlMessage {
            //                     object: AdnlMessage::Adnl_Message_Nop,
            //                     #[cfg(feature = "telemetry")]
            //                     tag: 0x80000000 // Service message, tag is not important
            //                 },
            //                 &AdnlPeers::with_keys(local_key.clone(), ret.clone()),
            //                 false,
            //             )?;
            //             fail!(
            //                 "Destination reinit date is too old: {} vs {}, {:?}",
            //                 dst_reinit_date,
            //                 peer.recv_state.reinit_date(),
            //                 packet
            //             )
            //         }
            //     }
            // }
            // let other_reinit_date = peer.send_state.reinit_date();
            // match reinit_date.cmp(&other_reinit_date) {
            //     Ordering::Equal => (),
            //     Ordering::Greater => if *reinit_date > Version::get() + Self::CLOCK_TOLERANCE_SEC {
            //         fail!("Source reinit date is too new: {}", reinit_date)
            //     } else {
            //         peer.send_state.reset_reinit_date(*reinit_date);
            //         if other_reinit_date != 0 {
            //             peer.send_state.reset_seqno().await?;
            //             peer.recv_state.reset_seqno().await?;
            //         }
            //     },
            //     Ordering::Less => fail!("Source reinit date is too old: {}", reinit_date)
            // }
        }
        if let Some(address_reinit_date) = address_reinit_date {
            if !peer.try_reinit(address_reinit_date).await? {
                log::warn!(
                    target: TARGET,
                    "Address list reinit date is too old: {}",
                    address_reinit_date
                )
            }
        }
        if let Some(seqno) = &packet.confirm_seqno {
            let local_seqno = peer.send_state.seqno(method);
            if *seqno as u64 > local_seqno {
                fail!(
                    "{local_key} <- {ret}: peer confirmed too new ADNL packet seqno \
                    {seqno}, expected <= {local_seqno}, method {method}"
                )
            }
        }
        if let Some(seqno) = &packet.seqno {
            match peer.recv_state.save_seqno(*seqno as u64, method).await {
                Err(e) => {
                    fail!("Peer {ret} ({:?}): {e}", channel.map(|ch| ch.other_key.clone()))
                }
                Ok(false) => return Ok(None),
                _ => (),
            }
        }
        log::trace!(
            target: TARGET,
            "recv packet {} -> {local_key} {:?}, seqno S{} R{}, method {method}",
            peer.address.key.id(),
            packet.seqno,
            peer.send_state.seqno(method),
            peer.recv_state.seqno(method)
        );
        Ok(Some(ret))
    }

    fn create_channel(
        &self,
        peers: &AdnlPeers,
        local_pub: &mut Option<[u8; 32]>,
        other_pub: &[u8; 32],
        context: &str,
    ) -> Result<Arc<AdnlChannel>> {
        let local_key = peers.local();
        let other_key = peers.other();
        let peer = self.peers(local_key)?;
        let peer = if let Some(peer) = peer.map_of.get(other_key) {
            peer
        } else {
            fail!("Channel {} with unknown peer {} -> {}", context, local_key, other_key)
        };
        let local_pvt_key = &peer.val().address.channel_key;
        let local_pub_key = local_pvt_key.pub_key()?;
        if let Some(ref local_pub) = local_pub {
            if local_pub_key != local_pub {
                fail!(
                    "Mismatch in key for channel {}\n{} / {}",
                    context,
                    base64_encode(local_pub_key),
                    base64_encode(other_pub)
                )
            }
        } else {
            local_pub.replace(local_pub_key.try_into()?);
        }
        let channel = AdnlChannel::with_keys(
            local_key,
            local_pvt_key,
            other_key,
            other_pub,
            self.allocated.channels.clone(),
        )?;
        #[cfg(feature = "telemetry")]
        self.telemetry.allocated.channels.update(self.allocated.channels.load(Ordering::Relaxed));
        log::debug!(target: TARGET, "Channel {}: {} -> {}", context, local_key, other_key);
        log::trace!(
            target: TARGET,
            "Channel send ID {}, recv ID {}",
            base64_encode(&channel.send.normal.id),
            base64_encode(&channel.recv.normal.id)
        );
        Ok(Arc::new(channel))
    }

    fn decrypt_packet_from_channel(
        &self,
        buf: &mut Vec<u8>,
        channel: &Arc<AdnlChannel>,
        method: &AdnlSendMethodDetailed,
    ) -> Result<Option<u16>> {
        let version = channel.decrypt_by_method(buf, method)?;
        // Ensure both sides of channel established
        if channel.flags.load(Ordering::Relaxed) & AdnlChannel::ESTABLISHED == 0 {
            let peers = self.peers(&channel.local_key)?;
            if let Some(removed) = peers.channels_wait.remove(&channel.other_key) {
                let result = peers.channels_send.reinsert(removed);
                if let lockfree::map::Insertion::Failed(_) = result {
                    fail!("Internal error when register send channel");
                }
            }
        }
        // Restore channel health
        let was = channel
            .flags
            .swap(AdnlChannel::ESTABLISHED | AdnlChannel::SEQNO_RESET, Ordering::Relaxed);
        if (was & AdnlChannel::MASK_TIMESTAMP) != 0 {
            log::debug!(
                target: TARGET,
                "Reset channel {} -> {} cancelled",
                channel.local_key, channel.other_key
            );
        }
        Ok(version)
    }

    fn delete_peer_from_peers(&self, peers: &Arc<Peers>, peer_key: &Arc<KeyId>) -> Result<bool> {
        let Some(_removed) = peers.map_of.remove(peer_key) else { return Ok(false) };
        #[cfg(feature = "telemetry")]
        {
            let removed = _removed.val();
            if let Some(item) = &removed.recv_state.telemetry.packets {
                self.telemetry.printer.delete_metric(&item.metric().name())
            }
            if let Some(item) = &removed.send_state.telemetry.packets {
                self.telemetry.printer.delete_metric(&item.metric().name())
            }
        }
        Ok(true)
    }

    fn drop_receive_subchannels(&self, channel: &Arc<AdnlChannel>) {
        self.channels_recv.remove(&channel.recv.normal.id);
        self.channels_recv.remove(&channel.recv.stream.id);
        self.channels_recv.remove(&channel.recv.urgent.id);
    }

    fn gen_rand() -> Vec<u8> {
        const RAND_SIZE: usize = 16;
        let mut ret = vec![0; RAND_SIZE];
        rand::thread_rng().fill(&mut ret[..]);
        ret
    }

    fn get_query_print_id(
        query_id: &QueryId,
        peer: &Arc<KeyId>,
        method: &AdnlSendMethodDetailed,
    ) -> String {
        format!(
            "{method} query {:02x}{:02x}{:02x}{:02x} @ {peer}",
            query_id[0], query_id[1], query_id[2], query_id[3],
        )
    }

    async fn graceful_close<T>(mut reader: tokio::sync::mpsc::UnboundedReceiver<T>) {
        reader.close();
        while reader.recv().await.is_some() {}
    }

    #[cfg(feature = "dump")]
    fn need_dump(pkt: &AdnlPacketContents) -> bool {
        if let Some(msg) = pkt.message.as_ref() {
            match msg {
                AdnlMessage::Adnl_Message_Custom(_) => false,
                AdnlMessage::Adnl_Message_Nop => false,
                _ => true,
            }
        } else if let Some(msgs) = pkt.messages.as_ref() {
            for msg in msgs.0.iter() {
                match msg {
                    AdnlMessage::Adnl_Message_Custom(_) => (),
                    AdnlMessage::Adnl_Message_Nop => (),
                    _ => return true,
                }
            }
            false
        } else {
            false
        }
    }

    fn peers(&self, src: &Arc<KeyId>) -> Result<Arc<Peers>> {
        if let Some(peers) = self.peers.get(src) {
            Ok(peers.val().clone())
        } else {
            fail!("Cannot get peers list for unknown local key {}", src)
        }
    }

    fn print_message(msg: &AdnlMessage) -> Cow<'_, str> {
        match msg {
            AdnlMessage::Adnl_Message_Answer(answer) => Cow::Owned(format!(
                "Adnl_Message_Answer {:x} {} bytes",
                answer.query_id,
                answer.answer.len()
            )),
            AdnlMessage::Adnl_Message_ConfirmChannel(_) => {
                Cow::Borrowed("Adnl_Message_ConfirmChannel")
            }
            AdnlMessage::Adnl_Message_CreateChannel(_) => {
                Cow::Borrowed("Adnl_Message_CreateChannel")
            }
            AdnlMessage::Adnl_Message_Custom(custom) => {
                Cow::Owned(format!("Adnl_Message_Custom {} bytes", custom.data.len()))
            }
            AdnlMessage::Adnl_Message_Nop => Cow::Borrowed("Adnl_Message_Nop"),
            AdnlMessage::Adnl_Message_Part(_) => Cow::Borrowed("Adnl_Message_Part"),
            AdnlMessage::Adnl_Message_Query(query) => Cow::Owned(format!(
                "Adnl_Message_Query {:x} {} bytes",
                query.query_id,
                query.query.len()
            )),
            AdnlMessage::Adnl_Message_Reinit(_) => Cow::Borrowed("Adnl_Message_Reinit"),
        }
    }

    async fn process_answer(&self, answer: &AdnlAnswerMessage, src: &Arc<KeyId>) -> Result<()> {
        let query_id = answer.query_id.as_slice().clone();
        if !Self::update_query(&self.queries, query_id, Some(&answer.answer)).await? {
            fail!("Received answer from {} to unknown query {:?}", src, answer)
        }
        Ok(())
    }

    async fn process_message(
        self: &Arc<Self>,
        subscribers: &[Arc<dyn Subscriber>],
        mut msg: AdnlMessage,
        peers: &AdnlPeers,
        method: &AdnlSendMethodDetailed,
    ) -> Result<()> {
        fn reply(
            node: &Arc<AdnlNode>,
            msg: Option<TaggedAdnlMessage>,
            peers: &AdnlPeers,
            method: &AdnlSendMethodDetailed,
        ) -> Result<()> {
            if let Some(msg) = msg {
                if node.can_send_loopback_message(peers) {
                    node.queue_send_loopback_packets
                        .send(LoopbackData::Message((msg.object, peers.clone())))
                        .map_err(|e| error!("Error when sending loopback AdnlMessage: {e}"))?;
                    Ok(())
                } else {
                    let urgent = *method == AdnlSendMethodDetailed::FastUrgent;
                    match node.send_message_to_peer(msg, peers, method.clone())? {
                        (_, MessageRepeat::NotNeeded) if urgent => Ok(()),
                        (_, MessageRepeat::Unapplicable) if !urgent => Ok(()),
                        (_, x) => {
                            fail!("INTERNAL ERROR: bad repeat {:?} in answer to ADNL message", x)
                        }
                    }
                }
            } else {
                Ok(())
            }
        }

        log::trace!(
            target: TARGET,
            "Process message {} -> {} {}, method {method}",
            peers.other(),
            peers.local(),
            Self::print_message(&msg)
        );

        if let AdnlMessage::Adnl_Message_Part(part) = &mut msg {
            let transfer_id = part.hash.as_slice();
            let added = add_counted_object_to_map(&self.transfers, transfer_id.clone(), || {
                let transfer = Transfer {
                    data: lockfree::map::Map::new(),
                    received: AtomicUsize::new(0),
                    total: part.total_size as usize,
                    updated: UpdatedAt::new(),
                    counter: self.allocated.transfers.clone().into(),
                };
                #[cfg(feature = "telemetry")]
                self.telemetry
                    .allocated
                    .transfers
                    .update(self.allocated.transfers.load(Ordering::Relaxed));
                Ok(Arc::new(transfer))
            })?;
            if let Some(transfer) = self.transfers.get(transfer_id) {
                if added {
                    let transfers_wait = self.transfers.clone();
                    let transfer_wait = transfer.val().clone();
                    let transfer_id_wait = transfer_id.clone();
                    tokio::spawn(async move {
                        loop {
                            tokio::time::sleep(Duration::from_millis(
                                Self::TIMEOUT_TRANSFER_SEC * 100,
                            ))
                            .await;
                            if transfer_wait.updated.is_expired(Self::TIMEOUT_TRANSFER_SEC) {
                                if transfers_wait.remove(&transfer_id_wait).is_some() {
                                    log::info!(
                                        target: TARGET,
                                        "ADNL transfer {} timed out",
                                        base64_encode(transfer_id_wait)
                                    );
                                }
                                break;
                            }
                        }
                    });
                }
                let transfer = transfer.val();
                transfer.updated.refresh();
                let data = std::mem::take(&mut part.data);
                let data_len = data.len();
                if transfer.data.insert(part.offset as usize, data).is_some() {
                    // Duplicated part
                    return Ok(());
                }
                transfer.received.fetch_add(data_len, Ordering::Relaxed);
                match Self::update_transfer(transfer_id, transfer) {
                    Ok(Some(new_msg)) => {
                        self.transfers.remove(transfer_id);
                        msg = new_msg;
                    }
                    Err(error) => {
                        self.transfers.remove(transfer_id);
                        return Err(error);
                    }
                    _ => return Ok(()),
                }
            } else {
                // Transfer was already completed and removed by another thread
                return Ok(());
            }
        }
        let msg = match &msg {
            AdnlMessage::Adnl_Message_Answer(answer) => {
                self.process_answer(answer, peers.other()).await?;
                None
            }
            AdnlMessage::Adnl_Message_ConfirmChannel(confirm) => {
                let mut local_pub = Some(confirm.peer_key.as_slice().clone());
                let channel = self.create_channel(
                    peers,
                    &mut local_pub,
                    confirm.key.as_slice(),
                    "confirmation",
                )?;
                // self.channels_send.insert(peers.other().clone(), channel.clone());
                // log::warn!(target: TARGET, "On recv confirm channel in {}", channel.local_key);
                // self.add_receive_subchannels(channel).await?;
                self.add_subchannels(channel, false).await?;
                // Speed up channel establishment
                Some(TaggedAdnlMessage {
                    object: AdnlMessage::Adnl_Message_Nop,
                    #[cfg(feature = "telemetry")]
                    tag: 0x80000000, // Service message, tag is not important
                })
            }
            AdnlMessage::Adnl_Message_CreateChannel(create) => {
                let mut local_pub = None;
                let channel =
                    self.create_channel(peers, &mut local_pub, create.key.as_slice(), "creation")?;
                let msg = if let Some(local_pub) = local_pub {
                    ConfirmChannel {
                        key: UInt256::with_array(local_pub),
                        peer_key: create.key.clone(),
                        date: create.date,
                    }
                    .into_boxed()
                } else {
                    fail!("INTERNAL ERROR: local key mismatch in channel creation")
                };
                // self.channels_wait
                //     .insert(peers.other().clone(), channel.clone())
                //     .or(self.channels_send.remove(peers.other()))
                //     .and_then(|removed| self.drop_receive_subchannels(removed.val()));
                // log::warn!(target: TARGET, "On recv create channel in {}", channel.local_key);
                // self.add_receive_subchannels(channel).await?;
                self.add_subchannels(channel, true).await?;
                Some(TaggedAdnlMessage {
                    object: msg,
                    #[cfg(feature = "telemetry")]
                    tag: 0x80000000, // Service message, tag is not important
                })
            }
            AdnlMessage::Adnl_Message_Custom(custom) => {
                if !Custom::process(subscribers, &custom.data, peers).await? {
                    fail!("No subscribers for custom message {:?} from {}", custom, peers.other())
                }
                None
            }
            AdnlMessage::Adnl_Message_Nop => None,
            AdnlMessage::Adnl_Message_Query(query) => {
                let answer = Self::process_query(subscribers, &query, peers, method).await?;
                match answer.try_finalize()? {
                    (Some(answer), _) => {
                        let peers = peers.clone();
                        let query = format!("{:?}", query);
                        let method = method.clone();
                        let node = self.clone();
                        tokio::spawn(async move {
                            match answer.try_wait().await {
                                Err(e) => log::error!(
                                    target: TARGET,
                                    "Error processing ADNL query {}: {}",
                                    query, e
                                ),
                                Ok(msg) => match reply(&node, msg.answer, &peers, &method) {
                                    Err(e) => log::error!(
                                        target: TARGET,
                                        "Error when replying to ADNL query {query}: {e}"
                                    ),
                                    _ => (),
                                },
                            }
                        });
                        return Ok(());
                    }
                    (None, msg) => msg,
                }
            }
            _ => fail!("Unsupported ADNL message {:?}", msg),
        };

        reply(self, msg, peers, method)
    }

    async fn process_packet(
        self: &Arc<Self>,
        packet: &mut PacketBuffer,
        subchannel: Subchannel,
        subscribers: &[Arc<dyn Subscriber>],
    ) -> Result<()> {
        #[cfg(feature = "telemetry")]
        let received_len = packet.buf.len();
        let (method, local_key, channel, version) = match subchannel.get_channel() {
            None => {
                if let (Some(local_key), version) =
                    AdnlHandshake::parse_packet(&self.config.keys, &mut packet.buf, None, true)?
                {
                    (AdnlSendMethodDetailed::FastNormal, local_key, None, version)
                } else {
                    log::trace!(
                        target: TARGET,
                        "Received message to unknown key ID {}",
                        base64_encode(&packet.buf[0..32])
                    );
                    #[cfg(feature = "telemetry")]
                    self.telemetry.normal.proc_unknown.update(1);
                    return Ok(());
                }
            }
            Some((channel, method)) => {
                let version =
                    self.decrypt_packet_from_channel(&mut packet.buf, channel, &method)?;
                (method, channel.local_key.clone(), Some(channel), version)
            }
        };
        if let Some(version) = version {
            if version != AdnlNode::VERSION_INITIAL {
                fail!("Unsupported ADNL version {}", version)
            }
        }
        let pkt = deserialize_typed::<AdnlPacketContentsBoxed>(&packet.buf)?.only();
        let other_key =
            if let Some(key) = self.check_packet(&pkt, &method, &local_key, channel).await? {
                key
            } else {
                #[cfg(feature = "telemetry")]
                if method == AdnlSendMethodDetailed::FastUrgent {
                    self.telemetry.urgent.proc_invalid.update(1);
                } else {
                    self.telemetry.normal.proc_invalid.update(1);
                }
                return Ok(());
            };
        #[cfg(feature = "dump")]
        if Self::need_dump(&pkt) {
            if let Some(dump) = self.dump.as_ref() {
                let msg = format!(
                    "{} Recv packet, method {method}\n{:?}\nDump\n{}",
                    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                    pkt,
                    dump!(&packet.buf[..])
                );
                dump.sender.send(DumpRecord { alive: true, key_id: other_key.clone(), msg })?;
            }
        }
        let peers = Arc::new(AdnlPeers::with_keys(local_key, other_key));
        #[cfg(feature = "telemetry")]
        if let Some(peer) = self.peers(peers.local())?.map_of.get(peers.other()) {
            peer.val().update_recv_stats(received_len as u64, peers.local());
        }
        if let Some(msg) = pkt.message {
            #[cfg(feature = "telemetry")]
            let chk = self.telemetry.add_check(Telemetry::get_message_info(&msg))?;
            #[allow(clippy::let_and_return)]
            let res = self.clone().process_message(subscribers, msg, &peers, &method).await;
            #[cfg(feature = "telemetry")]
            self.telemetry.drop_check(chk);
            res
        } else if let Some(msgs) = pkt.messages {
            let mut res = Ok(());
            for msg in msgs {
                #[cfg(feature = "telemetry")]
                let chk = self.telemetry.add_check(Telemetry::get_message_info(&msg))?;
                res = self.clone().process_message(subscribers, msg, &peers, &method).await;
                #[cfg(feature = "telemetry")]
                self.telemetry.drop_check(chk);
                if res.is_err() {
                    break;
                }
            }
            res
        } else {
            // Specifics of implementation.
            // Address/seqno update is to be sent serarately from data
            // fail!("ADNL packet ({}) without a message: {:?}", buf.len(), pkt)
            Ok(())
        }?;
        #[cfg(feature = "telemetry")]
        if method == AdnlSendMethodDetailed::FastUrgent {
            self.telemetry.urgent.proc_success.update(1)
        } else {
            self.telemetry.normal.proc_success.update(1)
        }
        Ok(())
    }

    async fn process_query(
        subscribers: &[Arc<dyn Subscriber>],
        query: &AdnlQueryMessage,
        peers: &AdnlPeers,
        method: &AdnlSendMethodDetailed,
    ) -> Result<QueryAdnlAnswer> {
        let query_id = query.query_id.as_slice();
        log::info!(
            target: TARGET_QUERY,
            "Recv {}",
            Self::get_query_print_id(&query_id, peers.other(), method)
        );
        if let Some(answer) = Query::process_adnl(subscribers, query, peers).await? {
            log::info!(
                target: TARGET_QUERY,
                "Reply to {}",
                Self::get_query_print_id(&query_id, peers.other(), method)
            );
            Ok(answer)
        } else {
            fail!("No subscribers for query {:?}", query)
        }
    }

    fn process_query_result(
        &self,
        context: &Arc<QuerySendContext>,
        peers: &AdnlPeers,
    ) -> Result<AdnlReplyWithStatus> {
        log::info!(
            target: TARGET_QUERY,
            "Finished {}",
            context.get_query_print_id(peers.other())
        );
        if let Some(removed) = self.queries.remove(&context.query_id) {
            let reply = match removed.val() {
                Query::Received(answer) => Some(deserialize_boxed(answer)?),
                Query::Timeout => {
                    if let MessageRepeat::Required = context.repeat {
                        None
                    } else {
                        /* Monitor channel health using queries */
                        if let Some(channel) = &context.channel {
                            self.try_reset_peers(channel, peers)?;
                        }
                        None
                    }
                }
                Query::Sent(_) => fail!(
                    "INTERNAL ERROR: ADNL {} Query::Sent state on receive detected",
                    context.get_query_print_id(peers.other())
                ),
            };
            let ret = AdnlReplyWithStatus {
                reply,
                status: AdnlStatus::with_params(context.method.clone(), &context.channel),
            };
            Ok(ret)
        } else {
            fail!(
                "INTERNAL ERROR: ADNL {} mismatch: unregistered query detected",
                context.get_query_print_id(peers.other())
            )
        }
    }

    fn push_peer_to_refresh(&self, peers: &AdnlPeers) -> Result<()> {
        self.send_message_to_peer(
            TaggedAdnlMessage {
                object: AdnlMessage::Adnl_Message_Nop,
                #[cfg(feature = "telemetry")]
                tag: 0x80000000, // Service message, tag is not important
            },
            peers,
            AdnlSendMethodDetailed::FastNormal,
        )?;
        Ok(())
    }

    fn send_message_to_peer(
        &self,
        msg: TaggedAdnlMessage,
        adnl_peers: &AdnlPeers,
        method: AdnlSendMethodDetailed,
    ) -> Result<(Option<Arc<AdnlChannel>>, MessageRepeat)> {
        const SIZE_ANSWER_MSG: usize = 44;
        const SIZE_CONFIRM_CHANNEL_MSG: usize = 72;
        const SIZE_CREATE_CHANNEL_MSG: usize = 40;
        const SIZE_CUSTOM_MSG: usize = 12;
        const SIZE_NOP_MSG: usize = 4;
        const SIZE_QUERY_MSG: usize = 44;

        fn build_part_message(
            data: &[u8],
            hash: &[u8; 32],
            offset: &mut usize,
            max_size: usize,
        ) -> AdnlMessage {
            let mut part = Vec::new();
            let next = min(data.len(), *offset + max_size);
            part.extend_from_slice(&data[*offset..next]);
            let ret = AdnlPartMessage {
                hash: UInt256::with_array(hash.clone()),
                total_size: data.len() as i32,
                offset: *offset as i32,
                data: part.into(),
            }
            .into_boxed();
            *offset = next;
            ret
        }

        fn calc_repeat(
            channel: Option<&Arc<AdnlChannel>>,
            peer: &Peer,
            mut method: AdnlSendMethodDetailed,
        ) -> (AdnlSendMethodDetailed, MessageRepeat) {
            if (method == AdnlSendMethodDetailed::Safe) && channel.is_none() {
                // Stream is available only with channel
                method = AdnlSendMethodDetailed::FastNormal;
            }
            let repeat = if method == AdnlSendMethodDetailed::FastUrgent {
                // Decide whether we need priority traffic
                if channel.is_none() {
                    // No need if no channel
                    method = AdnlSendMethodDetailed::FastNormal;
                } else {
                    // No need if no priority replies
                    if peer.recv_state.seqno(&method) == 0 {
                        if peer.send_state.seqno(&method) > AdnlNode::MAX_PRIORITY_ATTEMPTS {
                            method = AdnlSendMethodDetailed::FastNormal;
                        }
                    }
                }
                if (method == AdnlSendMethodDetailed::FastUrgent)
                    && (peer.recv_state.seqno(&method) == 0)
                {
                    MessageRepeat::Required
                } else {
                    MessageRepeat::NotNeeded
                }
            } else {
                MessageRepeat::Unapplicable
            };
            (method, repeat)
        }

        let src = adnl_peers.local();
        let dst = adnl_peers.other();

        log::trace!(
            target: TARGET,
            "Send message {src} -> {dst} {}, method {method}",
            Self::print_message(&msg.object)
        );

        let peers = self.peers(src)?;
        let mut peer = peers.map_of.get(dst).ok_or_else(|| error!("Unknown peer {}", dst))?;
        let mut channel = peers.channels_send.get(dst).map(|guard| guard.val().clone());

        if let Some(ch) = &channel {
            if let AdnlMessage::Adnl_Message_Custom(_) = &msg.object {
                if self.try_reset_peers(ch, adnl_peers)? {
                    peer = peers.map_of.get(dst).ok_or_else(|| error!("Unknown peer {}", dst))?;
                    channel = None;
                }
            }
        }

        let peer = peer.val();
        let src = self.key_by_id(src)?;
        let create_channel_msg = if channel.is_none() && peers.channels_wait.get(dst).is_none() {
            log::debug!(target: TARGET, "Create channel {} -> {}", src.id(), dst);
            let pub_key = peer.address.channel_key.pub_key()?;
            Some(
                CreateChannel {
                    key: UInt256::with_array(pub_key.try_into()?),
                    date: Version::get(),
                }
                .into_boxed(),
            )
        } else {
            None
        };
        let mut size = if create_channel_msg.is_some() { SIZE_CREATE_CHANNEL_MSG } else { 0 };
        size += match &msg.object {
            AdnlMessage::Adnl_Message_Answer(answer) => answer.answer.len() + SIZE_ANSWER_MSG,
            AdnlMessage::Adnl_Message_ConfirmChannel(_) => SIZE_CONFIRM_CHANNEL_MSG,
            AdnlMessage::Adnl_Message_Custom(custom) => custom.data.len() + SIZE_CUSTOM_MSG,
            AdnlMessage::Adnl_Message_Nop => SIZE_NOP_MSG,
            AdnlMessage::Adnl_Message_Query(query) => query.query.len() + SIZE_QUERY_MSG,
            _ => fail!("Unexpected message to send {:?}", msg.object),
        };

        let (mut method, mut repeat) = calc_repeat(channel.as_ref(), peer, method);
        if (size <= Self::MAX_ADNL_MESSAGE) || (AdnlSendMethodDetailed::Safe == method) {
            if let Some(create_channel_msg) = create_channel_msg {
                log::trace!(target: TARGET, "Send with message {:?}", create_channel_msg);
                self.send_packet(
                    peer,
                    &src,
                    channel.as_ref(),
                    None,
                    Some(vec![create_channel_msg, msg.object]),
                    method,
                    #[cfg(feature = "telemetry")]
                    msg.tag,
                )?
            } else {
                self.send_packet(
                    peer,
                    &src,
                    channel.as_ref(),
                    Some(msg.object),
                    None,
                    method,
                    #[cfg(feature = "telemetry")]
                    msg.tag,
                )?
            }
        } else {
            let data = serialize_boxed(&msg.object)?;
            let hash = sha256_digest(&data);
            let mut offset = 0;
            if let Some(create_channel_msg) = create_channel_msg {
                let part_msg = build_part_message(
                    &data[..],
                    &hash,
                    &mut offset,
                    Self::MAX_ADNL_MESSAGE - SIZE_CREATE_CHANNEL_MSG,
                );
                self.send_packet(
                    peer,
                    &src,
                    channel.as_ref(),
                    None,
                    Some(vec![create_channel_msg, part_msg]),
                    method.clone(),
                    #[cfg(feature = "telemetry")]
                    msg.tag,
                )?
            } else {
                repeat = MessageRepeat::Unapplicable
            };
            while offset < data.len() {
                let part_msg =
                    build_part_message(&data[..], &hash, &mut offset, Self::MAX_ADNL_MESSAGE);
                let (upd_method, upd_repeat) = calc_repeat(channel.as_ref(), peer, method);
                self.send_packet(
                    peer,
                    &src,
                    channel.as_ref(),
                    Some(part_msg),
                    None,
                    upd_method.clone(),
                    #[cfg(feature = "telemetry")]
                    msg.tag,
                )?;
                method = upd_method;
                if let MessageRepeat::Unapplicable = &repeat {
                    repeat = upd_repeat
                } else if repeat != upd_repeat {
                    fail!("INTERNAL ERROR: bad repeat in ADNL message part")
                }
            }
        };
        Ok((channel, repeat))
    }

    fn send_packet(
        &self,
        peer: &Peer,
        source: &Arc<dyn KeyOption>,
        channel: Option<&Arc<AdnlChannel>>,
        message: Option<AdnlMessage>,
        messages: Option<Vec<AdnlMessage>>,
        method: AdnlSendMethodDetailed,
        #[cfg(feature = "telemetry")] tag: u32,
    ) -> Result<()> {
        let mut pkt = AdnlPacketContents {
            rand1: Self::gen_rand().into(),
            from: if channel.is_some() { None } else { Some(source.try_into()?) },
            from_short: if channel.is_some() {
                None
            } else {
                Some(AdnlIdShort { id: UInt256::with_array(source.id().data().clone()) })
            },
            message,
            messages: messages.map(|messages| messages.into()),
            address: Some(
                self.build_address_list(Some(Version::get() + Self::TIMEOUT_ADDRESS_SEC))?,
            ),
            priority_address: None,
            seqno: Some(peer.send_state.next_seqno(&method) as i64),
            confirm_seqno: Some(peer.recv_state.seqno(&method) as i64),
            recv_addr_list_version: None,
            recv_priority_addr_list_version: None,
            reinit_date: if channel.is_some() { None } else { Some(peer.recv_state.reinit_date()) },
            dst_reinit_date: if channel.is_some() {
                None
            } else {
                Some(peer.send_state.reinit_date())
            },
            signature: None,
            rand2: Self::gen_rand().into(),
        };
        if channel.is_none() {
            let signature = source.sign(&serialize_boxed(&pkt.clone().into_boxed())?)?;
            pkt.signature = Some(signature);
        }
        #[cfg(feature = "dump")]
        let msg = if Self::need_dump(&pkt) && self.dump.is_some() {
            Some(format!("Send packet, priority {}\n{:?}", priority, pkt))
        } else {
            None
        };
        let mut data = serialize_boxed(&pkt.into_boxed())?;
        #[cfg(feature = "dump")]
        if let Some(msg) = msg {
            if let Some(dump) = self.dump.as_ref() {
                let msg = format!(
                    "{} {}\nDump\n{}",
                    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                    msg,
                    dump!(&data[..])
                );
                dump.sender.send(DumpRecord {
                    alive: channel.is_some(),
                    key_id: peer.address.key.id().clone(),
                    msg,
                })?;
            }
        }
        let version = if self.check_options(Self::OPTION_FORCE_VERSIONING) {
            Some(Self::VERSION_INITIAL)
        } else {
            None
        };
        if let Some(channel) = channel {
            channel.encrypt_by_method(&mut data, version, &method)?;
        } else {
            let key = Ed25519KeyOption::generate()?;
            AdnlHandshake::build_packet(&mut data, &key, &peer.address.key, version)?;
        }
        log::trace!(
            target: TARGET,
            "send packet {} -> {}, seqno S{} R{}, method {method}",
            source.id(),
            peer.address.key.id(),
            peer.send_state.seqno(&method),
            peer.recv_state.seqno(&method)
        );
        #[cfg(feature = "telemetry")]
        peer.update_send_stats(data.len() as u64, source.id());
        #[cfg(feature = "telemetry")]
        if method != AdnlSendMethodDetailed::FastUrgent {
            loop {
                if let Some(metric) = self.telemetry.normal.send_tags.get(&tag) {
                    metric.val().update(1);
                    break;
                }
                let name = format!("Send ordinary {:08x}", tag);
                let metric = Telemetry::create_metric_builder(name.as_str());
                let added =
                    add_unbound_object_to_map(&self.telemetry.normal.send_tags, tag, || {
                        Ok(metric.clone())
                    })?;
                if added {
                    self.telemetry.printer.add_metric(TelemetryItem::MetricBuilder(metric))
                }
            }
        }
        if self.peers.get(peer.address.key.id()).is_some() {
            // Seems to be loopback
            self.queue_send_loopback_packets
                .send(LoopbackData::Packet(data))
                .map_err(|e| error!("Error when sending loopback ADNL packet: {e}"))?
        } else {
            let (_, address) = peer.address.ip_version_address_adnl.get();
            let job = SendData { destination: address, data, method: method.clone() };
            if method == AdnlSendMethodDetailed::FastUrgent {
                self.send_pipeline.put_urgent(SendJob::Data(job))
            } else {
                self.send_pipeline.put_normal(SendJob::Data(job))
            }
        }
        Ok(())
    }

    fn send_query_with_method(
        self: &Arc<Self>,
        prefix: Option<&[u8]>,
        query: &TaggedTlObject,
        peers: &AdnlPeers,
        timeout_ms: Option<u64>,
        method: AdnlSendMethodDetailed,
    ) -> Result<Arc<QuerySendContext>> {
        let (query_id, msg) = Query::build(prefix, &query)?;
        let (ping, query) = Query::new();
        self.queries.insert(query_id, query);
        log::info!(
            target: TARGET_QUERY,
            "Send {}",
            Self::get_query_print_id(&query_id, peers.other(), &method)
        );
        let (channel, repeat) = if self.can_send_loopback_message(peers) {
            self.queue_send_loopback_packets
                .send(LoopbackData::Message((msg.object, peers.clone())))
                .map_err(|e| error!("Error when sending loopback ADNL query: {e}"))?;
            (None, MessageRepeat::Unapplicable)
        } else {
            self.send_message_to_peer(msg, peers, method.clone())?
        };
        self.queue_monitor_queries
            .push((timeout_ms.unwrap_or(Self::TIMEOUT_QUERY_MAX_MS), query_id));
        let ret = QuerySendContext { channel, method, query_id, repeat, reply_ping: ping };
        Ok(Arc::new(ret))
    }

    fn try_reset_peers(&self, channel: &Arc<AdnlChannel>, peers: &AdnlPeers) -> Result<bool> {
        let flags = channel.flags.load(Ordering::Relaxed);
        let flags = flags & (AdnlChannel::ESTABLISHED | AdnlChannel::SEQNO_RESET);
        let now = Version::get() as u64;
        let timeout = self.get_channel_reset_timeout();

        // Schedule reset if not yet
        let was = channel
            .flags
            .compare_exchange(flags, flags | (now + timeout), Ordering::Relaxed, Ordering::Relaxed)
            .unwrap_or_else(|was| was);

        // Reset if timeout is over
        let ts = was & AdnlChannel::MASK_TIMESTAMP;
        if (ts > 0) && (ts < now) {
            if channel
                .flags
                .compare_exchange(
                    flags | ts,
                    flags | ts | AdnlChannel::CHANNEL_RESET,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                log::warn!(
                    target: TARGET,
                    "Reset channel {} -> {} due to absent feedback",
                    peers.local(), peers.other()
                );
                self.reset_peers(peers)?;
                return Ok(true);
            }
        } else if ts == 0 {
            log::debug!(
                target: TARGET,
                "Reset channel {} -> {} scheduled after {timeout} seconds",
                peers.local(), peers.other()
            );
        }
        Ok(false)
    }

    async fn update_query(
        queries: &Arc<QueryCache>,
        query_id: QueryId,
        answer: Option<&[u8]>,
    ) -> Result<bool> {
        let insertion = queries.insert_with(query_id, |_, inserted, found| {
            if let Some(&(_, Query::Sent(_))) = found {
                lockfree::map::Preview::New(if let Some(answer) = answer {
                    Query::Received(answer.to_vec())
                } else {
                    Query::Timeout
                })
            } else if inserted.is_none() {
                lockfree::map::Preview::Discard
            } else {
                lockfree::map::Preview::Keep
            }
        });
        let removed = if let Some(removed) = insertion.updated() {
            removed
        } else {
            return Ok(false);
        };
        if let Query::Sent(pong) = removed.val() {
            pong.wait().await;
        } else {
            fail!(
                "INTERNAL ERROR: ADNL query state mismatch, \
                 expected Query::Sent, found {:?}",
                removed.val()
            )
        }
        Ok(true)
    }

    fn update_transfer(
        transfer_id: &TransferId,
        transfer: &Transfer,
    ) -> Result<Option<AdnlMessage>> {
        let mut received = transfer
            .received
            .compare_exchange(
                transfer.total,
                2 * transfer.total,
                Ordering::Relaxed,
                Ordering::Relaxed,
            )
            .unwrap_or_else(|was| was);
        if received > transfer.total {
            if received == 2 * transfer.total {
                // It seems we finished transfer in neighbour thread
                return Ok(None);
            }
            fail!(
                "Invalid ADNL part transfer: size mismatch {} vs. total {}",
                received,
                transfer.total
            )
        }
        if received == transfer.total {
            log::debug!("Finished ADNL part {} (total {})", received, transfer.total);
            received = 0;
            let mut buf = Vec::with_capacity(transfer.total);
            while received < transfer.total {
                if let Some(data) = transfer.data.get(&received) {
                    let data = data.val();
                    received += data.len();
                    buf.extend_from_slice(data)
                } else {
                    fail!("Invalid ADNL part transfer: parts mismatch")
                }
            }
            if !sha256_digest(&buf).eq(transfer_id) {
                fail!("Bad hash of ADNL transfer {}", base64_encode(transfer_id))
            }
            let msg = deserialize_boxed(&buf)?
                .downcast::<AdnlMessage>()
                .map_err(|msg| error!("Unsupported ADNL messge {:?}", msg))?;
            Ok(Some(msg))
        } else {
            log::debug!("Received ADNL part {} (total {})", received, transfer.total);
            Ok(None)
        }
    }
}
