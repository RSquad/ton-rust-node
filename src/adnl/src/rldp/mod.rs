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
            add_counted_object_to_map, add_unbound_object_to_map, AdnlPeers, CountedObject,
            Counter, Custom, Query, QueryId, Subscriber, TaggedByteSlice, TimedAnswer, Version,
        },
        node::{AdnlNode, DataCompression},
    },
    declare_counted,
};
use rand::Rng;
#[cfg(feature = "debug")]
use std::sync::atomic::AtomicPtr;
#[cfg(any(feature = "debug", feature = "telemetry"))]
use std::time::Instant;
use std::{
    cmp::min,
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
#[cfg(feature = "telemetry")]
use ton_api::BoxedSerialize;
use ton_api::{
    deserialize_boxed, serialize_boxed,
    ton::{
        fec::Type as FecType,
        rldp::{
            message::{Message as RldpMessage, Query as RldpQuery},
            messagepart::{
                Complete as RldpComplete, Confirm as RldpConfirm, MessagePart as RldpMessagePart,
            },
            Message as RldpMessageBoxed, MessagePart as RldpMessagePartBoxed,
        },
        rldp2::{
            messagepart::{
                Complete as Rldp2Complete, Confirm as Rldp2Confirm, MessagePart as Rldp2MessagePart,
            },
            MessagePart as Rldp2MessagePartBoxed,
        },
    },
    AnyBoxedSerialize, IntoBoxed,
};
use ton_block::{base64_encode, error, fail, KeyId, Result, UInt256};

mod recv;
pub use recv::RaptorqDecoder;
use recv::{RecvContext, RecvTransfer};

mod send;
pub use send::RaptorqEncoder;
use send::{
    SendActionV2, SendContext, SendPartContextV2, SendPartStateV1, SendPartStateV2, SendPartV2,
    SendTransfer, SendTransferState,
};

mod stat;
use stat::{StatsConfigV2, StatsV2};

const TARGET: &str = "rldp";

pub enum Chunk {
    V1(RldpMessagePart),
    V2(Rldp2MessagePart),
}

impl Chunk {
    fn transfer_id(&self) -> &TransferId {
        match self {
            Chunk::V1(RldpMessagePart(chunk)) => chunk.transfer_id.as_array(),
            Chunk::V2(Rldp2MessagePart(chunk)) => chunk.transfer_id.as_array(),
        }
    }
}

pub struct Constraints {
    pub data_size: usize,
}

impl Constraints {
    const MAX_PARTS_IN_TRANSIT: usize = 20;
    const SLICE: usize = 2000000;
    const SYMBOL: usize = 768;

    pub fn check_data_size(&self, data_size: i32) -> Result<()> {
        if data_size == 0 {
            fail!("Empty RaptorQ data payload")
        }
        if data_size as usize > self.data_size {
            fail!("Too big RaptorQ data payload: {}", data_size)
        }
        Ok(())
    }

    pub fn check_fec_type(&self, fec_type: &FecType) -> Result<()> {
        match fec_type {
            FecType::Fec_RaptorQ(fec_type) => {
                if fec_type.symbol_size as usize != Self::SYMBOL {
                    fail!("Bad RaptorQ symbol size: {}", fec_type.symbol_size)
                }
                self.check_data_size(fec_type.data_size)?;
            }
            x => fail!("Bad FEC type {:?}", x),
        }
        Ok(())
    }

    pub fn check_seqno(seqno: u32) -> Result<()> {
        if (seqno & 0xff000000) != 0 {
            fail!("RaptorQ seqno is longer than 24 bits: {:x}", seqno)
        }
        Ok(())
    }
}

#[cfg(feature = "debug")]
pub type LossFn = fn(&Chunk) -> bool;

#[derive(Debug)]
enum MessagePart {
    V1(RldpMessagePartBoxed),
    V2(Rldp2MessagePartBoxed),
}

type TransferId = [u8; 32];

enum RldpTransfer {
    Recv(tokio::sync::mpsc::UnboundedSender<Chunk>),
    Send(SendTransferState),
    Done,
}

#[cfg(feature = "telemetry")]
#[derive(Default)]
struct RldpStats {
    transfers_sent_all: AtomicU64,
    transfers_recv_all: AtomicU64,
    transfers_sent_now: AtomicU64,
    transfers_recv_now: AtomicU64,
}

#[cfg(feature = "telemetry")]
impl RldpStats {
    fn inc(stat: &AtomicU64) -> u64 {
        stat.fetch_add(1, Ordering::Relaxed) + 1
    }
    fn dec(stat: &AtomicU64) -> u64 {
        stat.fetch_sub(1, Ordering::Relaxed) - 1
    }
}

declare_counted!(
    struct RldpPeer {
        outbounds: AtomicU32,
        queue: lockfree::queue::Queue<Arc<tokio::sync::Barrier>>,
        stats: StatsV2,
    }
);

struct RldpAlloc {
    peers: Arc<AtomicU64>,
    send_transfers: Arc<AtomicU64>,
    recv_transfers: Arc<AtomicU64>,
}

#[cfg(feature = "telemetry")]
struct RldpTelemetry {
    peers: Arc<Metric>,
    recv_transfers: Arc<Metric>,
    send_transfers: Arc<Metric>,
}

/// Rldp Node
pub struct RldpNode {
    adnl: Arc<AdnlNode>,
    local_id: Option<Arc<KeyId>>,
    min_timeout_ms: u64,
    peers: lockfree::map::Map<Arc<KeyId>, Arc<RldpPeer>>,
    #[cfg(feature = "telemetry")]
    stats: Arc<RldpStats>,
    subscribers: Arc<Vec<Arc<dyn Subscriber>>>,
    transfers: Arc<lockfree::map::Map<TransferId, RldpTransfer>>,
    #[cfg(feature = "debug")]
    timestamp: Arc<AtomicPtr<Instant>>,
    #[cfg(feature = "telemetry")]
    telemetry: RldpTelemetry,
    allocated: RldpAlloc,
    #[cfg(feature = "debug")]
    loss_fn: Option<LossFn>,
}

impl RldpNode {
    const ACK_DELAY_MS: u64 = 1; // RLDPv2 confirmation delay
    const MAX_OUTBOUNDS_PER_PEER: u32 = 3;
    const SIZE_TRANSFER_WAVE: u32 = 10;
    const SPINNER_MS: u64 = 1;
    const SPINNER_V1_SEND_MS: u64 = 10;
    const TIMEOUT_MAX_MS: u64 = 10000;
    const TIMEOUT_MIN_MS: u64 = 500;
    const TIMEOUT_WARN_MS: u64 = 5000;
    #[cfg(feature = "telemetry")]
    const TIMEOUT_TELEMETRY_SEC: u64 = 10;

    /// Constructor
    pub fn with_params(
        adnl: Arc<AdnlNode>,
        subscribers: Vec<Arc<dyn Subscriber>>,
        local_id: Option<Arc<KeyId>>,
        #[cfg(feature = "debug")] loss_fn: Option<LossFn>,
        #[cfg(feature = "debug")] min_timeout_ms: Option<u64>,
    ) -> Result<Arc<Self>> {
        #[cfg(feature = "telemetry")]
        let telemetry = RldpTelemetry {
            peers: adnl.add_metric("Alloc RLDP peers"),
            recv_transfers: adnl.add_metric("Alloc RLDP recv"),
            send_transfers: adnl.add_metric("Alloc RLDP send"),
        };
        let allocated = RldpAlloc {
            peers: Arc::new(AtomicU64::new(0)),
            recv_transfers: Arc::new(AtomicU64::new(0)),
            send_transfers: Arc::new(AtomicU64::new(0)),
        };
        #[cfg(not(feature = "debug"))]
        let min_timeout_ms = Self::TIMEOUT_MIN_MS;
        #[cfg(feature = "debug")]
        let min_timeout_ms = min_timeout_ms.unwrap_or(Self::TIMEOUT_MIN_MS);
        #[cfg(feature = "debug")]
        let timestamp = Box::new(Instant::now());
        let ret = Self {
            adnl,
            local_id,
            min_timeout_ms,
            peers: lockfree::map::Map::new(),
            #[cfg(feature = "telemetry")]
            stats: Arc::new(RldpStats::default()),
            subscribers: Arc::new(subscribers),
            transfers: Arc::new(lockfree::map::Map::new()),
            #[cfg(feature = "debug")]
            timestamp: Arc::new(AtomicPtr::new(Box::into_raw(timestamp))),
            #[cfg(feature = "telemetry")]
            telemetry,
            allocated,
            #[cfg(feature = "debug")]
            loss_fn,
        };
        Ok(Arc::new(ret))
    }

    /// Send message
    pub async fn message(
        &self,
        data: &TaggedByteSlice<'_>,
        peers: &AdnlPeers,
        v2: bool,
        roundtrip: Option<u64>,
    ) -> Result<u64> {
        let ret = self.outbound(data, None, peers, v2, roundtrip).await;
        #[cfg(feature = "telemetry")]
        let prefix = if v2 { "RLDP STAT recv v2" } else { "RLDP STAT recv v1" };
        #[cfg(feature = "telemetry")]
        if let Err(e) = &ret {
            log::info!(
                target: TARGET,
                "{prefix}: failed {:x} from {}: {e}",
                data.tag,
                peers.other()
            );
        }
        ret.map(|(_, timeout)| timeout)
    }

    /// Send query
    pub async fn query(
        &self,
        data: &TaggedByteSlice<'_>,
        max_answer_size: Option<u64>,
        peers: &AdnlPeers,
        v2: bool,
        roundtrip: Option<u64>,
    ) -> Result<(Option<Vec<u8>>, u64)> {
        let max_answer_size = max_answer_size.or(Some(128 * 1024));
        let ret = self.outbound(data, max_answer_size, peers, v2, roundtrip).await;
        #[cfg(feature = "telemetry")]
        let prefix = if v2 { "RLDP STAT query v2" } else { "RLDP STAT query v1" };
        #[cfg(feature = "telemetry")]
        match &ret {
            Err(e) => log::info!(
                target: TARGET,
                "{prefix}: failed {:x} from {}: {e}",
                data.tag,
                peers.other()
            ),
            Ok((Some(reply), _)) => log::info!(
                target: TARGET,
                "{prefix}: success {:x} from {}: {} bytes",
                data.tag,
                peers.other(),
                reply.len()
            ),
            Ok((None, _)) => log::info!(
                target: TARGET,
                "{prefix}: no data {:x} from {}",
                data.tag,
                peers.other()
            ),
        }
        ret
    }

    #[cfg(feature = "debug")]
    /// For time measurements
    pub fn check_time(&self, msg: &str) {
        Self::check_timestamp(&self.timestamp, msg)
    }

    #[cfg(feature = "debug")]
    /// For time measurements
    fn check_timestamp(timestamp: &Arc<AtomicPtr<Instant>>, msg: &str) {
        let timestamp = timestamp.load(Ordering::Relaxed);
        let elapsed = unsafe { (*timestamp).elapsed() };
        println!("{}; elapsed: {}s", msg, (elapsed.as_micros() as f32) / 1000000.0);
    }

    #[cfg(feature = "debug")]
    /// For time measurements
    pub fn reset_timestamp(&self) {
        let timestamp = Box::new(Instant::now());
        self.timestamp.store(Box::into_raw(timestamp), Ordering::Relaxed);
    }

    fn check_message(fec_type: Option<&FecType>, seqno: i32) -> Result<()> {
        const CONSTRAINTS: Constraints = Constraints { data_size: Constraints::SLICE };
        if let Some(fec_type) = fec_type {
            CONSTRAINTS.check_fec_type(fec_type)?
        }
        Constraints::check_seqno(seqno as u32)
    }

    fn check_too_long_transfer(
        start_ms: u64,
        last_warn_ms: &mut u64,
        timestamp_ms: u64,
        transfer_str: &String,
        msg: &str,
    ) {
        let elapsed_ms = timestamp_ms - start_ms;
        if elapsed_ms > Self::TIMEOUT_MAX_MS {
            if timestamp_ms - *last_warn_ms > Self::TIMEOUT_WARN_MS {
                *last_warn_ms = elapsed_ms;
                log::warn!(
                    target: TARGET,
                    "{msg} {transfer_str} took {elapsed_ms}ms"
                );
            }
        }
    }

    async fn consume_chunk_message(&self, chunk: Chunk, peers: &AdnlPeers) -> Result<()> {
        let transfer_id = chunk.transfer_id();
        let (v2, part, seqno) = match &chunk {
            Chunk::V1(RldpMessagePart(chunk)) => (false, chunk.part, chunk.seqno),
            Chunk::V2(Rldp2MessagePart(chunk)) => (true, chunk.part, chunk.seqno),
        };
        loop {
            let result = if let Some(transfer) = self.transfers.get(transfer_id) {
                if let RldpTransfer::Recv(queue_sender) = transfer.val() {
                    queue_sender.send(chunk)
                } else {
                    let reply = if v2 {
                        Rldp2Confirm {
                            transfer_id: transfer_id.into(),
                            part,
                            max_seqno: seqno,
                            received_count: 0,
                            received_mask: 0,
                        }
                        .into_boxed()
                        .into_tl_object()
                    } else {
                        RldpConfirm { transfer_id: transfer_id.into(), part, seqno }
                            .into_boxed()
                            .into_tl_object()
                    };
                    #[cfg(feature = "telemetry")]
                    let tag = reply.bare_object().constructor();
                    let data = serialize_boxed(&reply)?;
                    self.adnl
                        .send_custom(
                            &TaggedByteSlice {
                                object: &data,
                                #[cfg(feature = "telemetry")]
                                tag,
                            },
                            &peers,
                        )
                        .await?;
                    let reply = if v2 {
                        Rldp2Complete { transfer_id: transfer_id.into(), part }
                            .into_boxed()
                            .into_tl_object()
                    } else {
                        RldpComplete { transfer_id: transfer_id.into(), part }
                            .into_boxed()
                            .into_tl_object()
                    };
                    #[cfg(feature = "telemetry")]
                    let tag = reply.bare_object().constructor();
                    let data = serialize_boxed(&reply)?;
                    self.adnl
                        .send_custom(
                            &TaggedByteSlice {
                                object: &data,
                                #[cfg(feature = "telemetry")]
                                tag,
                            },
                            &peers,
                        )
                        .await?;
                    let rldp = if v2 { "RLDPv2" } else { "RLDPv1" };
                    log::debug!(
                        target: TARGET,
                        "Receive update on closed {rldp} transfer {}, part {}, seqno {}",
                        base64_encode(transfer_id), part, seqno
                    );
                    break;
                }
            } else if let Some(queue_sender) = self.inbound(transfer_id, peers, v2)? {
                queue_sender.send(chunk)
            } else {
                continue;
            };
            match result {
                Ok(()) => (),
                Err(tokio::sync::mpsc::error::SendError(_)) => (),
            }
            break;
        }
        Ok(())
    }

    async fn consume_message_part(&self, msg: MessagePart, peers: &AdnlPeers) -> Result<bool> {
        let (check, v2) = match &msg {
            MessagePart::V1(RldpMessagePartBoxed::Rldp_MessagePart(part)) => {
                let RldpMessagePart(part) = part;
                (Self::check_message(Some(&part.fec_type), part.seqno), false)
            }
            MessagePart::V2(Rldp2MessagePartBoxed::Rldp2_MessagePart(part)) => {
                let Rldp2MessagePart(part) = part;
                (Self::check_message(Some(&part.fec_type), part.seqno), true)
            }
            MessagePart::V1(RldpMessagePartBoxed::Rldp_Confirm(confirm)) => {
                (Self::check_message(None, confirm.seqno), false)
            }
            MessagePart::V2(Rldp2MessagePartBoxed::Rldp2_Confirm(confirm)) => {
                (Self::check_message(None, confirm.max_seqno), true)
            }
            MessagePart::V1(RldpMessagePartBoxed::Rldp_Complete(_)) => (Ok(()), false),
            MessagePart::V2(Rldp2MessagePartBoxed::Rldp2_Complete(_)) => (Ok(()), true),
        };
        if let Err(e) = check {
            // Ignore invalid messages as early as possible
            log::warn!(
                target: TARGET, "Received bad RLDP {} message. {}",
                if v2 {
                    "v2"
                } else {
                    "v1"
                },
                e
            );
            return Ok(true);
        }
        match msg {
            MessagePart::V1(RldpMessagePartBoxed::Rldp_Complete(msg)) => {
                if let Some(state) = self.get_v1_state(msg.transfer_id.as_ref(), msg.part as u32) {
                    state.set_next_part(msg.part as u32 + 1)
                }
            }
            MessagePart::V2(Rldp2MessagePartBoxed::Rldp2_Complete(msg)) => {
                if let Some(state) = self.get_v2_state(msg.transfer_id.as_ref(), msg.part as u32) {
                    state.set_finished(true)
                }
            }
            MessagePart::V1(RldpMessagePartBoxed::Rldp_Confirm(msg)) => {
                if let Some(state) = self.get_v1_state(msg.transfer_id.as_ref(), msg.part as u32) {
                    state.set_seqno_recv(msg.seqno as u32);
                }
            }
            MessagePart::V2(Rldp2MessagePartBoxed::Rldp2_Confirm(msg)) => {
                if let Some(state) = self.get_v2_state(msg.transfer_id.as_ref(), msg.part as u32) {
                    state.set_recv_info(
                        msg.max_seqno as u32,
                        msg.received_count as u32,
                        msg.received_mask as u32,
                    )
                }
            }
            MessagePart::V1(RldpMessagePartBoxed::Rldp_MessagePart(msg)) => {
                self.consume_chunk_message(Chunk::V1(msg), peers).await?
            }
            MessagePart::V2(Rldp2MessagePartBoxed::Rldp2_MessagePart(msg)) => {
                self.consume_chunk_message(Chunk::V2(msg), peers).await?
            }
        }
        Ok(true)
    }

    #[cfg(feature = "telemetry")]
    fn fetch_tag(data: &[u8]) -> u32 {
        if data.len() >= 4 {
            let mut tag = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
            // Uncover Overlay.Query internal message if possible
            if (tag == 0xCCFD8443) && (data.len() >= 40) {
                tag = u32::from_le_bytes([data[36], data[37], data[38], data[39]]);
            }
            tag
        } else {
            0
        }
    }

    fn fetch_transfer_state_v2(
        transfers: &Arc<lockfree::map::Map<TransferId, RldpTransfer>>,
        transfer_id: &TransferId,
    ) -> Result<Arc<Vec<Arc<SendPartStateV2>>>> {
        match transfers
            .get(transfer_id)
            .ok_or_else(|| error!("No state for transfer {}", base64_encode(transfer_id)))?
            .val()
        {
            RldpTransfer::Send(SendTransferState::V2(state)) => Ok(state.clone()),
            _ => fail!("Wrong state for transfer {}", base64_encode(transfer_id)),
        }
    }

    fn get_peer(&self, id: &Arc<KeyId>) -> Result<Arc<RldpPeer>> {
        let ret = loop {
            if let Some(peer) = self.peers.get(id) {
                break peer.val().clone();
            }
            add_counted_object_to_map(&self.peers, id.clone(), || {
                let ret = RldpPeer {
                    outbounds: AtomicU32::new(0),
                    queue: lockfree::queue::Queue::new(),
                    stats: StatsV2::new(StatsConfigV2::default(), self.min_timeout_ms)?,
                    counter: self.allocated.peers.clone().into(),
                };
                #[cfg(feature = "telemetry")]
                self.telemetry.peers.update(self.allocated.peers.load(Ordering::Relaxed));
                Ok(Arc::new(ret))
            })?;
        };
        ret.stats.v1.on_connect();
        Ok(ret)
    }

    fn get_v1_state(&self, transfer_id: &TransferId, part: u32) -> Option<Arc<SendPartStateV1>> {
        let mut ret = None;
        if let Some(transfer) = self.transfers.get(transfer_id) {
            match transfer.val() {
                RldpTransfer::Send(SendTransferState::V1(state)) => {
                    if state.part() == part {
                        ret.replace(state.clone());
                    }
                }
                RldpTransfer::Send(SendTransferState::V2(_)) => log::warn!(
                    target: TARGET,
                    "Received V1 message in V2 transfer {}",
                    base64_encode(transfer.key())
                ),
                _ => (),
            }
        }
        ret
    }

    fn get_v2_state(&self, transfer_id: &TransferId, part: u32) -> Option<Arc<SendPartStateV2>> {
        let mut ret = None;
        if let Some(transfer) = self.transfers.get(transfer_id) {
            match transfer.val() {
                RldpTransfer::Send(SendTransferState::V1(_)) => log::warn!(
                    target: TARGET,
                    "Received V2 message in V1 transfer {}",
                    base64_encode(transfer.key())
                ),
                RldpTransfer::Send(SendTransferState::V2(states)) => {
                    if (part as usize) < states.len() {
                        ret.replace(states[part as usize].clone());
                    }
                }
                _ => (),
            }
        }
        ret
    }

    fn inbound(
        &self,
        transfer_id: &TransferId,
        peers: &AdnlPeers,
        v2: bool,
    ) -> Result<Option<tokio::sync::mpsc::UnboundedSender<Chunk>>> {
        let rldp = if v2 { "RLDPv2" } else { "RLDPv1" };
        let (queue_sender, queue_reader) = tokio::sync::mpsc::unbounded_channel();
        let inserted = add_unbound_object_to_map(&self.transfers, *transfer_id, || {
            Ok(RldpTransfer::Recv(queue_sender.clone()))
        })?;
        if !inserted {
            return Ok(None);
        }
        #[cfg(feature = "telemetry")]
        let all = RldpStats::inc(&self.stats.transfers_recv_all);
        #[cfg(feature = "telemetry")]
        let now = RldpStats::inc(&self.stats.transfers_recv_now);
        #[cfg(feature = "telemetry")]
        log::trace!(target: TARGET, "{rldp} STAT recv: transfers total {all}, active {now}");
        let mut context = RecvContext {
            adnl: self.adnl.clone(),
            peers: peers.clone(),
            queue_reader,
            recv_transfer: RecvTransfer::new(
                *transfer_id,
                self.allocated.recv_transfers.clone(),
                v2,
                #[cfg(feature = "debug")]
                self.timestamp.clone(),
            ),
            transfer_id: *transfer_id,
            #[cfg(feature = "debug")]
            loss_fn: self.loss_fn,
        };
        #[cfg(feature = "telemetry")]
        self.telemetry.recv_transfers.update(self.allocated.recv_transfers.load(Ordering::Relaxed));
        let min_timeout_ms = self.min_timeout_ms;
        let peer = self.get_peer(peers.other())?;
        #[cfg(feature = "telemetry")]
        let stats = self.stats.clone();
        #[cfg(feature = "telemetry")]
        let send_metric = self.telemetry.send_transfers.clone();
        let send_counter = self.allocated.send_transfers.clone();
        let subscribers = self.subscribers.clone();
        let transfers = self.transfers.clone();
        tokio::spawn(async move {
            Self::receive_loop(&mut context, v2).await;
            transfers.insert(context.transfer_id, RldpTransfer::Done);
            let send_transfer_id = Self::inbound_loop(
                &mut context,
                subscribers,
                transfers.clone(),
                v2,
                &peer,
                min_timeout_ms,
                #[cfg(feature = "telemetry")]
                send_metric,
                send_counter,
            )
            .await
            .unwrap_or_else(|e| {
                log::warn!(
                    target: TARGET,
                    "ERROR: Incoming {rldp}: {e}, transfer {}",
                    base64_encode(&context.transfer_id)
                );
                None
            });
            #[cfg(feature = "telemetry")]
            let all = stats.transfers_recv_all.load(Ordering::Relaxed);
            #[cfg(feature = "telemetry")]
            let now = RldpStats::dec(&stats.transfers_recv_now);
            #[cfg(feature = "telemetry")]
            log::trace!(
                target: TARGET,
                "{rldp} STAT recv: transfers total {all}, active {now}"
            );
            tokio::time::sleep(Duration::from_millis(Self::TIMEOUT_MAX_MS * 2)).await;
            if let Some(send_transfer_id) = send_transfer_id {
                transfers.remove(&send_transfer_id);
            }
            transfers.remove(&context.transfer_id);
        });
        let transfers = self.transfers.clone();
        let transfer_id = *transfer_id;
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(Self::TIMEOUT_MAX_MS)).await;
            transfers.insert(transfer_id, RldpTransfer::Done);
        });
        Ok(Some(queue_sender))
    }

    async fn inbound_loop(
        context: &mut RecvContext,
        subscribers: Arc<Vec<Arc<dyn Subscriber>>>,
        transfers: Arc<lockfree::map::Map<TransferId, RldpTransfer>>,
        v2: bool,
        peer: &Arc<RldpPeer>,
        min_timeout_ms: u64,
        #[cfg(feature = "telemetry")] send_metric: Arc<Metric>,
        send_counter: Arc<AtomicU64>,
    ) -> Result<Option<TransferId>> {
        fn try_decompress(context: &mut RecvContext, data: &mut Vec<u8>) -> bool {
            if let Some(decompressed) = DataCompression::decompress_raw(data) {
                context.adnl.set_options(AdnlNode::OPTION_FORCE_COMPRESSION);
                *data = decompressed;
                true
            } else {
                context.adnl.check_options(AdnlNode::OPTION_FORCE_COMPRESSION)
            }
        }

        let rldp = if v2 { "RLDPv2" } else { "RLDPv1" };
        let mut query = match deserialize_boxed(&context.recv_transfer.data[..])?
            .downcast::<RldpMessageBoxed>()
        {
            Ok(RldpMessageBoxed::Rldp_Query(query)) => query,
            Ok(RldpMessageBoxed::Rldp_Message(mut message)) => {
                try_decompress(context, &mut message.data);
                #[cfg(feature = "debug")]
                Self::check_timestamp(&context.recv_transfer.timestamp, "Begin message process");
                if !Custom::process(&subscribers, &message.data, &context.peers).await? {
                    let len = min(16, message.data.len());
                    fail!("No subscribers for message {:?}...", hex::encode(&message.data[..len]));
                }
                #[cfg(feature = "debug")]
                Self::check_timestamp(&context.recv_transfer.timestamp, "End message process");
                return Ok(None);
            }
            Ok(message) => fail!("Unexpected {rldp} message: {message:?}"),
            Err(object) => fail!("Unexpected message in {rldp}: {object:?}"),
        };

        let now = Version::get();
        if now > query.timeout + Self::TIMEOUT_MAX_MS as i32 / 1000 {
            fail!(
                "{rldp} query was received expired on {} sec in transfer {} from {}",
                now - query.timeout,
                base64_encode(&context.transfer_id),
                context.peers.other()
            )
        }
        let compression = try_decompress(context, &mut query.data);
        #[cfg(feature = "debug")]
        Self::check_timestamp(&context.recv_transfer.timestamp, "Begin query process");
        #[cfg(feature = "telemetry")]
        let query_tag = Self::fetch_tag(&query.data[..]);
        let Some(answer) = Query::process_rldp(&subscribers, &query, &context.peers).await? else {
            fail!("No subscribers for query {:?}", query)
        };
        #[cfg(feature = "debug")]
        Self::check_timestamp(&context.recv_transfer.timestamp, "End query process");
        let answer = match answer.try_finalize()? {
            (Some(answer), _) => answer.try_wait().await?,
            (None, answer) => TimedAnswer {
                answer,
                #[cfg(feature = "telemetry")]
                actual_start_at: None,
            },
        };
        let Some(mut answer) = answer.answer else { return Ok(None) };
        if compression {
            answer.object.data = DataCompression::compress_raw(&answer.object.data)?;
        }
        let (len, max) = (answer.object.data.len(), query.max_answer_size as usize);
        if len > max {
            fail!("Exceeded max {rldp} answer size: {} vs {}", len, max)
        }
        #[cfg(feature = "telemetry")]
        let tag = answer.tag;
        let data = Arc::from(serialize_boxed(&answer.object.into_boxed())?);
        let mut send_transfer_id = context.transfer_id;
        for x in &mut send_transfer_id {
            *x ^= 0xFF
        }
        log::trace!(
            target: TARGET,
            "{rldp} answer ({len} bytes) to be sent in transfer {}/{} to {}",
            base64_encode(&context.transfer_id),
            base64_encode(&send_transfer_id),
            context.peers.other()
        );
        let (ping, pong) = tokio::sync::mpsc::unbounded_channel();
        let (send_transfer, send_transfer_state) =
            SendTransfer::new(data, send_transfer_id.clone(), send_counter.clone(), ping, v2)?;
        #[cfg(feature = "telemetry")]
        send_metric.update(send_counter.load(Ordering::Relaxed));
        transfers.insert(send_transfer_id, RldpTransfer::Send(send_transfer_state));
        let context_send = SendContext {
            adnl: context.adnl.clone(),
            peers: context.peers.clone(),
            pong,
            send_transfer,
            #[cfg(feature = "debug")]
            timestamp: context.recv_transfer.timestamp.clone(),
            transfer_id: context.transfer_id,
            #[cfg(feature = "telemetry")]
            tag,
        };
        let ok = if v2 {
            let transfer_state = Self::fetch_transfer_state_v2(&transfers, &send_transfer_id)?;
            Self::send_loop_v2(context_send, transfer_state, peer, min_timeout_ms).await
        } else {
            Self::send_loop_v1(context_send, peer, min_timeout_ms).await
        }?;
        if ok {
            log::trace!(
                target: TARGET,
                "{rldp} answer ({len} bytes) sent in transfer {} to {}",
                base64_encode(&context.transfer_id),
                context.peers.other()
            );
            #[cfg(feature = "telemetry")]
            log::info!(
                target: TARGET,
                "{rldp} STAT send: answer on {query_tag:x} ({len} bytes) sent in transfer {} to {}",
                base64_encode(&context.transfer_id),
                context.peers.other()
            );
        } else {
            log::warn!(
                target: TARGET,
                "Timeout >{} ms on answer in {rldp} transfer {} to {}",
                peer.stats.v1.timeout(),
                base64_encode(&context.transfer_id),
                context.peers.other()
            );
            #[cfg(feature = "telemetry")]
            log::info!(
                target: TARGET,
                "{rldp} STAT send: answer on {query_tag:x} timed out in transfer {} to {}",
                base64_encode(&context.transfer_id),
                context.peers.other()
            );
        }
        Ok(Some(send_transfer_id))
    }

    async fn outbound(
        &self,
        data: &TaggedByteSlice<'_>,
        max_answer_size: Option<u64>,
        peers: &AdnlPeers,
        v2: bool,
        roundtrip: Option<u64>,
    ) -> Result<(Option<Vec<u8>>, u64)> {
        #[cfg(feature = "telemetry")]
        let tag = data.tag;
        let data = if self.adnl.check_options(AdnlNode::OPTION_FORCE_COMPRESSION) {
            DataCompression::compress_raw(&data.object)?
        } else {
            data.object.to_vec()
        };
        let (query_id, message) = if let Some(max_answer_size) = max_answer_size {
            let query_id: QueryId = rand::thread_rng().gen();
            let message = RldpQuery {
                query_id: UInt256::with_array(query_id),
                max_answer_size: max_answer_size as i64,
                timeout: Version::get() + Self::TIMEOUT_MAX_MS as i32 / 1000,
                data,
            }
            .into_boxed();
            (Some(query_id), message)
        } else {
            let message = RldpMessage { id: UInt256::rand(), data }.into_boxed();
            (None::<QueryId>, message)
        };
        let data: Arc<[u8]> = Arc::from(serialize_boxed(&message)?);
        let peer = self.get_peer(peers.other())?;
        if let Some(roundtrip) = roundtrip {
            peer.stats.set_roundtrip(roundtrip)?
        }
        let outbounds = peer.outbounds.fetch_add(1, Ordering::Relaxed);
        #[cfg(feature = "telemetry")]
        log::trace!(
            target: TARGET,
            "RLDP STAT send: peer {} outbounds queued: {outbounds}",
            peers.other()
        );
        if outbounds >= Self::MAX_OUTBOUNDS_PER_PEER {
            let ping = Arc::new(tokio::sync::Barrier::new(2));
            peer.queue.push(ping.clone());
            ping.wait().await;
        }
        #[cfg(feature = "telemetry")]
        let all = RldpStats::inc(&self.stats.transfers_sent_all);
        #[cfg(feature = "telemetry")]
        let now = RldpStats::inc(&self.stats.transfers_sent_now);
        #[cfg(feature = "telemetry")]
        log::trace!(target: TARGET, "RLDP STAT send: transfers total {all}, active {now}");
        let (ping, pong) = tokio::sync::mpsc::unbounded_channel();
        let total_to_send = data.len();
        let send_transfer_id: TransferId = rand::thread_rng().gen();
        let (send_transfer, send_transfer_state) = SendTransfer::new(
            data,
            send_transfer_id.clone(),
            self.allocated.send_transfers.clone(),
            ping,
            v2,
        )?;
        #[cfg(feature = "telemetry")]
        self.telemetry.send_transfers.update(self.allocated.send_transfers.load(Ordering::Relaxed));
        self.transfers.insert(send_transfer_id.clone(), RldpTransfer::Send(send_transfer_state));
        let (recv_context, recv_transfer_id) = if query_id.is_some() {
            let mut recv_transfer_id = send_transfer_id;
            for x in &mut recv_transfer_id {
                *x ^= 0xFF
            }
            let (queue_sender, queue_reader) = tokio::sync::mpsc::unbounded_channel();
            let recv_transfer = RecvTransfer::new(
                recv_transfer_id,
                self.allocated.recv_transfers.clone(),
                v2,
                #[cfg(feature = "debug")]
                self.timestamp.clone(),
            );
            #[cfg(feature = "telemetry")]
            self.telemetry
                .recv_transfers
                .update(self.allocated.recv_transfers.load(Ordering::Relaxed));
            self.transfers.insert(recv_transfer_id, RldpTransfer::Recv(queue_sender));
            let recv_context = RecvContext {
                adnl: self.adnl.clone(),
                peers: peers.clone(),
                queue_reader,
                recv_transfer,
                transfer_id: send_transfer_id,
                #[cfg(feature = "debug")]
                loss_fn: self.loss_fn,
            };
            log::trace!(
                target: TARGET,
                "outbound query: transfer id {}/{}, total to send {total_to_send}",
                base64_encode(&send_transfer_id),
                base64_encode(&recv_transfer_id),
            );
            (Some(recv_context), Some(recv_transfer_id))
        } else {
            log::trace!(
                target: TARGET,
                "outbound message: transfer id {}, total to send {total_to_send}",
                base64_encode(&send_transfer_id)
            );
            (None, None)
        };
        let send_context = SendContext {
            adnl: self.adnl.clone(),
            peers: peers.clone(),
            pong,
            send_transfer,
            #[cfg(feature = "debug")]
            timestamp: self.timestamp.clone(),
            transfer_id: send_transfer_id,
            #[cfg(feature = "telemetry")]
            tag,
        };
        #[cfg(feature = "debug")]
        self.check_time("Outbound begin");
        let res =
            self.outbound_loop(send_context, recv_context, &send_transfer_id, v2, &peer).await;
        if res.is_err() {
            self.transfers.insert(send_transfer_id, RldpTransfer::Done);
        }
        #[cfg(feature = "debug")]
        self.check_time("Outbound end");
        if let Some(recv_transfer_id) = recv_transfer_id {
            self.transfers.insert(recv_transfer_id, RldpTransfer::Done);
        }
        let transfers = self.transfers.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(Self::TIMEOUT_MAX_MS * 2)).await;
            transfers.remove(&send_transfer_id);
            if let Some(recv_transfer_id) = recv_transfer_id {
                transfers.remove(&recv_transfer_id);
            }
        });
        #[cfg(feature = "telemetry")]
        let all = self.stats.transfers_sent_all.load(Ordering::Relaxed);
        #[cfg(feature = "telemetry")]
        let now = RldpStats::dec(&self.stats.transfers_sent_now);
        #[cfg(feature = "telemetry")]
        log::trace!(target: TARGET, "RLDP STAT send: transfers total {all}, actual {now}");
        let outbounds = peer.outbounds.fetch_sub(1, Ordering::Relaxed);
        #[cfg(feature = "telemetry")]
        log::trace!(
            target: TARGET,
            "RLDP STAT send: peer {} outbounds queued: {outbounds}",
            peers.other()
        );
        if outbounds > Self::MAX_OUTBOUNDS_PER_PEER {
            loop {
                if let Some(pong) = peer.queue.pop() {
                    pong.wait().await;
                    break;
                }
                tokio::task::yield_now().await;
            }
        }
        let answer = res?;
        if let Some(answer) = answer {
            let Some(query_id) = query_id else {
                fail!("Unexpected answer {answer:?} to RLDP message");
            };
            match deserialize_boxed(&answer[..])?.downcast::<RldpMessageBoxed>() {
                Ok(RldpMessageBoxed::Rldp_Answer(answer)) => {
                    if answer.query_id.as_slice() != &query_id {
                        fail!("Unknown query ID in RLDP answer")
                    } else {
                        let data = match DataCompression::decompress_raw(&answer.data) {
                            Some(data) => {
                                self.adnl.set_options(AdnlNode::OPTION_FORCE_COMPRESSION);
                                data
                            }
                            None => answer.data.to_vec(),
                        };
                        if data.len() >= 4 {
                            log::trace!(
                                target: TARGET,
                                "RLDP answer {:02x}{:02x}{:02x}{:02x}...",
                                data[0], data[1], data[2], data[3]
                            )
                        }
                        Ok((Some(data), peer.stats.v1.roundtrip()))
                    }
                }
                Ok(answer) => fail!("Unexpected answer to RLDP query: {:?}", answer),
                Err(answer) => fail!("Unexpected answer to RLDP query: {:?}", answer),
            }
        } else {
            Ok((None, peer.stats.v1.roundtrip()))
        }
    }

    async fn outbound_loop(
        &self,
        send_context: SendContext,
        recv_context: Option<RecvContext>,
        send_transfer_id: &TransferId,
        v2: bool,
        peer: &Arc<RldpPeer>,
    ) -> Result<Option<Vec<u8>>> {
        let (ping, mut pong) = tokio::sync::mpsc::unbounded_channel();
        let transfer_str =
            Self::transfer_string(&send_context.transfer_id, &send_context.peers, "to");
        let recv_state = if let Some(mut recv_context) = recv_context {
            let recv_state = recv_context.recv_transfer.state().clone();
            tokio::spawn(async move {
                Self::receive_loop(&mut recv_context, v2).await;
                ping.send(recv_context.recv_transfer)
            });
            Some(recv_state)
        } else {
            None
        };
        let ok = if v2 {
            let transfer_state = Self::fetch_transfer_state_v2(&self.transfers, &send_transfer_id)?;
            Self::send_loop_v2(send_context, transfer_state, peer, self.min_timeout_ms).await?
        } else {
            Self::send_loop_v1(send_context, peer, self.min_timeout_ms).await?
        };
        #[cfg(feature = "debug")]
        self.check_time(if recv_state.is_some() {
            "Outbound sent, wait reply"
        } else {
            "Outbound sent"
        });
        self.transfers.insert(send_transfer_id.clone(), RldpTransfer::Done);
        let outbound_str = if recv_state.is_some() { "outbound query" } else { "outbound message" };
        if ok {
            log::trace!(target: TARGET, "RLDP {outbound_str} sent in transfer {transfer_str}");
        } else {
            log::warn!(
                target: TARGET,
                "Timeout >{} ms on {outbound_str} in RLDP transfer {transfer_str}",
                peer.stats.v1.timeout()
            );
            return Ok(None);
        }
        let Some(recv_state) = recv_state else {
            return Ok(None);
        };
        let start_ms = peer.stats.v1.timestamp_ms();
        let mut last_warn_ms = start_ms;
        let mut last_diag_ms = start_ms;
        let mut updates = recv_state.updates();
        loop {
            match tokio::time::timeout(Duration::from_millis(Self::SPINNER_MS), pong.recv()).await {
                Ok(Some(reply)) => {
                    log::trace!(target: TARGET, "Got reply in {transfer_str}");
                    peer.stats.v1.update(self.min_timeout_ms);
                    return Ok(Some(reply.data));
                }
                Ok(None) => {
                    log::warn!(target: TARGET, "Error in RLDP transfer {transfer_str}");
                    break;
                }
                Err(_) => (),
            }
            let new_updates = recv_state.updates();
            if new_updates > updates {
                log::trace!(
                    target: TARGET,
                    "Recv updates {updates} -> {new_updates} in {transfer_str}"
                );
                let timestamp_ms = peer.stats.v1.timestamp_ms();
                let elapsed_ms = timestamp_ms - start_ms;
                let timeout = peer.stats.v1.timeout();
                if (timeout > 0)
                    && (elapsed_ms / timeout > 10)
                    && (timestamp_ms - last_diag_ms > Self::TIMEOUT_WARN_MS)
                {
                    log::warn!(
                        target: TARGET,
                        "RLDP query recv {transfer_str} masked timeout: \
                         elapsed {elapsed_ms} ms > 10*{timeout} ms, updates {new_updates}"
                    );
                    last_diag_ms = timestamp_ms;
                }
                peer.stats.v1.update(self.min_timeout_ms);
                updates = new_updates;
            } else if peer.stats.v1.try_timeout(start_ms) {
                log::warn!(
                    target: TARGET,
                    "Weak activity in RLDP {transfer_str} last {} ms, aborting",
                    peer.stats.v1.timeout()
                );
                break;
            }
            Self::check_too_long_transfer(
                start_ms,
                &mut last_warn_ms,
                peer.stats.v1.timestamp_ms(),
                &transfer_str,
                "RLDP query recv",
            );
        }
        Ok(None)
    }

    #[cfg(feature = "telemetry")]
    fn print_stats(&self) {}

    async fn receive_loop(context: &mut RecvContext, v2: bool) {
        let transfer_str = Self::transfer_string(&context.transfer_id, &context.peers, "from");
        let rldp = if v2 { "RLDPv2" } else { "RLDPv1" };
        let spin = Duration::from_millis(Self::ACK_DELAY_MS);
        let start = std::time::Instant::now();
        let mut last_diag = start;
        loop {
            if (start.elapsed() > Duration::from_millis(Self::TIMEOUT_MAX_MS))
                && (last_diag.elapsed() > Duration::from_millis(Self::TIMEOUT_WARN_MS))
            {
                let received = context.recv_transfer.data.len();
                let total = context.recv_transfer.total_size.unwrap_or(0);
                log::warn!(
                    target: TARGET,
                    "{rldp} receive_loop {transfer_str} running {} ms, {received}/{total} bytes",
                    start.elapsed().as_millis()
                );
                last_diag = std::time::Instant::now();
            }
            let job = match tokio::time::timeout(spin, context.queue_reader.recv()).await {
                Ok(Some(job)) => job,
                Ok(None) => break,
                Err(_) => {
                    if v2 {
                        if let Err(e) = context.send_confirmations().await {
                            log::warn!(target: TARGET, "{rldp} confirmation error: {e}")
                        }
                    }
                    continue;
                }
            };
            #[cfg(feature = "debug")]
            if context.loss_fn.map_or(false, |loss_fn| loss_fn(&job)) {
                continue;
            }
            match context.recv_transfer.process_chunk(job) {
                Err(e) => log::warn!(target: TARGET, "{rldp} error: {e}"),
                Ok(reply) => {
                    if let Some(reply) = reply {
                        if let Err(e) = context.adnl.send_custom(&reply, &context.peers).await {
                            log::warn!(target: TARGET, "{rldp} reply error: {e}")
                        }
                    }
                }
            }
            context.recv_transfer.state().set_updates();
            if let Some(total_size) = context.recv_transfer.total_size {
                if total_size == context.recv_transfer.data.len() {
                    log::trace!(
                        target: TARGET,
                        "{transfer_str}, receive completed ({total_size} bytes)"
                    );
                    #[cfg(feature = "debug")]
                    Self::check_timestamp(
                        &context.recv_transfer.timestamp,
                        format!(
                            "Recv transfer finished, {} data packets, {} confirm packets",
                            context.recv_transfer.total_data_packets,
                            context.recv_transfer.total_confirm_packets
                        )
                        .as_str(),
                    );
                    break;
                }
            } else {
                log::warn!("INTERNAL ERROR: {rldp} total size not set")
            }
        }
        // Graceful close
        context.queue_reader.close();
        while context.queue_reader.recv().await.is_some() {}
    }

    async fn send_loop_v1(
        mut context: SendContext,
        peer: &Arc<RldpPeer>,
        min_timeout_ms: u64,
    ) -> Result<bool> {
        let transfer_str = Self::transfer_string(&context.transfer_id, &context.peers, "to");
        let SendTransfer::V1(transfer) = &mut context.send_transfer else {
            fail!("Unexpected V2 send transfer in V1 send loop")
        };
        let start_ms = peer.stats.v1.timestamp_ms();
        let mut last_warn_ms = start_ms;
        let mut last_diag_ms = start_ms;
        #[cfg(feature = "debug")]
        let mut last_seqno = 0;
        #[cfg(any(feature = "debug", feature = "telemetry"))]
        let mut total_packets: u32 = 0;
        loop {
            let mut transfer_wave = transfer.start_next_part()?;
            if transfer_wave == 0 {
                #[cfg(feature = "debug")]
                Self::check_timestamp(
                    &context.timestamp,
                    format!("Send transfer finished, packets {total_packets}").as_str(),
                );
                break;
            }
            transfer_wave = min(transfer_wave, Self::SIZE_TRANSFER_WAVE);
            let part = transfer.state().part();
            let mut recv_seqno = 0;
            'part: loop {
                for _ in 0..transfer_wave {
                    #[cfg(any(feature = "debug", feature = "telemetry"))]
                    {
                        total_packets += 1;
                    }
                    #[cfg(feature = "debug")]
                    {
                        last_seqno = transfer.state().seqno_send()
                    }
                    let (object, do_next) = transfer.prepare_chunk()?;
                    let chunk = TaggedByteSlice {
                        object,
                        #[cfg(feature = "telemetry")]
                        tag: context.tag,
                    };
                    context.adnl.send_custom(&chunk, &context.peers).await?;
                    if !do_next {
                        #[cfg(feature = "debug")]
                        Self::check_timestamp(
                            &context.timestamp,
                            format!("Part {} transfer suspended, seqno {}", part, last_seqno)
                                .as_str(),
                        );
                        break 'part;
                    }
                    if transfer.state().is_transfer_finished_or_next_part(part)? {
                        #[cfg(feature = "debug")]
                        Self::check_timestamp(
                            &context.timestamp,
                            format!(
                                "Part {} transfer finished in progress, seqno {}",
                                part, last_seqno
                            )
                            .as_str(),
                        );
                        break 'part;
                    }
                }
                tokio::time::timeout(
                    Duration::from_millis(Self::SPINNER_V1_SEND_MS),
                    context.pong.recv(),
                )
                .await
                .ok();
                if transfer.state().is_transfer_finished_or_next_part(part)? {
                    #[cfg(feature = "debug")]
                    Self::check_timestamp(
                        &context.timestamp,
                        format!("Part {} transfer finished after wait, seqno {}", part, last_seqno)
                            .as_str(),
                    );
                    break;
                }
                let new_recv_seqno = transfer.state().seqno_recv();
                if new_recv_seqno > recv_seqno {
                    log::trace!(
                        target: TARGET,
                        "Send part {} updates {} -> {} in {}",
                        part, recv_seqno, new_recv_seqno, transfer_str
                    );
                    Self::check_too_long_transfer(
                        start_ms,
                        &mut last_warn_ms,
                        peer.stats.v1.timestamp_ms(),
                        &transfer_str,
                        "RLDPv1 send",
                    );
                    let timestamp_ms = peer.stats.v1.timestamp_ms();
                    let elapsed_ms = timestamp_ms - start_ms;
                    let timeout = peer.stats.v1.timeout();
                    if (timeout > 0)
                        && (elapsed_ms / timeout > 10)
                        && (timestamp_ms - last_diag_ms > Self::TIMEOUT_WARN_MS)
                    {
                        log::warn!(
                            target: TARGET,
                            "RLDPv1 send {transfer_str} masked timeout: \
                             elapsed {elapsed_ms} ms > 10*{timeout} ms, recv_seqno {new_recv_seqno}"
                        );
                        last_diag_ms = timestamp_ms;
                    }
                    peer.stats.v1.update(min_timeout_ms);
                    recv_seqno = new_recv_seqno;
                } else if peer.stats.v1.try_timeout(start_ms) {
                    #[cfg(feature = "telemetry")]
                    log::info!(
                        target: TARGET,
                        "RLDPv1 send: packets sent {total_packets} (timeout) in {transfer_str}"
                    );
                    return Ok(false);
                }
            }
            peer.stats.v1.update(min_timeout_ms);
        }
        #[cfg(feature = "telemetry")]
        log::info!(target: TARGET, "RLDPv1 send: packets sent {total_packets} in {transfer_str}");
        Ok(true)
    }

    async fn send_loop_v2(
        mut context: SendContext,
        transfer_state: Arc<Vec<Arc<SendPartStateV2>>>,
        peer: &Arc<RldpPeer>,
        min_timeout_ms: u64,
    ) -> Result<bool> {
        let transfer_str = Self::transfer_string(&context.transfer_id, &context.peers, "to");
        let SendTransfer::V2(part_transfers) = &mut context.send_transfer else {
            fail!("Unexpected V1 send transfer in V2 send loop")
        };
        #[cfg(any(feature = "debug", feature = "telemetry"))]
        let total_packets = Arc::new(AtomicU32::new(0));
        let progress = Arc::new(AtomicU64::new(0));
        let bbr_part_states = transfer_state.clone();
        let bbr_peer = peer.clone();
        let bbr_progress = progress.clone();
        let bbr_task = tokio::spawn(async move {
            bbr_peer.stats.bbr_step()?;
            loop {
                let mut in_progress = 0;
                let mut finished = 0;
                for part in bbr_part_states.iter() {
                    if part.is_finished().is_some() {
                        finished += 1
                    } else if part.is_started() {
                        in_progress += 1
                    }
                }
                if finished == bbr_part_states.len() {
                    break;
                }
                tokio::time::timeout(Duration::from_millis(Self::SPINNER_MS), context.pong.recv())
                    .await
                    .ok();
                let progress = bbr_progress.load(Ordering::Relaxed);
                let progress = progress as u32 - (progress >> 32) as u32;
                if progress >= in_progress as u32 {
                    bbr_peer.stats.bbr_step()?;
                    bbr_progress.fetch_add((progress as u64) << 32, Ordering::Relaxed);
                }
            }
            Ok(())
        });
        let start_ms = peer.stats.v1.timestamp_ms();
        let mut send_tasks = Vec::new();
        let ok = loop {
            while send_tasks.len() < Constraints::MAX_PARTS_IN_TRANSIT {
                if part_transfers.is_empty() {
                    break;
                }
                let mut transfer = part_transfers.remove(0);
                let context = SendPartContextV2 {
                    adnl: context.adnl.clone(),
                    part_states: transfer_state.clone(),
                    peer: peer.clone(),
                    peers: context.peers.clone(),
                    progress: progress.clone(),
                    #[cfg(feature = "telemetry")]
                    tag: context.tag,
                    #[cfg(feature = "debug")]
                    timestamp: context.timestamp.clone(),
                    #[cfg(any(feature = "debug", feature = "telemetry"))]
                    total_packets: total_packets.clone(),
                    transfer_str: transfer_str.clone(),
                };
                let send_task: tokio::task::JoinHandle<Result<bool>> = tokio::spawn(async move {
                    let ret =
                        Self::send_one_part_v2(&mut transfer, &context, start_ms, min_timeout_ms)
                            .await;
                    transfer.on_drop(&context.peer);
                    ret
                });
                send_tasks.push(send_task);
            }
            if send_tasks.is_empty() {
                #[cfg(feature = "debug")]
                Self::check_timestamp(
                    &context.timestamp,
                    format!(
                        "Send transfer finished, packets {}",
                        total_packets.load(Ordering::Relaxed)
                    )
                    .as_str(),
                );
                break Ok(true);
            }
            match futures::future::select_all(send_tasks).await {
                (Err(e), _, _) => break Err(e.into()),
                (Ok(Err(e)), _, _) => break Err(e),
                (Ok(Ok(ok)), _, wait_tasks) => {
                    if !ok {
                        break Ok(false);
                    } else {
                        send_tasks = wait_tasks;
                    }
                }
            }
        }?;
        #[cfg(feature = "telemetry")]
        log::info!(
            target: TARGET,
            "RLDPv2 send: packets sent {} ({}) in {transfer_str}",
            total_packets.load(Ordering::Relaxed),
            if ok { "ok" } else { "timeout" }
        );
        match bbr_task.await {
            Err(e) => Err(e.into()),
            Ok(Err(e)) => Err(e),
            Ok(Ok(_)) => Ok(ok),
        }
    }

    async fn send_one_part_v2(
        transfer: &mut SendPartV2,
        context: &SendPartContextV2,
        start_ms: u64,
        min_timeout_ms: u64,
    ) -> Result<bool> {
        let part = transfer.state().part();
        let on_ack = |transfer: &mut SendPartV2, ack| -> Result<u32> {
            let new_received = transfer.on_ack(ack, &context.peer, min_timeout_ms)?;
            if new_received > 0 {
                context.progress.fetch_add(1, Ordering::Relaxed);
                for (i, state) in context.part_states.iter().enumerate() {
                    if i == part as usize {
                        continue;
                    }
                    if !state.is_started() || state.is_finished().is_none() {
                        continue;
                    }
                    context.part_states[i].ack_ping()
                }
                transfer.state().ping();
            }
            Ok(new_received)
        };
        let mut updates = 0;
        let mut new_received = 0;
        let mut last_warn_ms = context.peer.stats.v1.timestamp_ms();
        let mut last_diag_ms = last_warn_ms;
        transfer.start()?;
        let ok = loop {
            if let Some(ok) = transfer.state().is_finished() {
                break ok;
            }
            if new_received > 0 {
                log::trace!(
                    target: TARGET,
                    "Send part {} updates {} -> {} in {}",
                    part, updates, updates + new_received, context.transfer_str
                );
                Self::check_too_long_transfer(
                    start_ms,
                    &mut last_warn_ms,
                    context.peer.stats.v1.timestamp_ms(),
                    &context.transfer_str,
                    "RLDPv2 send",
                );
                let timestamp_ms = context.peer.stats.v1.timestamp_ms();
                let elapsed_ms = timestamp_ms - start_ms;
                let timeout = context.peer.stats.v1.timeout();
                if (timeout > 0)
                    && (elapsed_ms / timeout > 10)
                    && (timestamp_ms - last_diag_ms > Self::TIMEOUT_WARN_MS)
                {
                    log::warn!(
                        target: TARGET,
                        "RLDPv2 send {} part {part} masked timeout: \
                         elapsed {elapsed_ms} ms > 10*{timeout} ms, updates {}",
                        context.transfer_str,
                        updates + new_received
                    );
                    last_diag_ms = timestamp_ms;
                }
                context.peer.stats.v1.update(min_timeout_ms);
                updates += new_received
            } else if context.peer.stats.v1.try_timeout(start_ms) {
                transfer.state().set_finished(false);
                continue;
            }
            new_received = 0;
            let (object, is_probe) = match transfer.prepare_chunk(&context.peer)? {
                SendActionV2::Send((object, is_probe)) => (object, is_probe),
                SendActionV2::Wait(timeout_micros) => {
                    #[cfg(feature = "debug")]
                    Self::check_timestamp(
                        &context.timestamp,
                        format!(
                            "Part {} suspended on {} micros, seqno {}",
                            part,
                            timeout_micros,
                            transfer.state().seqno_send()
                        )
                        .as_str(),
                    );
                    if let Ok(Some(Some(ack))) = tokio::time::timeout(
                        Duration::from_micros(timeout_micros),
                        transfer.ack_pong.recv(),
                    )
                    .await
                    {
                        new_received = on_ack(transfer, ack)?
                    }
                    continue;
                }
            };
            #[cfg(any(feature = "debug", feature = "telemetry"))]
            context.total_packets.fetch_add(1, Ordering::Relaxed);
            let chunk = TaggedByteSlice {
                object,
                #[cfg(feature = "telemetry")]
                tag: context.tag,
            };
            context.adnl.send_custom(&chunk, &context.peers).await?;
            transfer.on_send(is_probe, &context.peer)?;
            if let Ok(Some(ack)) = transfer.ack_pong.try_recv() {
                new_received = on_ack(transfer, ack)?
            }
        };
        #[cfg(feature = "debug")]
        Self::check_timestamp(
            &context.timestamp,
            format!(
                "Part {} finished {} in progress, seqno {}",
                part,
                if ok { "ok" } else { "with failure" },
                transfer.state().seqno_send()
            )
            .as_str(),
        );
        Ok(ok)
    }

    fn transfer_string(transfer_id: &TransferId, peers: &AdnlPeers, dir: &str) -> String {
        format!("transfer {} {dir} {}", base64_encode(transfer_id), peers.other())
    }
}

#[async_trait::async_trait]
impl Subscriber for RldpNode {
    #[cfg(feature = "telemetry")]
    async fn poll(&self, start: &Arc<Instant>) {
        if ((start.elapsed().as_secs() + 1) % Self::TIMEOUT_TELEMETRY_SEC) == 0 {
            self.print_stats()
        }
        self.telemetry.peers.update(self.allocated.peers.load(Ordering::Relaxed));
        self.telemetry.recv_transfers.update(self.allocated.recv_transfers.load(Ordering::Relaxed));
        self.telemetry.send_transfers.update(self.allocated.send_transfers.load(Ordering::Relaxed));
    }

    async fn try_consume_custom(&self, data: &[u8], peers: &AdnlPeers) -> Result<bool> {
        if let Some(local_id) = &self.local_id {
            if peers.local() != local_id {
                return Ok(false);
            }
        }
        let msg = if let Ok(msg) = deserialize_boxed(data) { msg } else { return Ok(false) };
        let msg = match msg.downcast::<RldpMessagePartBoxed>() {
            Ok(msg) => return self.consume_message_part(MessagePart::V1(msg), peers).await,
            Err(msg) => msg,
        };
        match msg.downcast::<Rldp2MessagePartBoxed>() {
            Ok(msg) => self.consume_message_part(MessagePart::V2(msg), peers).await,
            Err(_) => Ok(false),
        }
    }
}
