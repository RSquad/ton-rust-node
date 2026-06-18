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
use core::ops::Range;
use rand::Rng;
#[cfg(any(feature = "client", feature = "node", feature = "server"))]
use std::convert::TryInto;
use std::{
    fmt::Debug,
    hash::Hash,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use ton_api::{
    deserialize_boxed_bundle, serialize_boxed, serialize_boxed_append,
    ton::{
        adnl::{
            message::message::{Answer as AdnlAnswerMessage, Query as AdnlQueryMessage},
            pong::Pong as AdnlPong,
            Message as AdnlMessage,
        },
        rldp::message::{Answer as RldpAnswer, Query as RldpQuery},
        rpc::adnl::Ping as AdnlPing,
    },
    AnyBoxedSerialize, BoxedSerialize, IntoBoxed, TLObject,
};
#[cfg(any(feature = "client", feature = "server", feature = "node"))]
use ton_block::KeyOption;
use ton_block::{
    fail, sha256_digest, sha256_digest_slices, AesCtr, KeyId, Result, UInt256, UnixTime,
};

#[cfg(any(feature = "node", feature = "server"))]
pub(crate) const TARGET: &str = "adnl";
#[cfg(any(feature = "node", feature = "server"))]
pub(crate) const TARGET_QUERY: &str = "adnl_query";

#[macro_export]
macro_rules! dump {
    (info, $target:expr, $msg:expr, $data:expr) => {
        if log::log_enabled!(log::Level::Info) {
            log::info!(target: $target, "{}:\n{}", $msg, ton_api::hex_dump!($data))
        }
    };
    (debug, $target:expr, $msg:expr, $data:expr) => {
        if log::log_enabled!(log::Level::Debug) {
            log::debug!(target: $target, "{}:\n{}", $msg, ton_api::hex_dump!($data))
        }
    };
    (trace, $target:expr, $msg:expr, $data:expr) => {
        if log::log_enabled!(log::Level::Trace) {
            log::trace!(target: $target, "{}:\n{}", $msg, ton_api::hex_dump!($data))
        }
    }
}

#[macro_export]
macro_rules! trace {
    ($target:expr, $func:expr) => {
        {
            if log::log_enabled!(log::Level::Debug) {
                let msg = stringify!($func);
                let pos = msg.find('\n').unwrap_or(80);
                log::debug!(target: $target, "before {}...", &msg[..pos]);
                let ret = $func;
                log::debug!(target: $target, "after {}...", &msg[..pos]);
                ret
            } else {
                $func
            }
        }
    };
}

/// ADNL crypto utils
pub struct AdnlCryptoUtils;

impl AdnlCryptoUtils {
    /// Build AES-based cipher with clearing key data
    pub fn build_cipher_secure(secret: &[u8; 32], digest: &[u8; 32]) -> AesCtr {
        let x = secret;
        let y = digest;
        // let mut key = from_slice!(x, 0, 16, y, 16, 16);
        #[rustfmt::skip]
        let mut key = [
            x[ 0], x[ 1], x[ 2], x[ 3], x[ 4], x[ 5], x[ 6], x[ 7],
            x[ 8], x[ 9], x[10], x[11], x[12], x[13], x[14], x[15],
            y[16], y[17], y[18], y[19], y[20], y[21], y[22], y[23],
            y[24], y[25], y[26], y[27], y[28], y[29], y[30], y[31]
        ];
        // let mut ctr = from_slice!(y, 0,  4, x, 20, 12);
        #[rustfmt::skip]
        let mut ctr = [
            y[ 0], y[ 1], y[ 2], y[ 3], x[20], x[21], x[22], x[23],
            x[24], x[25], x[26], x[27], x[28], x[29], x[30], x[31]
        ];
        let ret = AesCtr::with_params(&key, &ctr);
        key.iter_mut().for_each(|a| *a = 0);
        ctr.iter_mut().for_each(|a| *a = 0);
        ret
    }

    /// Build AES-based cipher without clearing key data
    pub fn build_cipher_unsecure(
        nonce: &[u8; 160],
        range_key: Range<usize>,
        range_ctr: Range<usize>,
    ) -> AesCtr {
        AesCtr::with_params(&nonce[range_key], &nonce[range_ctr])
    }

    /// Calculate checksum
    pub fn calc_checksum(version: &Option<u16>, data: &[u8]) -> [u8; 32] {
        if let Some(version) = version {
            sha256_digest_slices(&[&[(*version >> 8) as u8, *version as u8], data])
        } else {
            sha256_digest(data)
        }
    }

    /// Decode ADNL version
    pub fn decode_version(version: &[u8; 4], hdra: &[u8], hdrb: &[u8]) -> Option<u16> {
        // Mix encoded version with other bytes of header to decode version
        let mut xor = *version;
        for i in 0..hdra.len() {
            xor[i & 0x03] ^= hdra[i];
        }
        for i in 0..hdrb.len() {
            xor[i & 0x03] ^= hdrb[i];
        }
        if (xor[0] == xor[2]) && (xor[1] == xor[3]) {
            Some(((xor[0] as u16) << 8) | (xor[1] as u16))
        } else {
            None
        }
    }

    /// Encode ADNL header
    pub fn encode_header(
        buf: &mut Vec<u8>,
        id: &[u8; 32],
        key: Option<&[u8; 32]>,
        version: Option<u16>,
    ) -> [u8; 32] {
        let checksum = Self::calc_checksum(&version, buf);
        let len = buf.len();
        let hdr = if key.is_some() { 96 } else { 64 } + if version.is_some() { 4 } else { 0 };
        buf.resize(len + hdr, 0);
        buf[..].copy_within(..len, hdr);
        buf[..32].copy_from_slice(id);
        let idx = if let Some(key) = key {
            buf[32..64].copy_from_slice(key);
            64
        } else {
            32
        };
        let idx = if let Some(version) = version {
            // Mix version with other bytes of header to get encoded version
            let mut xor =
                [(version >> 8) as u8, version as u8, (version >> 8) as u8, version as u8];
            for i in 0..idx {
                xor[i & 0x03] ^= buf[i];
            }
            for i in 0..checksum.len() {
                xor[i & 0x03] ^= checksum[i];
            }
            buf[idx..idx + 4].copy_from_slice(&xor);
            idx + 4
        } else {
            idx
        };
        buf[idx..hdr].copy_from_slice(&checksum);
        checksum
    }
}

/// ADNL handshake
pub struct AdnlHandshake;

impl AdnlHandshake {
    /// Build handshake packet
    #[cfg(any(feature = "client", feature = "node"))]
    pub fn build_packet(
        buf: &mut Vec<u8>,
        local: &Arc<dyn KeyOption>,
        other: &Arc<dyn KeyOption>,
        version: Option<u16>,
    ) -> Result<()> {
        let checksum = AdnlCryptoUtils::encode_header(
            buf,
            other.id().data(),
            Some(local.pub_key()?.try_into()?),
            version,
        );
        let hdr = if version.is_some() { 100 } else { 96 };
        let shared_secret = local.shared_secret(other.pub_key()?)?;
        let shared_secret_data: &[u8] = &shared_secret.lock()?;
        Self::build_packet_cipher(shared_secret_data.try_into()?, &checksum)
            .apply_keystream(&mut buf[hdr..]);
        Ok(())
    }

    /// Parse handshake packet
    #[cfg(any(feature = "server", feature = "node"))]
    pub fn parse_packet(
        keys: &lockfree::map::Map<Arc<KeyId>, Arc<dyn KeyOption>>,
        buf: &mut Vec<u8>,
        len: Option<usize>,
        accept_versioning: bool,
    ) -> Result<(Option<Arc<KeyId>>, Option<u16>)> {
        fn process(
            buf: &mut Vec<u8>,
            secret: &[u8; 32],
            range: &Range<usize>,
            version: &Option<u16>,
        ) -> Result<()> {
            if range.start < 32 {
                fail!("INERNAL ERROR: bad range");
            }
            AdnlHandshake::build_packet_cipher(
                secret,
                buf[(range.start - 32)..range.start].try_into()?,
            )
            .apply_keystream(&mut buf[range.start..range.end]);
            if !AdnlCryptoUtils::calc_checksum(version, &buf[range.start..range.end])
                .eq(&buf[(range.start - 32)..range.start])
            {
                fail!("Bad handshake packet checksum, version {:?}", version);
            }
            buf.drain(0..range.start);
            Ok(())
        }

        if buf.len() < 96 + len.unwrap_or(0) {
            fail!("Bad handshake packet length: {}", buf.len());
        }
        for key in keys.iter() {
            if key.val().id().data().eq(&buf[0..32]) {
                let mut range = if let Some(len) = len { 96..96 + len } else { 96..buf.len() };
                if accept_versioning && (buf.len() >= 100) {
                    if let Some(version) = AdnlCryptoUtils::decode_version(
                        &buf[64..68].try_into()?,
                        &buf[..64],
                        &buf[68..100],
                    ) {
                        range.start += 4;
                        let mut tmp = Vec::with_capacity(buf.len() - range.end + range.start);
                        tmp.extend_from_slice(&buf[range.start..range.end]);
                        let version = Some(version);
                        let shared_secret = key.val().shared_secret(buf[32..64].try_into()?)?;
                        let guard = shared_secret.lock()?;
                        let shared_secret_data: &[u8] = &guard;
                        if process(buf, shared_secret_data.try_into()?, &range, &version).is_ok() {
                            return Ok((Some(key.key().clone()), version));
                        }
                        drop(guard);
                        buf[range.start..range.end].copy_from_slice(&tmp);
                    }
                }

                let shared_secret = key.val().shared_secret(buf[32..64].try_into()?)?;
                let guard = shared_secret.lock()?;
                let shared_secret_data: &[u8] = &guard;
                process(buf, shared_secret_data.try_into()?, &range, &None)?;
                drop(guard);
                return Ok((Some(key.key().clone()), None));
            }
        }
        Ok((None, None))
    }

    #[cfg(any(feature = "client", feature = "node", feature = "server"))]
    fn build_packet_cipher(shared_secret: &[u8; 32], checksum: &[u8; 32]) -> AesCtr {
        let ret = AdnlCryptoUtils::build_cipher_secure(shared_secret, checksum);
        ret
    }
}

/// ADNL peers
#[derive(Clone)]
pub struct AdnlPeers(Arc<KeyId>, Arc<KeyId>);

impl AdnlPeers {
    /// Constructor
    pub fn with_keys(local: Arc<KeyId>, other: Arc<KeyId>) -> Self {
        Self(local, other)
    }

    /// Local peer
    pub fn local(&self) -> &Arc<KeyId> {
        let AdnlPeers(local, _) = self;
        local
    }

    /// Other peer
    pub fn other(&self) -> &Arc<KeyId> {
        let AdnlPeers(_, other) = self;
        other
    }

    /// Change other peer
    pub fn set_other(&mut self, other: Arc<KeyId>) {
        let AdnlPeers(_, old_other) = self;
        *old_other = other
    }
}

/// ADNL ping subscriber
pub struct AdnlPingSubscriber;

#[async_trait::async_trait]
impl Subscriber for AdnlPingSubscriber {
    async fn try_consume_query(&self, object: TLObject, _peers: &AdnlPeers) -> Result<QueryResult> {
        match object.downcast::<AdnlPing>() {
            Ok(ping) => QueryResult::consume(
                AdnlPong { value: ping.value },
                #[cfg(feature = "telemetry")]
                None,
            ),
            Err(object) => Ok(QueryResult::Rejected(object)),
        }
    }
}

/// ADNL TCP stream
pub struct AdnlStream {
    stream: Pin<Box<tokio_io_timeout::TimeoutStream<tokio::net::TcpStream>>>,
}

impl AdnlStream {
    /// Constructor
    pub fn from_stream_with_timeouts(stream: tokio::net::TcpStream, timeouts: &Timeouts) -> Self {
        let mut stream = tokio_io_timeout::TimeoutStream::new(stream);
        stream.set_write_timeout(Some(timeouts.write()));
        stream.set_read_timeout(Some(timeouts.read()));
        Self { stream: Box::pin(stream) }
    }
    /// Read from stream (cancellation-safe with external offset tracking)
    pub async fn read(&mut self, buf: &mut Vec<u8>, len: usize, pos: &mut usize) -> Result<()> {
        buf.resize(len, 0);
        while *pos < len {
            let len = self.stream.as_mut().read(&mut buf[*pos..len]).await?;
            if len == 0 {
                fail!("ADNL stream closed by remote side");
            }
            *pos += len;
        }
        *pos = 0;
        Ok(())
    }
    /// Read exact number of bytes (not cancellation-safe, use only outside of tokio::select!)
    pub async fn read_exact(&mut self, buf: &mut Vec<u8>, len: usize) -> Result<()> {
        self.read(buf, len, &mut 0).await
    }
    /// Shutdown stream
    pub async fn shutdown(&mut self) -> Result<()> {
        self.stream.as_mut().shutdown().await?;
        Ok(())
    }
    /// Write to stream
    pub async fn write(&mut self, buf: &mut Vec<u8>) -> Result<()> {
        self.stream.as_mut().write_all(&buf[..]).await?;
        buf.truncate(0);
        Ok(())
    }
}

/// Receive state for cancellation-safe frame reading.
/// Tracks which phase of receive() we're in so that if tokio::select! cancels the future,
/// we resume from the correct point instead of re-reading the length prefix.
enum RecvState {
    /// Reading the 4-byte length prefix
    ReadingLength { pos: usize },
    /// Length prefix is read and decrypted; reading the body
    ReadingBody { len: usize, pos: usize },
}

/// ADNL stream cryptographic context
pub struct AdnlStreamCrypto {
    cipher_recv: AesCtr,
    cipher_send: AesCtr,
    recv_state: RecvState,
}

impl AdnlStreamCrypto {
    /// Construct as client
    #[cfg(feature = "client")]
    pub fn with_nonce_as_client(nonce: &[u8; 160]) -> Self {
        /* Do not clear nonce because it will be encrypted inplace afterwards */
        Self {
            cipher_recv: AdnlCryptoUtils::build_cipher_unsecure(nonce, 0..32, 64..80),
            cipher_send: AdnlCryptoUtils::build_cipher_unsecure(nonce, 32..64, 80..96),
            recv_state: RecvState::ReadingLength { pos: 0 },
        }
    }

    /// Construct as server
    #[cfg(feature = "server")]
    pub fn with_nonce_as_server(nonce: &mut [u8; 160]) -> Self {
        /* Clear nonce */
        let ret = Self {
            cipher_recv: AdnlCryptoUtils::build_cipher_unsecure(nonce, 32..64, 80..96),
            cipher_send: AdnlCryptoUtils::build_cipher_unsecure(nonce, 0..32, 64..80),
            recv_state: RecvState::ReadingLength { pos: 0 },
        };
        nonce.iter_mut().for_each(|a| *a = 0);
        ret
    }

    /// Send data in-place
    pub async fn send(&mut self, stream: &mut AdnlStream, buf: &mut Vec<u8>) -> Result<()> {
        let nonce: [u8; 32] = rand::thread_rng().gen();
        let len = buf.len();
        buf.reserve(len + 68);
        buf.resize(len + 36, 0);
        buf[..].copy_within(..len, 36);
        buf[..4].copy_from_slice(&((len + 64) as u32).to_le_bytes());
        buf[4..36].copy_from_slice(&nonce);
        buf.extend_from_slice(&sha256_digest(&buf[4..]));
        self.cipher_send.apply_keystream(&mut buf[..]);
        stream.write(buf).await?;
        Ok(())
    }

    /// Receive data (cancellation-safe: recv_state tracks progress across select! cancellations)
    pub async fn receive(&mut self, buf: &mut Vec<u8>, stream: &mut AdnlStream) -> Result<()> {
        // May be resumed from partial read
        // Phase 1: read length prefix
        let len = match &mut self.recv_state {
            RecvState::ReadingLength { pos } => {
                stream.read(buf, 4, pos).await?;
                self.cipher_recv.apply_keystream(&mut buf[..4]);
                let len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
                if len < 64 {
                    fail!("Too small size for ADNL packet: {len}");
                }
                // Transition to body phase before await point
                self.recv_state = RecvState::ReadingBody { len, pos: 0 };
                len
            }
            RecvState::ReadingBody { len, .. } => *len,
        };

        // Phase 2: read body
        let RecvState::ReadingBody { pos, .. } = &mut self.recv_state else {
            fail!("Unexpected receive state in ADNL stream crypto");
        };
        stream.read(buf, len, pos).await?;
        self.cipher_recv.apply_keystream(&mut buf[..len]);
        if !sha256_digest(&buf[..len - 32]).eq(&buf[len - 32..len]) {
            fail!("Bad checksum for ADNL packet, length {len}");
        }
        self.recv_state = RecvState::ReadingLength { pos: 0 };

        buf.truncate(len - 32);
        buf.drain(..32);
        Ok(())
    }
}

/// ADNL/RLDP answer
pub enum Answer {
    Object(TaggedTlObject),
    Raw(TaggedByteVec),
}

/// Cancellation-safety guard over AtomicU32 counter
struct SyncGuard<'a> {
    counter: &'a AtomicU32,
}

impl<'a> SyncGuard<'a> {
    fn new(counter: &'a AtomicU32) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self { counter }
    }
}

impl Drop for SyncGuard<'_> {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Asynchronous data receiver
pub(crate) struct AsyncReceiver<T> {
    id: u64,
    data: lockfree::queue::Queue<Option<T>>,
    subscribers: lockfree::queue::Queue<Arc<tokio::sync::Barrier>>,
    sync: AtomicU32,
    started_receiving: AtomicBool,
    alive_tasks: AtomicU32,
    total_spawned: AtomicU64,
}

impl<T> Drop for AsyncReceiver<T> {
    fn drop(&mut self) {
        log::info!(
            target: TARGET,
            "AsyncReceiver #{} dropped (alive_tasks={}, total_spawned={})",
            self.id,
            self.alive_tasks.load(Ordering::Relaxed),
            self.total_spawned.load(Ordering::Relaxed),
        );
    }
}

impl<T: Send + 'static> AsyncReceiver<T> {
    pub(crate) fn new() -> Arc<Self> {
        static INSTANCE_COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = INSTANCE_COUNTER.fetch_add(1, Ordering::Relaxed);
        log::info!(target: TARGET, "AsyncReceiver #{id} created");
        Arc::new(Self {
            id,
            data: lockfree::queue::Queue::new(),
            subscribers: lockfree::queue::Queue::new(),
            sync: AtomicU32::new(0),
            started_receiving: AtomicBool::new(false),
            alive_tasks: AtomicU32::new(0),
            total_spawned: AtomicU64::new(0),
        })
    }

    pub(crate) fn push(self: &Arc<Self>, data: T) {
        if self.started_receiving.load(Ordering::Relaxed) {
            self.do_push(Some(data))
        }
    }

    pub(crate) async fn pop(&self) -> Result<Option<T>> {
        self.started_receiving.store(true, Ordering::Relaxed);
        // Protect counter by guard
        let _guard = SyncGuard::new(&self.sync);
        loop {
            if let Some(data) = self.data.pop() {
                break Ok(data);
            }
            let subscriber = Arc::new(tokio::sync::Barrier::new(2));
            self.subscribers.push(subscriber.clone());
            subscriber.wait().await;
        }
    }

    pub(crate) fn stop(self: &Arc<Self>) {
        self.do_push(None)
    }

    fn do_push(self: &Arc<Self>, data: Option<T>) {
        self.data.push(data);
        if self.sync.load(Ordering::Relaxed) == 0 {
            return;
        }
        let alive = self.alive_tasks.fetch_add(1, Ordering::Relaxed) + 1;
        let total = self.total_spawned.fetch_add(1, Ordering::Relaxed) + 1;
        if total % 1000 == 0 || alive > 10 {
            log::warn!(
                target: TARGET,
                "AsyncReceiver #{}: alive_tasks={alive}, total_spawned={total}, sync={}",
                self.id, self.sync.load(Ordering::Relaxed)
            );
        }
        let receiver = self.clone();
        tokio::spawn(async move {
            while receiver.sync.load(Ordering::Relaxed) > 0 {
                if let Some(subscriber) = receiver.subscribers.pop() {
                    subscriber.wait().await;
                    break;
                } else {
                    tokio::task::yield_now().await;
                }
            }
            receiver.alive_tasks.fetch_sub(1, Ordering::Relaxed);
        });
    }
}

/// Consistent pair of linked atomics
pub struct AtomicPair {
    atomic1: AtomicU64,
    atomic2: AtomicU64,
}

impl AtomicPair {
    const LOCK_BIT: u64 = 0x8000000000000000;

    pub fn new(value1: u64, value2: u64) -> Self {
        Self { atomic1: AtomicU64::new(value1), atomic2: AtomicU64::new(value2) }
    }

    pub fn get(&self) -> (u64, u64) {
        loop {
            let value2 = self.atomic2.fetch_or(Self::LOCK_BIT, Ordering::Relaxed);
            if (value2 & Self::LOCK_BIT) != 0 {
                // Locked, concurrent use
                continue;
            }
            let value1 = self.atomic1.load(Ordering::Relaxed);
            self.atomic2
                .compare_exchange(
                    value2 | Self::LOCK_BIT,
                    value2,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                )
                .ok();
            break (value1, value2);
        }
    }

    pub fn update(&self, update1: u64, update2: u64, checker: impl Fn(u64, u64) -> bool) -> bool {
        loop {
            let old_value1 = self.atomic1.load(Ordering::Relaxed);
            if !checker(old_value1, update1) {
                break false;
            }
            let old_value2 = self.atomic2.fetch_or(Self::LOCK_BIT, Ordering::Relaxed);
            if (old_value2 & Self::LOCK_BIT) != 0 {
                // Locked, concurrent use
                continue;
            }
            self.atomic1.store(update1, Ordering::Relaxed);
            self.atomic2
                .compare_exchange(
                    old_value2 | Self::LOCK_BIT,
                    update2,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                )
                .ok();
            break true;
        }
        /*
                loop {
                    let old_value1 = self.atomic1.load(Ordering::Relaxed);
                    if !checker(old_value1, update1) {
                        break false
                    }
                    let old_value2 = self.atomic2.fetch_or(Self::LOCK_BIT, Ordering::Relaxed);
                    if (old_value2 & Self::LOCK_BIT) != 0 {
                        // Locked for write, concurrent change
                        continue
                    }
                    if self.atomic1.compare_exchange(
                        old_value1,
                        update1,
                        Ordering::Relaxed,
                        Ordering::Relaxed
                    ).is_err() {
                        // Concurrent change, restore data
                        self.atomic2.compare_exchange(
                            old_value2 | Self::LOCK_BIT,
                            old_value2,
                            Ordering::Relaxed,
                            Ordering::Relaxed
                        ).ok();
                        continue
                    }
                    if self.atomic2.compare_exchange(
                       old_value2 | Self::LOCK_BIT,
                       update2,
                       Ordering::Relaxed,
                       Ordering::Relaxed
                    ).is_err() {
                        // Concurrent change, restore data
                        self.atomic1.compare_exchange(
                            update1,
                            old_value1,
                            Ordering::Relaxed,
                            Ordering::Relaxed
                        ).ok();
                        continue
                    }
                    break true
                }
        */
    }
}

/// Counted object
pub trait CountedObject {
    fn counter(&self) -> &Counter;
}

impl<T: CountedObject> CountedObject for Arc<T> {
    fn counter(&self) -> &Counter {
        self.as_ref().counter()
    }
}

pub struct Counter(Arc<AtomicU64>);

impl From<Arc<AtomicU64>> for Counter {
    fn from(counter: Arc<AtomicU64>) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self(counter)
    }
}

impl Drop for Counter {
    fn drop(&mut self) {
        let Counter(counter) = self;
        counter.fetch_sub(1, Ordering::Relaxed);
    }
}

#[macro_export]
macro_rules! declare_counted {
    (
        $(#[$attr_struct: meta])?
        $vis: vis struct $struct: ident $(<$tt: tt>)? {
            $($(#[$attr_element: meta])? $element: ident : $ty: ty), *,
        }
    ) => {
        $(#[$attr_struct])?
        $vis struct $struct $(<$tt>)? {
            $($(#[$attr_element])? $element: $ty,)*
            counter: Counter
        }
        impl $(<$tt>)? CountedObject for $struct $(<$tt>)? {
            fn counter(&self) -> &Counter {
                &self.counter
           }
        }
    }
}

/// ADNL/RLDP custom data
pub(crate) struct Custom;

impl Custom {
    /// Process custom data
    pub(crate) async fn process(
        subscribers: &[Arc<dyn Subscriber>],
        custom: &[u8],
        peers: &AdnlPeers,
    ) -> Result<bool> {
        for subscriber in subscribers.iter() {
            if subscriber.try_consume_custom(custom, peers).await? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// ADNL/RLDP Query
#[derive(Debug)]
pub enum Query {
    Received(Vec<u8>),
    Sent(Arc<tokio::sync::Barrier>),
    Timeout,
}

impl Query {
    /// Construct new query
    pub(crate) fn new() -> (Arc<tokio::sync::Barrier>, Self) {
        let ping = Arc::new(tokio::sync::Barrier::new(2));
        let pong = ping.clone();
        (ping, Query::Sent(pong))
    }

    /// Build query
    pub(crate) fn build(
        prefix: Option<&[u8]>,
        query_body: &TaggedTlObject,
    ) -> Result<(QueryId, TaggedAdnlMessage)> {
        let query_id: QueryId = rand::thread_rng().gen();
        let query = if let Some(prefix) = prefix {
            let mut prefix = prefix.to_vec();
            serialize_boxed_append(&mut prefix, &query_body.object)?;
            prefix
        } else {
            serialize_boxed(&query_body.object)?
        };
        let msg = TaggedAdnlMessage {
            object: AdnlQueryMessage {
                query_id: UInt256::with_array(query_id),
                query: query.into(),
            }
            .into_boxed(),
            #[cfg(feature = "telemetry")]
            tag: query_body.tag,
        };
        Ok((query_id, msg))
    }

    /// Parse answer
    pub fn parse<Q, A>(answer: TLObject, query: &Q) -> Result<A>
    where
        A: AnyBoxedSerialize,
        Q: Debug,
    {
        match answer.downcast::<A>() {
            Ok(answer) => Ok(answer),
            Err(answer) => fail!("Unsupported response to {:?}: {:?}", query, answer),
        }
    }

    /// Process ADNL query
    pub(crate) async fn process_adnl(
        subscribers: &[Arc<dyn Subscriber>],
        query: &AdnlQueryMessage,
        peers: &AdnlPeers,
    ) -> Result<Option<QueryAdnlAnswer>> {
        let ret = Self::process(subscribers, &query.query[..], peers).await?.map(|answer| {
            QueryAdnlAnswer {
                answer,
                convert: Self::convert_to_adnl_answer,
                query_id: query.query_id.clone(),
            }
        });
        Ok(ret)
    }

    /// Process RLDP query
    pub(crate) async fn process_rldp(
        subscribers: &[Arc<dyn Subscriber>],
        query: &RldpQuery,
        peers: &AdnlPeers,
    ) -> Result<Option<QueryRldpAnswer>> {
        let ret = Self::process(subscribers, &query.data[..], peers).await?.map(|answer| {
            QueryRldpAnswer {
                answer,
                convert: Self::convert_to_rldp_answer,
                query_id: query.query_id.clone(),
            }
        });
        Ok(ret)
    }

    fn convert_to_adnl_answer(data: TaggedByteVec, query_id: UInt256) -> TaggedAdnlMessage {
        TaggedAdnlMessage {
            object: AdnlAnswerMessage { query_id, answer: data.object.into() }.into_boxed(),
            #[cfg(feature = "telemetry")]
            tag: data.tag,
        }
    }

    fn convert_to_rldp_answer(data: TaggedByteVec, query_id: UInt256) -> TaggedRldpAnswer {
        TaggedRldpAnswer {
            object: RldpAnswer { query_id, data: data.object.into() },
            #[cfg(feature = "telemetry")]
            tag: data.tag,
        }
    }

    pub(crate) async fn process(
        subscribers: &[Arc<dyn Subscriber>],
        query: &[u8],
        peers: &AdnlPeers,
    ) -> Result<Option<QueryAnswer>> {
        let mut queries = deserialize_boxed_bundle(query)?;
        if queries.len() == 1 {
            let mut query = queries.remove(0);
            for subscriber in subscribers.iter() {
                query = match subscriber.try_consume_query(query, peers).await? {
                    QueryResult::Consumed(answer) => return Ok(Some(answer)),
                    QueryResult::Rejected(query) => query,
                    QueryResult::RejectedBundle(_) => unreachable!(),
                };
            }
        } else {
            for subscriber in subscribers.iter() {
                queries = match subscriber.try_consume_query_bundle(queries, peers).await? {
                    QueryResult::Consumed(answer) => return Ok(Some(answer)),
                    QueryResult::Rejected(_) => unreachable!(),
                    QueryResult::RejectedBundle(queries) => queries,
                };
            }
        };
        Ok(None)
    }
}

/// Query answer with processing start time mark
pub struct TimedAnswer<A> {
    pub answer: Option<A>,
    #[cfg(feature = "telemetry")]
    pub actual_start_at: Option<Instant>,
}

/// ADNL/RLDP query answer in transit
pub enum QueryAnswer {
    Pending(tokio::task::JoinHandle<Result<TimedAnswer<Answer>>>),
    Ready(Option<Answer>),
}

/// ADNL/RLDP query answer finalizer
pub struct QueryAnswerFinalizer<A> {
    answer: QueryAnswer,
    convert: fn(TaggedByteVec, UInt256) -> A,
    query_id: UInt256,
}

impl<A> QueryAnswerFinalizer<A> {
    pub fn try_finalize(self) -> Result<(Option<Self>, Option<A>)> {
        let QueryAnswer::Ready(answer) = self.answer else { return Ok((Some(self), None)) };
        Ok((None, Self::convert(answer, self.convert, self.query_id)?))
    }

    pub async fn try_wait(self) -> Result<TimedAnswer<A>> {
        let QueryAnswer::Pending(handle) = self.answer else {
            fail!("Cannot wait on ready answer");
        };
        let answer = handle.await??;
        Ok(TimedAnswer {
            answer: Self::convert(answer.answer, self.convert, self.query_id)?,
            #[cfg(feature = "telemetry")]
            actual_start_at: answer.actual_start_at,
        })
    }

    fn convert(
        answer: Option<Answer>,
        convert: fn(TaggedByteVec, UInt256) -> A,
        query_id: UInt256,
    ) -> Result<Option<A>> {
        if let Some(answer) = answer {
            let answer = match answer {
                Answer::Object(x) => TaggedByteVec {
                    object: serialize_boxed(&x.object)?,
                    #[cfg(feature = "telemetry")]
                    tag: x.tag,
                },
                Answer::Raw(x) => x,
            };
            Ok(Some((convert)(answer, query_id)))
        } else {
            Ok(None)
        }
    }
}

/// Dedicated finalizers for ADNL and RLDP query answers
pub type QueryAdnlAnswer = QueryAnswerFinalizer<TaggedAdnlMessage>;
pub type QueryRldpAnswer = QueryAnswerFinalizer<TaggedRldpAnswer>;

/// ADNL query cache
pub type QueryCache = lockfree::map::Map<QueryId, Query>;

/// ADNL query ID
pub type QueryId = [u8; 32];

/// ADNL/RLDP query consumption result
pub enum QueryResult {
    /// Consumed with optional answer
    Consumed(QueryAnswer),
    /// Rejected
    Rejected(TLObject),
    /// Rejected bundle
    RejectedBundle(Vec<TLObject>),
}

impl QueryResult {
    /// Consume plain helper
    pub fn consume<A: IntoBoxed>(
        answer: A,
        #[cfg(feature = "telemetry")] tag: Option<u32>,
    ) -> Result<Self>
    where
        <A as IntoBoxed>::Boxed: AnyBoxedSerialize,
    {
        QueryResult::consume_boxed(
            answer.into_boxed(),
            #[cfg(feature = "telemetry")]
            tag,
        )
    }

    /// Consume boxed helper
    pub fn consume_boxed<A>(
        answer: A,
        #[cfg(feature = "telemetry")] tag: Option<u32>,
    ) -> Result<Self>
    where
        A: AnyBoxedSerialize,
    {
        let object = answer.into_tl_object();
        #[cfg(feature = "telemetry")]
        let tag = tag.unwrap_or_else(|| object.bare_object().constructor());
        let ret = TaggedTlObject {
            object,
            #[cfg(feature = "telemetry")]
            tag,
        };
        Ok(QueryResult::Consumed(QueryAnswer::Ready(Some(Answer::Object(ret)))))
    }
}

pub(crate) struct Stopper(AtomicU32);

impl Stopper {
    const MASK_STOP: u32 = 0x80000000;

    pub(crate) fn new() -> Self {
        Self(AtomicU32::new(0))
    }
    pub(crate) fn acquire(&self, mask: u32) {
        let Self(stop) = self;
        stop.fetch_or(mask, Ordering::Relaxed);
    }
    pub(crate) fn is_stopped(&self) -> bool {
        let Self(stop) = self;
        (stop.load(Ordering::Relaxed) & Self::MASK_STOP) != 0
    }
    pub(crate) fn release(&self, mask: u32) {
        let Self(stop) = self;
        stop.fetch_and(!mask, Ordering::Relaxed);
    }
    pub(crate) fn still_running(&self) -> u32 {
        let Self(stop) = self;
        stop.load(Ordering::Relaxed) & !Self::MASK_STOP
    }
    pub(crate) fn stop(&self) {
        let Self(stop) = self;
        stop.fetch_or(Self::MASK_STOP, Ordering::Relaxed);
    }
}

/// ADNL subscriber
#[async_trait::async_trait]
pub trait Subscriber: Send + Sync {
    /// Poll (for periodic actions)
    async fn poll(&self, _start: &Arc<Instant>) {}
    /// Try consume custom data: data -> consumed yes/no
    async fn try_consume_custom(&self, _data: &[u8], _peers: &AdnlPeers) -> Result<bool> {
        Ok(false)
    }
    /// Try consume TL object: object -> consumed yes/no
    async fn try_consume_object(&self, _object: TLObject, _peers: &AdnlPeers) -> Result<bool> {
        Ok(false)
    }
    /// Try consume query: object -> result
    async fn try_consume_query(&self, object: TLObject, _peers: &AdnlPeers) -> Result<QueryResult> {
        Ok(QueryResult::Rejected(object))
    }
    /// Try consume query bundle: objects -> result
    async fn try_consume_query_bundle(
        &self,
        objects: Vec<TLObject>,
        _peers: &AdnlPeers,
    ) -> Result<QueryResult> {
        Ok(QueryResult::RejectedBundle(objects))
    }
}

/// Tagged objects
pub struct TaggedObject<T> {
    pub object: T,
    #[cfg(feature = "telemetry")]
    pub tag: u32,
}

impl<T> TaggedObject<T> {
    pub fn with_object(object: T) -> Self {
        Self {
            object,
            #[cfg(feature = "telemetry")]
            tag: 0,
        }
    }
}

pub type TaggedAdnlMessage = TaggedObject<AdnlMessage>;
pub type TaggedByteSlice<'a> = TaggedObject<&'a [u8]>;
pub type TaggedByteVec = TaggedObject<Vec<u8>>;
pub type TaggedTlObject = TaggedObject<TLObject>;
pub type TaggedRldpAnswer = TaggedObject<RldpAnswer>;

impl From<TLObject> for TaggedTlObject {
    fn from(object: TLObject) -> Self {
        Self {
            #[cfg(feature = "telemetry")]
            tag: object.bare_object().constructor(),
            object,
        }
    }
}

/// Network timeouts
#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub struct Timeouts {
    read: Duration,
    write: Duration,
}

impl Timeouts {
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);
    pub fn with_duration(duration: Duration) -> Self {
        Self { read: duration.clone(), write: duration }
    }
    /// Read timeout
    pub fn read(&self) -> Duration {
        self.read
    }
    /// Write timeout
    pub fn write(&self) -> Duration {
        self.write
    }
}

impl Default for Timeouts {
    fn default() -> Self {
        Self { read: Self::DEFAULT_TIMEOUT, write: Self::DEFAULT_TIMEOUT }
    }
}

/// Data structure version
pub struct Version;

impl Version {
    pub fn get() -> i32 {
        UnixTime::now() as i32
    }
}

/// Data structure update timestamp
pub struct UpdatedAt {
    started: Instant,
    updated: AtomicU64,
}

#[allow(clippy::new_without_default)]
impl UpdatedAt {
    pub fn new() -> Self {
        Self { started: Instant::now(), updated: AtomicU64::new(0) }
    }
    pub fn refresh(&self) {
        self.updated.store(self.started.elapsed().as_secs(), Ordering::Relaxed)
    }
    pub fn is_expired(&self, timeout: u64) -> bool {
        self.started.elapsed().as_secs() - self.updated.load(Ordering::Relaxed) >= timeout
    }
}

pub struct Wait<T> {
    count: AtomicUsize,
    queue_sender: tokio::sync::mpsc::UnboundedSender<Option<T>>,
}

impl<T> Wait<T> {
    pub fn new() -> (Arc<Self>, tokio::sync::mpsc::UnboundedReceiver<Option<T>>) {
        let (queue_sender, queue_reader) = tokio::sync::mpsc::unbounded_channel();
        let ret = Self { count: AtomicUsize::new(0), queue_sender };
        (Arc::new(ret), queue_reader)
    }

    pub fn count(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }

    pub fn request(&self) -> usize {
        self.count.fetch_add(1, Ordering::Relaxed)
    }

    pub fn request_immediate(&self) -> usize {
        self.count.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn respond(&self, val: Option<T>) {
        match self.queue_sender.send(val) {
            Ok(()) => (),
            Err(tokio::sync::mpsc::error::SendError(_)) => (),
        }
    }

    pub async fn wait(
        &self,
        queue_reader: &mut tokio::sync::mpsc::UnboundedReceiver<Option<T>>,
        only_one: bool,
    ) -> Option<Option<T>> {
        let mut empty = self.count.load(Ordering::Relaxed) == 0;
        let mut ret = None;
        if !empty {
            ret = queue_reader.recv().await;
            match ret {
                Some(ref item) => {
                    self.count.fetch_sub(1, Ordering::Relaxed);
                    if item.is_some() && only_one {
                        empty = true
                    }
                }
                None => empty = true,
            }
        }
        if empty {
            // Graceful close
            queue_reader.close();
            while queue_reader.recv().await.is_some() {}
        }
        ret
    }
}

/// Add counted object to map
pub fn add_counted_object_to_map<K: Hash + Ord, V: CountedObject>(
    to: &lockfree::map::Map<K, V>,
    key: K,
    factory: impl FnMut() -> Result<V>,
) -> Result<bool> {
    add_unbound_object_to_map(to, key, factory)
}

/// Add or update counted object in map
pub fn add_counted_object_to_map_with_update<K: Hash + Ord, V: CountedObject>(
    to: &lockfree::map::Map<K, V>,
    key: K,
    factory: impl FnMut(Option<&V>) -> Result<Option<V>>,
) -> Result<bool> {
    add_unbound_object_to_map_with_update(to, key, factory)
}

/// Add unbound object to map
pub fn add_unbound_object_to_map<K: Hash + Ord, V>(
    to: &lockfree::map::Map<K, V>,
    key: K,
    mut factory: impl FnMut() -> Result<V>,
) -> Result<bool> {
    add_unbound_object_to_map_with_update(to, key, |found| {
        if found.is_some() {
            Ok(None)
        } else {
            Ok(Some(factory()?))
        }
    })
}

/// Add or update unbound object in map
pub fn add_unbound_object_to_map_with_update<K: Hash + Ord, V>(
    to: &lockfree::map::Map<K, V>,
    key: K,
    mut factory: impl FnMut(Option<&V>) -> Result<Option<V>>,
) -> Result<bool> {
    let mut error = None;
    let insertion = to.insert_with(key, |_, inserted, found| {
        let found = if let Some((_, found)) = found {
            Some(found)
        } else if inserted.is_some() {
            return lockfree::map::Preview::Keep;
        } else {
            None
        };
        match factory(found) {
            Err(err) => error = Some(err),
            Ok(Some(value)) => return lockfree::map::Preview::New(value),
            _ => (),
        }
        lockfree::map::Preview::Discard
    });
    match insertion {
        lockfree::map::Insertion::Created => Ok(true),
        lockfree::map::Insertion::Failed(_) => {
            if let Some(error) = error {
                Err(error)
            } else {
                Ok(false)
            }
        }
        lockfree::map::Insertion::Updated(_) => Ok(true),
    }
}

/// Calculate hash of TL object, non-boxed option
pub fn hash<T: IntoBoxed>(object: T) -> Result<[u8; 32]> {
    hash_boxed(&object.into_boxed())
}

/// Calculate hash of TL object, boxed option
pub fn hash_boxed<T: BoxedSerialize>(object: &T) -> Result<[u8; 32]> {
    let data = serialize_boxed(object)?;
    Ok(sha256_digest(data))
}

/// Spawn tokio task cancelable by token
pub fn spawn_cancelable<F>(
    cancellation_token: tokio_util::sync::CancellationToken,
    task: F,
) -> tokio::task::JoinHandle<F::Output>
where
    F: futures::Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        tokio::select! {
            _ = task => {},
            _ = cancellation_token.cancelled() => {}
        }
    })
}
