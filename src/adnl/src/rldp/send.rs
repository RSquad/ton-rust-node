/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    adnl::common::{AdnlPeers, CountedObject, Counter},
    declare_counted,
    node::AdnlNode,
    rldp::{stat::PacketBandwidthInfo, Constraints, RldpPeer, TransferId},
};
use std::{
    cmp::min,
    collections::HashMap,
    ops::Range,
    sync::{
        atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering},
        Arc,
    },
};
#[cfg(feature = "debug")]
use std::{sync::atomic::AtomicPtr, time::Instant};
use ton_api::{
    serialize_bare_inplace,
    ton::{
        fec::{type_::RaptorQ as FecTypeRaptorQ, Type as FecType},
        rldp::messagepart::MessagePart as RldpMessagePart,
        rldp2::messagepart::MessagePart as Rldp2MessagePart,
    },
    IntoBoxed, RldpChunk,
};
use ton_block::{fail, Result, UInt256};

/// RaptorQ encoder
pub struct RaptorqEncoder {
    encoder_index: usize,
    engine: raptorq::Encoder,
    params: FecTypeRaptorQ,
    source_packets: Vec<raptorq::EncodingPacket>,
}

impl RaptorqEncoder {
    /// Construct over data
    pub fn with_data(data: &[u8], symbol: Option<u16>) -> Self {
        let engine =
            if let Some(symbol_size) = symbol.filter(|&s| s as usize != Constraints::SYMBOL) {
                // symbol_size is set for the two-step FEC broadcast case where data is
                // sent to a fixed set of neighbours. In that case symbol_size is derived
                // as ceil(data.len() / k) where k = ((neighbours * 2) - 2) / 3.
                //
                // Use alignment=1 to match C++ behaviour: pass symbol_size as-is without
                // rounding down to a multiple of 8.
                //
                // source_blocks=1 is valid because the number of source symbols
                //   kt = ceil(data.len() / symbol_size) <= k
                // and k is bounded by the neighbour count (typically a few dozen, at most
                // ~130), which is far below K'_max = 56403 (RFC 6330 max symbols per block).
                //
                // sub_blocks=1: the data fits comfortably in memory since part_size < 65536
                // and kt <= k << K'_max, so no sub-block splitting is needed.
                let config = raptorq::ObjectTransmissionInformation::new(
                    data.len() as u64,
                    symbol_size,
                    1,
                    1,
                    1,
                );
                raptorq::Encoder::new(data, config)
            } else {
                raptorq::Encoder::with_defaults(data, Constraints::SYMBOL as u16)
            };
        let mut source_packets = Vec::new();
        for encoder in engine.get_block_encoders() {
            // Reverse order to send efficiently
            let mut packets = encoder.source_packets();
            while let Some(packet) = packets.pop() {
                source_packets.push(packet)
            }
        }
        let symbol_size = engine.get_config().symbol_size() as i32;
        Self {
            encoder_index: 0,
            engine,
            params: FecTypeRaptorQ {
                data_size: data.len() as i32,
                symbol_size,
                symbols_count: source_packets.len() as i32,
            },
            source_packets,
        }
    }

    /// Encode
    pub fn encode(&mut self, seqno: &mut u32) -> Result<Vec<u8>> {
        let encoders = self.engine.get_block_encoders();
        let packet = if let Some(packet) = self.source_packets.pop() {
            packet
        } else {
            let mut packets = encoders[self.encoder_index].repair_packets(*seqno, 1);
            let packet = if let Some(packet) = packets.pop() {
                packet
            } else {
                fail!("INTERNAL ERROR: cannot encode repair packet");
            };
            self.encoder_index += 1;
            if self.encoder_index >= encoders.len() {
                self.encoder_index = 0;
            }
            packet
        };
        *seqno = packet.payload_id().encoding_symbol_id();
        Ok(packet.data().to_vec())
    }

    /// Parameters
    pub fn params(&self) -> &FecTypeRaptorQ {
        &self.params
    }
}

pub(crate) struct SendContext {
    pub(crate) adnl: Arc<AdnlNode>,
    pub(crate) peers: AdnlPeers,
    pub(crate) pong: tokio::sync::mpsc::UnboundedReceiver<()>,
    pub(crate) send_transfer: SendTransfer,
    pub(crate) transfer_id: TransferId,
    #[cfg(feature = "debug")]
    pub(crate) timestamp: Arc<AtomicPtr<Instant>>,
    #[cfg(feature = "telemetry")]
    pub(crate) tag: u32,
}

struct SendPart {
    buf: Vec<u8>,
    data: Arc<[u8]>,
    encoder: Option<RaptorqEncoder>,
}

impl SendPart {
    fn new(data: Arc<[u8]>) -> Self {
        Self { buf: Vec::new(), data, encoder: None }
    }

    fn chunk(transfer_id: TransferId, total_size: usize) -> RldpChunk {
        let fec_type = FecTypeRaptorQ {
            data_size: 0,
            symbol_size: Constraints::SYMBOL as i32,
            symbols_count: 0,
        }
        .into_boxed();
        RldpChunk {
            transfer_id: UInt256::with_array(transfer_id),
            fec_type,
            part: 0,
            total_size: total_size as i64,
            seqno: 0,
            data: Vec::new(),
        }
    }

    fn encode_chunk(&mut self, chunk: &mut RldpChunk, mut seqno_send: u32) -> Result<()> {
        if let Some(encoder) = &mut self.encoder {
            let data = encoder.encode(&mut seqno_send)?;
            chunk.seqno = seqno_send as i32;
            chunk.data = data;
            Ok(())
        } else {
            fail!("Encoder is not ready");
        }
    }

    fn start(&mut self, part: u32, range: Range<usize>, chunk: &mut RldpChunk) -> Result<u32> {
        let encoder = RaptorqEncoder::with_data(&self.data[range], None);
        let ret = encoder.params.symbols_count;
        chunk.part = part as i32;
        match chunk.fec_type {
            FecType::Fec_RaptorQ(ref mut fec_type) => {
                fec_type.data_size = encoder.params.data_size;
                fec_type.symbols_count = ret;
            }
            _ => fail!("INTERNAL ERROR: unsupported FEC type"),
        }
        self.encoder = Some(encoder);
        Ok(ret as u32)
    }
}

pub(crate) struct SendPartV1 {
    core: SendPart,
    chunk: RldpMessagePart,
    state: Arc<SendPartStateV1>,
}

impl SendPartV1 {
    const WINDOW: usize = 1000;

    fn new(data: Arc<[u8]>, transfer_id: TransferId, state: Arc<SendPartStateV1>) -> Self {
        Self {
            chunk: RldpMessagePart(SendPart::chunk(transfer_id, data.len())),
            core: SendPart::new(data),
            state,
        }
    }

    pub(crate) fn prepare_chunk(&mut self) -> Result<(&[u8], bool)> {
        let seqno_send_original = self.state.core.seqno_send();
        let RldpMessagePart(chunk) = &mut self.chunk;
        self.core.encode_chunk(chunk, seqno_send_original)?;
        let mut seqno_send = chunk.seqno as u32;
        let seqno_recv = self.state.seqno_recv();
        let do_next = if seqno_send - seqno_recv <= Self::WINDOW as u32 {
            if seqno_send_original == seqno_send {
                seqno_send += 1;
            }
            self.state.core.set_seqno_send(seqno_send);
            true
        } else {
            false
        };
        serialize_bare_inplace(&mut self.core.buf, &self.chunk)?;
        Ok((&self.core.buf[..], do_next))
    }

    pub(crate) fn start_next_part(&mut self) -> Result<u32> {
        if self.state.is_transfer_finished() {
            return Ok(0);
        }
        let part = self.state.part();
        let offset = (part as usize) * Constraints::SLICE;
        if offset >= self.state.total {
            return Ok(0);
        }
        let upto = offset + Constraints::SLICE.min(self.state.total - offset);
        let RldpMessagePart(chunk) = &mut self.chunk;
        self.core.start(part, offset..upto, chunk)
    }

    pub(crate) fn state(&self) -> &Arc<SendPartStateV1> {
        &self.state
    }
}

pub(crate) struct SendAckV2 {
    max_seqno: u32,
    received_count: u32,
    received_mask: u32,
}

pub(crate) enum SendActionV2<'a> {
    Send((&'a [u8], SendInfoV2)),
    Wait(u64),
}

pub(crate) struct SendInfoV2 {
    is_probe: bool,
    seqno: u32,
}

pub(crate) struct SendPacketV2 {
    pub(crate) bandwidth_info: PacketBandwidthInfo,
    pub(crate) sent_at_micros: u64,
}

pub(crate) struct SendPartV2 {
    pub(crate) ack_pong: tokio::sync::mpsc::UnboundedReceiver<Option<SendAckV2>>,
    core: SendPart,
    chunk: Rldp2MessagePart,
    extra_symbols: u32,
    in_flight_symbols: u32,
    left_to_ack: u32,
    packets: HashMap<u32, SendPacketV2>,
    probe_at_micros: u64,
    probe_k: u32,
    range: Range<usize>,
    received_symbols: u32,
    seqno_max_recv: Option<u32>,
    seqno_min_sent: Option<u32>,
    state: Arc<SendPartStateV2>,
    total_ack: u32,
    total_ack_prev: u32,
    total_lost: u32,
    total_lost_prev: u32,
}

impl SendPartV2 {
    const MAX_PROBE_K: u32 = 10;
    const MIN_PROBE_K: u32 = 1;

    fn new(
        data: Arc<[u8]>,
        ack_pong: tokio::sync::mpsc::UnboundedReceiver<Option<SendAckV2>>,
        range: Range<usize>,
        transfer_id: TransferId,
        state: Arc<SendPartStateV2>,
    ) -> Result<Self> {
        let ret = Self {
            ack_pong,
            chunk: Rldp2MessagePart(SendPart::chunk(transfer_id, data.len())),
            core: SendPart::new(data),
            extra_symbols: 0,
            in_flight_symbols: 0,
            left_to_ack: 0,
            packets: HashMap::new(),
            probe_at_micros: 0,
            probe_k: Self::MIN_PROBE_K,
            range,
            received_symbols: 0,
            seqno_max_recv: None,
            seqno_min_sent: None,
            state,
            total_ack: 0,
            total_ack_prev: 0,
            total_lost: 0,
            total_lost_prev: 0,
        };
        Ok(ret)
    }

    pub(crate) fn on_ack(
        &mut self,
        ack: SendAckV2,
        peer: &Arc<RldpPeer>,
        min_timeout_ms: u64,
    ) -> Result<u32> {
        //  ack.max_seqno = td::min(ack.max_seqno, last_seqno_);
        //  ack.received_count = td::min(ack.received_count, ack.max_seqno);
        // TODO: seqno of rldp and seqno of a packet must be completly separate seqnos
        let new_received = if self.received_symbols < ack.received_count {
            let new_received = ack.received_count - self.received_symbols;
            self.left_to_ack = self.in_flight_symbols.min(self.left_to_ack + new_received);
            self.received_symbols = ack.received_count;
            new_received
        } else {
            0
        };
        let mut max_packet = None;
        let update = if let Some(seqno_max_recv) = &self.seqno_max_recv {
            *seqno_max_recv < ack.max_seqno
        } else {
            true
        };
        let mut new_dropped = 0;
        if update {
            self.seqno_max_recv = Some(ack.max_seqno);
            for i in 0..31 {
                let mask = 1u32 << i;
                if (mask > ack.received_mask) || (ack.max_seqno < i) {
                    break;
                }
                if (mask & ack.received_mask) == 0 {
                    continue;
                }
                let seqno = ack.max_seqno - i;
                let Some(packet) = self.packets.remove(&seqno) else { continue };
                self.count_ack();
                new_dropped += 1;
                if max_packet.is_none() {
                    max_packet.replace(packet);
                }
                let Some(seqno_min_sent) = &self.seqno_min_sent else { continue };
                if *seqno_min_sent == seqno {
                    self.seqno_min_sent = self.next_seqno_min_sent(*seqno_min_sent)
                }
            }
        }
        /*
        SenderPackets::Update SenderPackets::on_ack(Ack ack) {

          if (max_packet_.seqno > ack.max_seqno) {
            return update;
          }

          auto packet = get_packet(ack.max_seqno);
          if (!packet) {
            return update;
          }

          if (max_packet_.seqno < ack.max_seqno) {
            update.was_max_updated = true;
            max_packet_ = *packet;
          }

          for (td::uint32 i : td::BitsRange(ack.received_mask)) {
            if (ack.max_seqno < i) {
              break;
            }
            auto seqno = ack.max_seqno - i;
            auto packet = get_packet(seqno);
            if (!packet) {
              break;
            }
            mark_ack(*packet);
          }

          return update;
        }
        */

        /*
          auto update = packets_.on_ack(ack);
          if (!update.was_max_updated) {
            return update;
          }

          // update rtt
          ack_delay = td::clamp(ack_delay, 0.0, config_.max_ack_delay);
          auto rtt_sample = now.at() - packets_.max_packet().sent_at.at();
          rtt_stats.on_rtt_sample(rtt_sample, ack_delay, now);

          bdw_stats.on_update(now, update.new_received);
          bdw_stats.on_packet_ack(packets_.max_packet().bdw_packet_info, packets_.max_packet().sent_at, now);
        */

        /*
          while (!packets.empty()) {
            auto &packet = packets.();
            if (!limits.should_drop(packet)) {
              break;
            }
            mark_ack_or_lost(packet);
            packets.pop();
          }
          DropUpdate update;
          update.new_ack = total_ack_ - last_total_ack_;
          update.new_lost = total_lost_ - last_total_lost_;
          last_total_ack_ = total_ack_;
          last_total_lost_ = total_lost_;
          update.o_loss_at = std::move(last_loss_);
        */

        /*
          // drop ready packets
          SenderPackets::Limits limits;
          limits.sent_at = td::Timestamp::at(now.at() - get_loss_delay(rtt_stats));
          limits.seqno = sub_or_zero(packets_.max_packet().seqno, get_loss_seqno_delay());
          update.drop_update = packets_.drop_packets(limits);

          loss_stats.on_update(update.drop_update.new_ack, update.drop_update.new_lost);

          fec_helper_.received_symbols_count = packets_.received_count();
          extra_symbols_ = loss_stats.prob.send_n(fec_helper_.get_left_fec_symbols_count());
          return update;
        */
        if let Some(max_packet) = max_packet {
            peer.stats.on_ack(max_packet, new_received, min_timeout_ms)?;
            let sent_at_micros_drop = peer.stats.calc_loss_delay();
            let seqno_drop = ack.max_seqno.saturating_sub(peer.stats.config().packet_threshold);
            while let Some(seqno_min_sent) = &self.seqno_min_sent {
                let seqno_min_sent = *seqno_min_sent;
                let drop = if seqno_min_sent >= seqno_drop {
                    let Some(packet) = self.packets.get(&seqno_min_sent) else {
                        fail!("Packet {} is absent in stats", seqno_min_sent)
                    };
                    packet.sent_at_micros < sent_at_micros_drop
                } else {
                    true
                };
                if !drop {
                    break;
                }
                if self.packets.remove(&seqno_min_sent).is_none() {
                    fail!("Packet {} is absent in stats", seqno_min_sent)
                }
                if self.left_to_ack > 0 {
                    self.count_ack()
                } else {
                    self.count_lost()
                }
                new_dropped += 1;
                self.seqno_min_sent = self.next_seqno_min_sent(seqno_min_sent)
            }
            let new_ack = self.total_ack - self.total_ack_prev;
            self.total_ack_prev = self.total_ack;
            let new_loss = self.total_lost - self.total_lost_prev;
            self.total_lost_prev = self.total_lost;
            self.extra_symbols =
                peer.stats.calc_extra_symbols(new_ack, new_loss, self.get_left_symbols_count()?)?;
        }
        //println!("ACK task {} max {}, recv {}, new recv {}, drop {}", self.state.part, ack.max_seqno, ack.received_count, new_received, new_dropped);
        if new_dropped > 0 {
            peer.stats.on_drop(new_dropped)
        }
        Ok(new_received)
    }

    pub(crate) fn on_drop(&self, peer: &Arc<RldpPeer>) {
        peer.stats.on_drop(self.in_flight_symbols)
    }

    pub(crate) fn on_send(&mut self, info: SendInfoV2, peer: &Arc<RldpPeer>) -> Result<()> {
        let timestamp_micros = peer.stats.timestamp_micros();
        let first_sent_at_micros = self
            .seqno_min_sent
            .as_ref()
            .and_then(|seqno_min_sent| {
                self.packets.get(seqno_min_sent).map(|packet| packet.sent_at_micros)
            })
            .unwrap_or(timestamp_micros);
        let bandwidth_info = peer.stats.on_send(info.is_probe, first_sent_at_micros)?;
        /*
        void RldpSender::on_send(td::uint32 seqno, td::Timestamp now, bool is_probe, const RttStats &rtt_stats,
                                 const BdwStats &bdw_stats) {
          SenderPackets::Packet packet;
          packet.is_in_flight = true;
          packet.sent_at = now;
          packet.seqno = seqno;
          packet.size = 0;
          packet.bdw_packet_info = bdw_stats.on_packet_send(packets_.first_sent_at(now));
          packets_.send(packet);
        }
        */
        self.probe_at_micros = peer.stats.calc_probe_delay(self.probe_k);
        if info.is_probe {
            self.probe_k = min(self.probe_k * 2, Self::MAX_PROBE_K)
        } else {
            self.probe_k = Self::MIN_PROBE_K
        }
        let packet = SendPacketV2 { bandwidth_info, sent_at_micros: timestamp_micros };
        if self.packets.insert(info.seqno, packet).is_none() {
            self.in_flight_symbols += 1;
            self.seqno_min_sent.get_or_insert(info.seqno);
        }
        Ok(())
    }

    pub(crate) fn prepare_chunk(&mut self, peer: &Arc<RldpPeer>) -> Result<SendActionV2<'_>> {
        let only_probe = peer.stats.is_only_probe();
        let timestamp_micros = peer.stats.timestamp_micros();
        let probe = if only_probe || (self.extra_symbols <= self.in_flight_symbols) {
            if self.probe_at_micros <= timestamp_micros {
                true
            } else {
                //println!("WAIT part {}, only_probe {}, received {}, extra {}, in_flight {} probe {}, now {}",
                //self.state.part, only_probe, self.received_symbols, self.extra_symbols, self.in_flight_symbols, self.probe_at_micros, timestamp_micros);
                return Ok(SendActionV2::Wait(self.probe_at_micros - timestamp_micros));
            }
        } else {
            false
        };
        let seqno_send_original = self.state.core.seqno_send();
        let Rldp2MessagePart(chunk) = &mut self.chunk;
        self.core.encode_chunk(chunk, seqno_send_original)?;
        let mut seqno_send = chunk.seqno as u32;
        let info = SendInfoV2 { is_probe: probe, seqno: seqno_send };
        if seqno_send_original == seqno_send {
            seqno_send += 1;
        }
        self.state.core.set_seqno_send(seqno_send);
        serialize_bare_inplace(&mut self.core.buf, &self.chunk)?;
        //if probe {
        //println!("SENT part {} {} extra {} only_probe {}", self.state.part, info.seqno, self.extra_symbols, only_probe);
        //}
        Ok(SendActionV2::Send((&self.core.buf[..], info)))
    }

    pub(crate) fn start(&mut self) -> Result<()> {
        let Rldp2MessagePart(chunk) = &mut self.chunk;
        self.core.start(self.state.part, self.range.clone(), chunk)?;
        self.state.status.store(SendPartStateV2::STATUS_STARTED, Ordering::Relaxed);
        self.extra_symbols = self.get_left_symbols_count()?;
        Ok(())
    }

    pub(crate) fn state(&self) -> &Arc<SendPartStateV2> {
        &self.state
    }

    fn count_ack(&mut self) {
        if self.left_to_ack > 0 {
            self.left_to_ack -= 1
        }
        self.total_ack += 1;
        self.in_flight_symbols -= 1
    }

    fn count_lost(&mut self) {
        self.total_lost += 1;
        self.in_flight_symbols -= 1
    }

    fn get_left_symbols_count(&self) -> Result<u32> {
        // Find smallest (symbols_count + x + y * i) > received_symbols
        const X: u32 = 5;
        const Y: u32 = 5;
        let Rldp2MessagePart(chunk) = &self.chunk;
        let symbols = match &chunk.fec_type {
            FecType::Fec_RaptorQ(fec_type) => fec_type.symbols_count as u32,
            _ => fail!("INTERNAL ERROR: unsupported FEC type"),
        };
        let symbols = if symbols + X > self.received_symbols {
            symbols + X
        } else {
            let i = (self.received_symbols - (symbols + X)) / Y + 1;
            symbols + X + i * Y
        };
        Ok(symbols - self.received_symbols)
    }

    fn next_seqno_min_sent(&self, mut seqno_min_sent: u32) -> Option<u32> {
        if !self.packets.is_empty() {
            let seqno_min_sent = loop {
                seqno_min_sent += 1;
                if self.packets.get(&seqno_min_sent).is_some() {
                    break seqno_min_sent;
                }
            };
            Some(seqno_min_sent)
        } else {
            None
        }
    }
}

pub(crate) struct SendPartContextV2 {
    pub(crate) adnl: Arc<AdnlNode>,
    pub(crate) part_states: Arc<Vec<Arc<SendPartStateV2>>>,
    pub(crate) peer: Arc<RldpPeer>,
    pub(crate) peers: AdnlPeers,
    pub(crate) progress: Arc<AtomicU64>,
    #[cfg(feature = "telemetry")]
    pub(crate) tag: u32,
    #[cfg(feature = "debug")]
    pub(crate) timestamp: Arc<AtomicPtr<Instant>>,
    #[cfg(any(feature = "debug", feature = "telemetry"))]
    pub(crate) total_packets: Arc<AtomicU32>,
    pub(crate) transfer_str: String,
}

struct SendPartState {
    ping: tokio::sync::mpsc::UnboundedSender<()>,
    seqno_send: AtomicU32,
}

impl SendPartState {
    fn seqno_send(&self) -> u32 {
        self.seqno_send.load(Ordering::Relaxed)
    }

    fn set_seqno_send(&self, seqno: u32) {
        let seqno_send = self.seqno_send();
        if seqno_send < seqno {
            self.seqno_send
                .compare_exchange(seqno_send, seqno, Ordering::Relaxed, Ordering::Relaxed)
                .ok();
        }
    }
}

declare_counted!(
    pub(crate) struct SendPartStateV1 {
        core: SendPartState,
        part: AtomicU32,
        seqno_recv: AtomicU32,
        total: usize,
    }
);

impl SendPartStateV1 {
    pub(crate) fn is_transfer_finished_or_next_part(&self, part: u32) -> Result<bool> {
        if self.is_transfer_finished() {
            Ok(true)
        } else {
            match self.part() {
                x if x == part => Ok(false),
                x if x == part + 1 => Ok(true),
                _ => fail!("INTERNAL ERROR: part # mismatch"),
            }
        }
    }

    pub(crate) fn part(&self) -> u32 {
        self.part.load(Ordering::Relaxed)
    }

    pub(crate) fn seqno_recv(&self) -> u32 {
        self.seqno_recv.load(Ordering::Relaxed)
    }

    #[cfg(feature = "debug")]
    pub(crate) fn seqno_send(&self) -> u32 {
        self.core.seqno_send()
    }

    pub(crate) fn set_next_part(&self, part: u32) {
        if self.part.compare_exchange(part - 1, part, Ordering::Relaxed, Ordering::Relaxed).is_ok()
        {
            self.seqno_recv.store(0, Ordering::Relaxed);
            self.core.seqno_send.store(0, Ordering::Relaxed);
            self.core.ping.send(()).ok();
        }
    }

    pub(crate) fn set_seqno_recv(&self, seqno: u32) {
        if self.core.seqno_send() > seqno {
            let seqno_recv = self.seqno_recv();
            if seqno_recv < seqno {
                if self
                    .seqno_recv
                    .compare_exchange(seqno_recv, seqno, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
                {
                    self.core.ping.send(()).ok();
                }
            }
        }
    }

    fn is_transfer_finished(&self) -> bool {
        (self.part() as usize) * Constraints::SLICE >= self.total
    }
}

declare_counted!(
    pub(crate) struct SendPartStateV2 {
        part: u32,
        ack_ping: tokio::sync::mpsc::UnboundedSender<Option<SendAckV2>>,
        core: SendPartState,
        status: AtomicU8,
    }
);

impl SendPartStateV2 {
    const STATUS_FAILURE: u8 = 3;
    const STATUS_IDLE: u8 = 0;
    const STATUS_STARTED: u8 = 1;
    const STATUS_SUCCESS: u8 = 2;

    pub(crate) fn ack_ping(&self) {
        self.ack_ping.send(None).ok();
    }

    pub(crate) fn is_finished(&self) -> Option<bool> {
        match self.status.load(Ordering::Relaxed) {
            Self::STATUS_FAILURE => Some(false),
            Self::STATUS_SUCCESS => Some(true),
            _ => None,
        }
    }

    pub(crate) fn is_started(&self) -> bool {
        self.status.load(Ordering::Relaxed) == Self::STATUS_STARTED
    }

    pub(crate) fn part(&self) -> u32 {
        self.part
    }

    pub(crate) fn ping(&self) {
        self.core.ping.send(()).ok();
    }

    #[cfg(feature = "debug")]
    pub(crate) fn seqno_send(&self) -> u32 {
        self.core.seqno_send()
    }

    pub(crate) fn set_finished(&self, ok: bool) {
        self.status
            .store(if ok { Self::STATUS_SUCCESS } else { Self::STATUS_FAILURE }, Ordering::Relaxed);
        self.ack_ping();
        self.ping()
    }

    pub(crate) fn set_recv_info(&self, max_seqno: u32, received_count: u32, received_mask: u32) {
        if (self.core.seqno_send() <= max_seqno) || (received_count > max_seqno + 1) {
            return;
        }
        let ack = SendAckV2 { max_seqno, received_count, received_mask };
        self.ack_ping.send(Some(ack)).ok();
    }
}

pub(crate) enum SendTransferState {
    V1(Arc<SendPartStateV1>),
    V2(Arc<Vec<Arc<SendPartStateV2>>>),
}

pub(crate) enum SendTransfer {
    V1(SendPartV1),
    V2(Vec<SendPartV2>),
}

impl SendTransfer {
    pub(crate) fn new(
        data: Arc<[u8]>,
        transfer_id: TransferId,
        counter: Arc<AtomicU64>,
        ping: tokio::sync::mpsc::UnboundedSender<()>,
        v2: bool,
    ) -> Result<(Self, SendTransferState)> {
        if v2 {
            let mut parts = Vec::new();
            let mut states = Vec::new();
            let mut create_part = |data, range, part, transfer_id, counter, ping| -> Result<()> {
                let (ack_ping, ack_pong) = tokio::sync::mpsc::unbounded_channel();
                let state = SendPartStateV2 {
                    ack_ping,
                    core: SendPartState { seqno_send: AtomicU32::new(0), ping },
                    part,
                    status: AtomicU8::new(SendPartStateV2::STATUS_IDLE),
                    counter,
                };
                let state = Arc::new(state);
                parts.push(SendPartV2::new(data, ack_pong, range, transfer_id, state.clone())?);
                states.push(state);
                Ok(())
            };
            let mut offsets = vec![0];
            let mut offset = 0;
            while offset < data.len() {
                if Constraints::SLICE >= data.len() - offset {
                    break;
                }
                offset += Constraints::SLICE;
                offsets.push(offset)
            }
            for i in 0..offsets.len() - 1 {
                let upto = if i + 1 < offsets.len() { offsets[i + 1] } else { data.len() };
                create_part(
                    data.clone(),
                    offsets[i]..upto,
                    i as u32,
                    transfer_id.clone(),
                    counter.clone().into(),
                    ping.clone(),
                )?;
            }
            let len = data.len();
            create_part(
                data,
                offsets[offsets.len() - 1]..len,
                offsets.len() as u32 - 1,
                transfer_id,
                counter.into(),
                ping,
            )?;
            Ok((Self::V2(parts), SendTransferState::V2(Arc::new(states))))
        } else {
            let state = SendPartStateV1 {
                core: SendPartState { ping, seqno_send: AtomicU32::new(0) },
                part: AtomicU32::new(0),
                seqno_recv: AtomicU32::new(0),
                total: data.len(),
                counter: counter.into(),
            };
            let state = Arc::new(state);
            let part = SendPartV1::new(data, transfer_id, state.clone());
            Ok((Self::V1(part), SendTransferState::V1(state)))
        }
    }
}
