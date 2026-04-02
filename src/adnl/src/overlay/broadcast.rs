/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    common::{
        add_unbound_object_to_map, hash, AdnlPeers, CountedObject, Counter, TaggedByteSlice,
        UpdatedAt, Version,
    },
    declare_counted,
    node::PeerHistory,
    overlay::{Overlay, OverlayNode, TARGET, TARGET_BROADCAST},
    rldp::{RaptorqDecoder, RaptorqEncoder},
};
#[cfg(feature = "telemetry")]
use std::sync::atomic::AtomicU32;
use std::{
    fmt::{Display, Formatter},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
#[cfg(feature = "telemetry")]
use ton_api::Constructor;
use ton_api::{
    serialize_bare, serialize_boxed,
    ton::{
        fec::{type_::RaptorQ as FecTypeRaptorQ, Type as FecType},
        overlay::{
            broadcast::{
                id::Id as BroadcastSimpleId, tosign::ToSign as BroadcastToSign,
                Broadcast as BroadcastSimple, BroadcastFec, BroadcastStream, BroadcastTwostepFec,
                BroadcastTwostepSimple,
            },
            broadcast_fec::{id::Id as BroadcastFecId, partid::PartId as BroadcastFecPartId},
            broadcast_twostep::id::Id as BroadcastTwostepId,
            broadcast_twostep_fec::tosign::ToSign as BroadcastTwostepFecToSign,
            broadcast_twostep_simple::tosign::ToSign as BroadcastTwostepSimpleToSign,
            Broadcast, Certificate as OverlayCertificate,
        },
    },
    IntoBoxed,
};
use ton_block::{base64_encode, error, fail, sha256_digest, KeyId, KeyOption, Result, UInt256};

pub(crate) struct BroadcastCheckInfo {
    bcast_id: BroadcastId,
    dup: bool,
    data_len: usize,
    seqno: u32,
    #[cfg(feature = "telemetry")]
    maybe_tag: Option<u32>,
}

pub(crate) enum BroadcastData<'a> {
    Buf(Vec<u8>),
    None,
    Raw(&'a [u8]),
    Stream(Option<BroadcastStream>),
}

pub(crate) type BroadcastId = [u8; 32];

pub(crate) enum BroadcastJob {
    Background(u32),
    Foreground(Broadcast),
}

pub(crate) struct BroadcastNeighbours {
    expected_count: u32,
    hops: Option<u8>,
    ids: Option<Vec<Arc<KeyId>>>,
}

impl BroadcastNeighbours {
    fn get_ids(&mut self) -> Option<&mut Vec<Arc<KeyId>>> {
        self.ids.as_mut().and_then(|ids| if ids.is_empty() { None } else { Some(ids) })
    }
    fn skip(&mut self, skip: Option<&Arc<KeyId>>) {
        if let Some(ids) = &mut self.ids {
            if let Some(skip) = skip {
                ids.retain(|id| id != skip);
            }
        }
    }
}

pub struct BroadcastRecvInfo {
    pub packets: u32,
    pub data: Vec<u8>,
    pub extra: Option<Vec<u8>>,
    pub recv_from: Arc<KeyId>,
}

#[derive(Debug, Default)]
pub struct BroadcastSendInfo {
    pub packets: u32,
    pub send_to: u32,
}

pub(crate) struct BroadcastRecvContext<'a> {
    pub(crate) data: BroadcastData<'a>,
    pub(crate) hops: Option<u8>,
    pub(crate) overlay: &'a Arc<Overlay>,
    pub(crate) peers: &'a AdnlPeers,
}

pub(crate) struct BroadcastSendContext<'a> {
    pub(crate) data: &'a TaggedByteSlice<'a>,
    pub(crate) flags: u32,
    pub(crate) overlay: &'a Arc<Overlay>,
    pub(crate) src_key: &'a Arc<dyn KeyOption>,
    pub(crate) src_adnl_key_id: &'a Arc<KeyId>,
}

pub(crate) enum BroadcastSendMethod {
    Fast,
    QuicOrRldp,
    Safe,
}

impl Display for BroadcastSendMethod {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        let msg = match self {
            Self::Fast => "datagram ADNL",
            Self::QuicOrRldp => "QUIC/RLDP",
            Self::Safe => "stream ADNL",
        };
        write!(f, "{msg}")
    }
}

#[derive(Clone, PartialEq)]
pub(crate) enum BroadcastType {
    Fec,
    Simple,
    StreamSimple,
    TwostepFec,
    TwostepSimple,
}

impl Display for BroadcastType {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        let msg = match self {
            Self::Fec => "datagram FEC",
            Self::Simple => "datagram simple",
            Self::StreamSimple => "stream simple",
            Self::TwostepFec => "two-step datagram FEC",
            Self::TwostepSimple => "two-step datagram simple",
        };
        write!(f, "{msg}")
    }
}

pub(crate) enum OwnedBroadcast {
    Send,
    RecvFec(RecvTransferFec<BroadcastFec>),
    RecvTwostepFec(RecvTransferFec<BroadcastTwostepFec>),
    WillBeRecv(BroadcastType),
}

declare_counted!(
    pub(crate) struct RecvTransferFec<T> {
        completed: AtomicBool,
        data_hash: [u8; 32],
        history: PeerHistory,
        sender: tokio::sync::mpsc::UnboundedSender<Option<T>>,
        src_key_id: Arc<KeyId>,
        #[cfg(feature = "telemetry")]
        telemetry: RecvTransferFecTelemetry,
        updated_at: UpdatedAt,
    }
);

#[cfg(feature = "telemetry")]
impl<T> RecvTransferFec<T> {
    pub(crate) fn get_telemetry_tag_and_len(&self) -> Option<(u32, u32)> {
        if self.updated_at.is_expired(5) {
            return None;
        }
        let mut tag = self.telemetry.tag.load(Ordering::Relaxed);
        let flags = self.telemetry.flags.load(Ordering::Relaxed);
        if (flags & RecvTransferFecTelemetry::FLAG_RECEIVED) == 0 {
            tag |= flags;
        }
        Some((tag, self.telemetry.len.load(Ordering::Relaxed)))
    }
}

#[cfg(feature = "telemetry")]
struct RecvTransferFecTelemetry {
    flags: AtomicU32,
    len: AtomicU32,
    tag: AtomicU32,
}

#[cfg(feature = "telemetry")]
impl RecvTransferFecTelemetry {
    const FLAG_RECEIVE_STARTED: u32 = 0x01;
    const FLAG_RECEIVED: u32 = 0x02;
    const FLAG_FAILED: u32 = 0x04;
}

declare_counted!(
    struct SendTransferFec {
        bcast_id: BroadcastId,
        data_hash: [u8; 32],
        date: i32,
        encoder: RaptorqEncoder,
        extra: Vec<u8>,
        flags: u32,
        seqno: u32,
        src_key: Arc<dyn KeyOption>,
        src_adnl_key_id: Arc<KeyId>,
    }
);

// Broadcast traits ***********************************************************

pub(crate) trait BroadcastParsed {
    fn date(&self) -> i32;
    fn src_key(&self) -> Result<Arc<dyn KeyOption>>;
    #[cfg(feature = "telemetry")]
    fn default_tag(&self) -> u32;
    #[cfg(feature = "xp25")]
    fn flags(&self) -> u32;
}

#[async_trait::async_trait]
pub(crate) trait BroadcastProtocol<T: BroadcastParsed + Send + 'static>:
    Send + Sync
{
    // Common

    fn broadcast_type() -> BroadcastType;
    fn calc_broadcast_neighbours(
        &self,
        overlay: &Arc<Overlay>,
        hops: Option<u8>,
    ) -> Result<BroadcastNeighbours>;
    fn select_broadcast_neighbours(
        &self,
        overlay: &Arc<Overlay>,
        neighbours: &mut BroadcastNeighbours,
        skip: Option<&Arc<KeyId>>,
    );
    fn send_method(&self) -> BroadcastSendMethod;

    fn calc_broadcast_timeout(#[cfg(feature = "xp25")] flags: u32) -> u64 {
        #[cfg(feature = "xp25")]
        if (flags & OverlayNode::FLAG_BCAST_REPEATED) != 0 {
            return Overlay::TIMEOUT_REPEATED_BROADCAST_SEC;
        }
        Overlay::TIMEOUT_ONE_TIME_BROADCAST_SEC
    }

    async fn distribute_broadcast(
        overlay: &Arc<Overlay>,
        bcast: &[u8],
        bcast_id: &BroadcastId,
        seqno: u32,
        neighbours: &[Arc<KeyId>],
        src_adnl_key_id: &Arc<KeyId>,
        send_method: &BroadcastSendMethod,
        #[cfg(feature = "telemetry")] tag: u32,
    ) -> Result<()> {
        overlay
            .distribute_broadcast(
                &TaggedByteSlice {
                    object: bcast,
                    #[cfg(feature = "telemetry")]
                    tag,
                },
                bcast_id,
                seqno,
                src_adnl_key_id,
                neighbours,
                send_method,
            )
            .await
    }

    // Receive side

    fn check_broadcast(&self, bcast: &T, ctx: &BroadcastRecvContext) -> Result<BroadcastCheckInfo>;
    async fn process_broadcast(
        &self,
        bcast: T,
        ctx: &mut BroadcastRecvContext,
        bcast_id: &BroadcastId,
    ) -> Result<(Option<BroadcastRecvInfo>, bool)>;

    fn build_rebroadcast(
        &self,
        ctx: &mut BroadcastRecvContext,
        neighbours: &mut BroadcastNeighbours,
    ) -> Result<()> {
        // Transit broadcasts will be traced untagged
        if let Some(hops) = &neighbours.hops {
            let BroadcastData::Raw(raw_data) = &ctx.data else {
                fail!("INTERNAL ERROR: unexpected broadcast data type for rebroadcast");
            };
            let mut len = raw_data.len();
            if ctx.hops.is_none() {
                // Initial broadcast didn't count hops
                len += 1;
            }
            let mut buf = Vec::with_capacity(len);
            buf.extend_from_slice(raw_data);
            if ctx.hops.is_some() {
                buf[len - 1] = *hops;
            } else {
                buf.push(*hops);
            }
            ctx.data = BroadcastData::Buf(buf);
        }
        Ok(self.select_broadcast_neighbours(ctx.overlay, neighbours, Some(ctx.peers.other())))
    }

    async fn recv<'a>(&mut self, bcast: T, mut ctx: BroadcastRecvContext<'a>) -> Result<()> {
        let bcast_type = Self::broadcast_type();
        let now = Version::get();
        #[cfg(feature = "xp25")]
        let flags = bcast.flags();
        let timeout_sec = Self::calc_broadcast_timeout(
            #[cfg(feature = "xp25")]
            flags,
        );
        if bcast.date() + (timeout_sec as i32) < now {
            log::warn!(
                target: TARGET,
                "Old {bcast_type} broadcast {} seconds old from {} in overlay {}",
                now - bcast.date(),
                ctx.peers.other(),
                ctx.overlay.overlay_id
            );
            return Ok(());
        }
        let info = self.check_broadcast(&bcast, &ctx)?;
        if info.dup {
            let bcast_id = base64_encode(&info.bcast_id);
            log::info!(target: TARGET, "Received duplicated {bcast_type} broadcast {bcast_id}");
            return Ok(());
        };
        log::trace!(target: TARGET, "Received {bcast_type} broadcast, {} bytes", info.data_len);
        #[cfg(feature = "telemetry")]
        let tag = info.maybe_tag.unwrap_or(bcast.default_tag());
        #[cfg(feature = "telemetry")]
        log::info!(
            target: TARGET_BROADCAST,
            "Broadcast trace: recv {bcast_type} {} {} bytes, \
            tag {tag:08x} hops {:?} from {} in overlay {}",
            base64_encode(&info.bcast_id),
            info.data_len,
            ctx.hops,
            ctx.peers.other(),
            ctx.overlay.overlay_id
        );
        let (recv, resend) = self.process_broadcast(bcast, &mut ctx, &info.bcast_id).await?;
        if let Some(recv) = recv {
            ctx.overlay.received_rawbytes.push(recv);
            ctx.overlay.setup_broadcast_purge(
                info.bcast_id,
                #[cfg(feature = "xp25")]
                flags,
            )?;
        }
        if resend {
            self.resend_broadcast(
                &mut ctx,
                &info.bcast_id,
                info.seqno,
                #[cfg(feature = "telemetry")]
                tag,
            )
            .await?;
        }
        Ok(())
    }

    async fn resend_broadcast(
        &mut self,
        ctx: &mut BroadcastRecvContext,
        bcast_id: &BroadcastId,
        seqno: u32,
        #[cfg(feature = "telemetry")] tag: u32,
    ) -> Result<()> {
        let options = ctx.overlay.options.load(Ordering::Relaxed);
        if (options & Overlay::OPTION_DISABLE_BROADCAST_RETRANSMIT) != 0 {
            return Ok(());
        }
        let hops = if let Some(hops) = ctx.hops {
            if (hops & 0x0F) <= 1 {
                return Ok(());
            }
            Some(hops)
        } else {
            ctx.overlay.hops
        };
        let mut neighbours = self.calc_broadcast_neighbours(ctx.overlay, hops)?;
        self.build_rebroadcast(ctx, &mut neighbours)?;
        let Some(neighbours) = neighbours.get_ids() else {
            return Ok(());
        };
        let data = match &ctx.data {
            BroadcastData::Buf(data) => data,
            BroadcastData::None => return Ok(()),
            BroadcastData::Raw(data) => *data,
            BroadcastData::Stream(_) => fail!("INTERNAL ERROR: unprepared stream data"),
        };
        #[cfg(feature = "telemetry")]
        match Self::broadcast_type() {
            BroadcastType::Fec | BroadcastType::TwostepFec => {
                let stats = ctx.overlay.get_per_transfer_stats(bcast_id)?;
                stats.resent.fetch_add(neighbours.len() as u64, Ordering::Relaxed);
            }
            _ => (),
        };
        Self::distribute_broadcast(
            ctx.overlay,
            data,
            bcast_id,
            seqno,
            &neighbours,
            ctx.peers.local(),
            &self.send_method(),
            #[cfg(feature = "telemetry")]
            tag,
        )
        .await
    }

    // Send side

    fn calc_broadcast_id(
        &self,
        ctx: &BroadcastSendContext,
        date: i32,
    ) -> Result<(BroadcastId, bool)>;
    fn build_broadcast(
        &mut self,
        ctx: &BroadcastSendContext,
        bcast_id: &BroadcastId,
        date: i32,
        neighbours: &mut BroadcastNeighbours,
    ) -> Result<BroadcastJob>;

    async fn send<'a>(&mut self, ctx: BroadcastSendContext<'a>) -> Result<BroadcastSendInfo> {
        let mut neighbours = self.calc_broadcast_neighbours(ctx.overlay, ctx.overlay.hops)?;
        let bcast_type = Self::broadcast_type();
        let date = Version::get();
        let (bcast_id, allow_dup) = self.calc_broadcast_id(&ctx, date)?;
        let added = add_unbound_object_to_map(&ctx.overlay.owned_broadcasts, bcast_id, || {
            Ok(OwnedBroadcast::Send)
        })?;
        if !added && !allow_dup {
            let msg = base64_encode(bcast_id);
            #[cfg(feature = "telemetry")]
            let msg = format!("{msg}, tag {:08x}", ctx.data.tag);
            log::warn!(target: TARGET, "Try to send duplicated {bcast_type} broadcast {msg}");
            return Ok(BroadcastSendInfo::default());
        }
        let repeated = !added && allow_dup;
        #[cfg(feature = "xp25")]
        let repeated = repeated || ((ctx.flags & OverlayNode::FLAG_BCAST_REPEATED) != 0);
        if repeated {
            let msg = base64_encode(&bcast_id);
            #[cfg(feature = "telemetry")]
            let msg = format!("{msg}, tag {:08x}", ctx.data.tag);
            log::debug!(target: TARGET, "Sending repeated {bcast_type} broadcast {msg}");
        }
        #[cfg(feature = "telemetry")]
        log::info!(
            target: TARGET_BROADCAST,
            "Broadcast trace: send {bcast_type} {} {} bytes, tag {:08x} to overlay {}",
            base64_encode(&bcast_id),
            ctx.data.object.len(),
            ctx.data.tag,
            ctx.overlay.overlay_id
        );
        match self.build_broadcast(&ctx, &bcast_id, date, &mut neighbours)? {
            BroadcastJob::Background(packets) => {
                let send_to = if let Some(neighbours) = neighbours.get_ids() {
                    neighbours.len() as u32
                } else {
                    0
                };
                Ok(BroadcastSendInfo { packets, send_to })
            }
            BroadcastJob::Foreground(bcast) => {
                let mut buf = ctx.overlay.serialize_broadcast(&bcast)?;
                if let Some(hops) = &neighbours.hops {
                    buf.push(*hops);
                }
                self.select_broadcast_neighbours(ctx.overlay, &mut neighbours, None);
                let send_to = if let Some(neighbours) = neighbours.get_ids() {
                    Self::distribute_broadcast(
                        ctx.overlay,
                        &buf,
                        &bcast_id,
                        0,
                        &neighbours,
                        &ctx.src_adnl_key_id,
                        &self.send_method(),
                        #[cfg(feature = "telemetry")]
                        ctx.data.tag,
                    )
                    .await?;
                    ctx.overlay.setup_broadcast_purge(
                        bcast_id,
                        #[cfg(feature = "xp25")]
                        ctx.flags,
                    )?;
                    neighbours.len() as u32
                } else {
                    0
                };
                Ok(BroadcastSendInfo { packets: 1, send_to })
            }
        }
    }
}

pub(crate) trait FecBroadcastParsed: BroadcastParsed {
    fn data_hash(&self) -> &[u8; 32];
    fn data_size(&self) -> usize;
    fn extra(&self) -> Option<&[u8]> {
        None
    }
    fn fec_type(&self) -> Option<FecTypeRaptorQ>;
    fn part_data(&self) -> &[u8];
    fn seqno(&self) -> u32;
    fn signature(&self) -> &[u8];
}

#[async_trait::async_trait]
trait FecProtocol<T: FecBroadcastParsed + Send + 'static>: BroadcastProtocol<T> {
    fn build_broadcast_part(
        data: Vec<u8>,
        transfer: &SendTransferFec,
        signature: Vec<u8>,
    ) -> Result<Broadcast>;
    fn calc_to_sign(
        bcast_id: &BroadcastId,
        data_size: usize,
        part_data: &[u8],
        seqno: u32,
        date: i32,
    ) -> Result<Vec<u8>>;
    fn unwrap_transfer(wrapped: &OwnedBroadcast) -> Option<&RecvTransferFec<T>>;
    fn wrap_transfer(transfer: RecvTransferFec<T>) -> OwnedBroadcast;

    fn create_transfer(
        ctx: &BroadcastRecvContext,
        bcast_id: &BroadcastId,
        bcast: &T,
    ) -> Result<OwnedBroadcast> {
        let bcast_type = Self::broadcast_type();
        let Some(fec_type) = bcast.fec_type() else {
            fail!("Wrong FEC type set for {bcast_type} broadcast");
        };
        let (sender, mut reader) = tokio::sync::mpsc::unbounded_channel();
        let mut decoder = RaptorqDecoder::with_params(fec_type.clone())?;
        let bcast_id_recv = *bcast_id;
        let overlay = ctx.overlay.clone();
        let src_key = bcast.src_key()?;
        let src_key_id = src_key.id().clone();
        tokio::spawn(async move {
            let mut received = false;
            let mut packets = 0;
            let mut extra: Option<Vec<u8>> = None;
            #[cfg(feature = "telemetry")]
            let mut flags = RecvTransferFecTelemetry::FLAG_RECEIVE_STARTED;
            #[cfg(feature = "telemetry")]
            let mut len = 0;
            let mut tag = 0;
            while let Some(bcast) = reader.recv().await {
                let bcast: T = match bcast {
                    Some(bcast) => bcast,
                    None => break,
                };
                if extra.is_none() {
                    extra = bcast.extra().map(|extra| extra.to_vec());
                }
                packets += 1;
                let Some(fec_type) = bcast.fec_type() else {
                    log::warn!(
                        target: TARGET,
                        "FEC type is not properly set for {bcast_type} broadcast in overlay {}",
                        overlay.overlay_id
                    );
                    continue;
                };
                let other_fec_type = decoder.params();
                if fec_type != *other_fec_type {
                    log::warn!(
                        target: TARGET,
                        "Mismatch in FEC parameters of {bcast_type} broadcast in overlay {}: \
                        {fec_type:?} vs. {other_fec_type:?}",
                        overlay.overlay_id
                    );
                    continue;
                }
                match Self::process_part(&mut decoder, &bcast_id_recv, &bcast) {
                    Err(e) => {
                        log::warn!(
                            target: TARGET,
                            "Error when processing {bcast_type} broadcast in overlay {}: {e}",
                            overlay.overlay_id
                        );
                        #[cfg(feature = "telemetry")]
                        {
                            flags |= RecvTransferFecTelemetry::FLAG_FAILED;
                        }
                    }
                    Ok(Some(data)) => {
                        if data.len() > 4 {
                            tag = u32::from_le_bytes([data[0], data[1], data[2], data[3]])
                        }
                        #[cfg(feature = "telemetry")]
                        {
                            len = data.len() as u32;
                            flags |= RecvTransferFecTelemetry::FLAG_RECEIVED;
                        }
                        overlay.received_rawbytes.push(BroadcastRecvInfo {
                            packets,
                            data,
                            extra,
                            recv_from: src_key_id.clone(),
                        });
                        received = true;
                    }
                    Ok(None) => continue,
                }
                break;
            }
            if received {
                if let Some(transfer) = overlay.owned_broadcasts.get(&bcast_id_recv) {
                    if let Some(transfer) = Self::unwrap_transfer(transfer.val()) {
                        transfer.completed.store(true, Ordering::Relaxed);
                        #[cfg(feature = "telemetry")]
                        {
                            transfer.telemetry.flags.fetch_or(flags, Ordering::Relaxed);
                            transfer.telemetry.len.store(len, Ordering::Relaxed);
                            transfer.telemetry.tag.store(tag, Ordering::Relaxed);
                        }
                        log::debug!(
                            target: TARGET,
                            "Recv {bcast_type} broadcast {} with tag {tag:08x} \
                            in overlay {} from {src_key_id}",
                            base64_encode(&bcast_id_recv),
                            overlay.overlay_id
                        )
                    } else {
                        log::error!(
                            target: TARGET,
                            "INTERNAL ERROR: recv {bcast_type} broadcast {} mismatch in overlay {}",
                            base64_encode(&bcast_id_recv),
                            overlay.overlay_id
                        )
                    }
                }
            }
            // Graceful close
            reader.close();
            while reader.recv().await.is_some() {}
        });
        let timeout_sec = Self::calc_broadcast_timeout(
            #[cfg(feature = "xp25")]
            bcast.flags(),
        );
        let bcast_id_wait = *bcast_id;
        let bcast_type = Self::broadcast_type();
        let data_size = bcast.data_size();
        #[cfg(feature = "xp25")]
        let flags = bcast.flags();
        let overlay = ctx.overlay.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(timeout_sec * 100)).await;
                if let Some(transfer) = overlay.owned_broadcasts.get(&bcast_id_wait) {
                    if let Some(transfer) = Self::unwrap_transfer(transfer.val()) {
                        if !transfer.updated_at.is_expired(timeout_sec) {
                            continue;
                        }
                        if !transfer.completed.load(Ordering::Relaxed) {
                            log::warn!(
                                target: TARGET,
                                "{bcast_type} broadcast {} ({data_size} bytes) \
                                dropped incompleted by timeout in overlay {}",
                                base64_encode(&bcast_id_wait),
                                overlay.overlay_id
                            )
                        }
                        // Abort receiving loop
                        transfer.sender.send(None).ok();
                    } else {
                        log::error!(
                            target: TARGET,
                            "INTERNAL ERROR: recv {bcast_type} broadcast {} mismatch in overlay {}",
                            base64_encode(&bcast_id_wait),
                            overlay.overlay_id
                        )
                    }
                }
                break;
            }
            if let Err(e) = overlay.setup_broadcast_purge(
                bcast_id_wait,
                #[cfg(feature = "xp25")]
                flags,
            ) {
                log::warn!(
                    target: TARGET,
                    "Cannot setup {bcast_type} broadcast {} purge after recv: {e}",
                    base64_encode(&bcast_id_wait),
                )
            }
        });
        let transfer = RecvTransferFec {
            completed: AtomicBool::new(false),
            data_hash: *bcast.data_hash(),
            history: PeerHistory::for_recv(),
            sender,
            src_key_id: src_key.id().clone(),
            #[cfg(feature = "telemetry")]
            telemetry: RecvTransferFecTelemetry {
                flags: AtomicU32::new(0),
                len: AtomicU32::new(0),
                tag: AtomicU32::new(0),
            },
            updated_at: UpdatedAt::new(),
            counter: ctx.overlay.allocated.recv_transfers.clone().into(),
        };
        #[cfg(feature = "telemetry")]
        ctx.overlay
            .telemetry
            .recv_transfers
            .update(ctx.overlay.allocated.recv_transfers.load(Ordering::Relaxed));
        Ok(Self::wrap_transfer(transfer))
    }

    fn prepare_part(overlay: &Arc<Overlay>, transfer: &mut SendTransferFec) -> Result<Vec<u8>> {
        let seqno_original = transfer.seqno;
        let chunk = transfer.encoder.encode(&mut transfer.seqno)?;
        let to_sign = Self::calc_to_sign(
            &transfer.bcast_id,
            transfer.encoder.params().data_size as usize,
            &chunk,
            transfer.seqno,
            transfer.date,
        )?;
        let signature = transfer.src_key.sign(&to_sign)?;
        let bcast = Self::build_broadcast_part(chunk, transfer, signature)?;
        if seqno_original == transfer.seqno {
            transfer.seqno += 1;
        }
        overlay.serialize_broadcast(&bcast)
    }

    async fn process_broadcast(
        bcast: T,
        ctx: &BroadcastRecvContext,
        bcast_id: &BroadcastId,
    ) -> Result<(Option<BroadcastRecvInfo>, bool)> {
        #[cfg(feature = "telemetry")]
        log::info!(
            target: TARGET_BROADCAST,
            "Broadcast FEC trace: recv {} part {}, hops {:?} to overlay {}",
            base64_encode(&bcast_id),
            bcast.seqno(),
            ctx.hops,
            ctx.overlay.overlay_id
        );
        #[cfg(feature = "telemetry")]
        let stats = ctx.overlay.get_per_transfer_stats(bcast_id)?;
        #[cfg(feature = "telemetry")]
        stats.income.fetch_add(1, Ordering::Relaxed);
        let bcast_type = Self::broadcast_type();
        let transfer = loop {
            if let Some(transfer) = ctx.overlay.owned_broadcasts.get(bcast_id) {
                break transfer;
            }
            if !add_unbound_object_to_map(&ctx.overlay.owned_broadcasts, *bcast_id, || {
                Ok(OwnedBroadcast::WillBeRecv(bcast_type.clone()))
            })? {
                tokio::task::yield_now().await;
                continue;
            }
            #[cfg(feature = "xp25")]
            if (bcast.flags() & OverlayNode::FLAG_BCAST_REPEATED) != 0 {
                log::debug!(
                    target: TARGET,
                    "Receiving repeated {bcast_type} broadcast {}",
                    base64_encode(bcast_id)
                );
            }
            let transfer = Self::create_transfer(ctx, bcast_id, &bcast);
            if transfer.is_err() {
                ctx.overlay.owned_broadcasts.remove(bcast_id);
            }
            let ok = match ctx.overlay.owned_broadcasts.insert(*bcast_id, transfer?) {
                Some(removed) => matches!(
                    removed.val(),
                    OwnedBroadcast::WillBeRecv(x) if x == &bcast_type
                ),
                _ => false,
            };
            if !ok {
                log::error!(
                    target: TARGET,
                    "INTERNAL ERROR: recv {bcast_type} broadcast {} create mismatch in overlay {}",
                    base64_encode(bcast_id),
                    ctx.overlay.overlay_id
                )
            }
        };
        let Some(transfer) = Self::unwrap_transfer(transfer.val()) else {
            // Not a proper broadcast
            return Ok((None, false));
        };
        transfer.updated_at.refresh();
        if &transfer.data_hash != bcast.data_hash() {
            log::warn!(
                target: TARGET,
                "Same broadcast ID {} but different hash: {} vs {} (src {} vs {}) in overlay {}",
                base64_encode(bcast_id),
                base64_encode(transfer.data_hash),
                base64_encode(bcast.data_hash()),
                transfer.src_key_id,
                bcast.src_key()?.id(),
                ctx.overlay.overlay_id
            );
            return Ok((None, false));
        }
        if !transfer.history.update(bcast.seqno() as u64, TARGET_BROADCAST).await? {
            log::debug!(
                target: TARGET,
                "Broadcast {} part {} dropped by history filter in overlay {}",
                base64_encode(bcast_id),
                bcast.seqno(),
                ctx.overlay.overlay_id
            );
            return Ok((None, false));
        }
        if !transfer.completed.load(Ordering::Relaxed) {
            transfer
                .sender
                .send(Some(bcast))
                .map_err(|e| error!("Error sending broadcast packet to processing: {e}"))?;
        }
        #[cfg(feature = "telemetry")]
        stats.passed.fetch_add(1, Ordering::Relaxed);
        Ok((None, true))
    }

    fn process_part(
        decoder: &mut RaptorqDecoder,
        bcast_id: &BroadcastId,
        bcast: &T,
    ) -> Result<Option<Vec<u8>>> {
        let bcast_type = Self::broadcast_type();
        let to_sign = Self::calc_to_sign(
            bcast_id,
            bcast.data_size(),
            bcast.part_data(),
            bcast.seqno(),
            bcast.date(),
        )?;
        bcast.src_key()?.verify(&to_sign, bcast.signature())?;
        if let Some(ret) = decoder.decode(bcast.seqno(), bcast.part_data()) {
            let ret = if ret.len() != bcast.data_size() as usize {
                fail!("Expected {} bytes, but received {}", bcast.data_size(), ret.len())
            } else {
                let test_hash = sha256_digest(&ret);
                if &test_hash != bcast.data_hash() {
                    fail!(
                        "Expected {bcast_type} broadcast {} data hash, but received {}",
                        base64_encode(bcast.data_hash()),
                        base64_encode(test_hash),
                    )
                }
                let delay = Version::get() - bcast.date();
                if delay > 1 {
                    log::warn!(
                        target: TARGET,
                        "Received overlay {bcast_type} broadcast {} ({} bytes) in {delay} seconds",
                        base64_encode(bcast_id),
                        ret.len()
                    )
                } else {
                    log::trace!(
                        target: TARGET,
                        "Received overlay {bcast_type} broadcast {} ({} bytes) in {delay} seconds",
                        base64_encode(bcast_id),
                        ret.len()
                    )
                }
                ret
            };
            Ok(Some(ret))
        } else {
            Ok(None)
        }
    }
}

trait BroadcastMultistep {
    fn calc_broadcast_neighbours(
        overlay: &Arc<Overlay>,
        hops: Option<u8>,
        neighbours: u8,
    ) -> Result<BroadcastNeighbours> {
        let (hops, expected_count) = overlay.calc_broadcast_neighbours(hops, neighbours)?;
        Ok(BroadcastNeighbours { expected_count, hops, ids: None })
    }

    fn select_broadcast_neighbours(
        overlay: &Arc<Overlay>,
        neighbours: &mut BroadcastNeighbours,
        skip: Option<&Arc<KeyId>>,
    ) {
        if neighbours.ids.is_some() {
            neighbours.skip(skip);
        } else {
            neighbours.ids =
                Some(overlay.select_broadcast_neighbours(neighbours.expected_count, skip));
        }
    }
}

trait BroadcastTwostep {
    fn calc_broadcast_id(
        data_hash: [u8; 32],
        date: i32,
        data_size: usize,
        part_size: usize,
        src_key: &Arc<dyn KeyOption>,
        src_adnl_key_id: &Arc<KeyId>,
        flags: u32,
        extra: &[u8],
    ) -> Result<BroadcastId> {
        let bcast_id = BroadcastTwostepId {
            date,
            flags: flags as i32,
            src: UInt256::from_slice(src_key.id().data()),
            src_adnl_id: UInt256::from_slice(src_adnl_key_id.data()),
            data_hash: UInt256::with_array(data_hash),
            data_size: data_size as i32,
            part_size: part_size as i32,
            extra: extra.to_vec(),
        };
        hash(bcast_id)
    }

    fn calc_broadcast_id_when_send(
        ctx: &BroadcastSendContext,
        date: i32,
        data_size: usize,
        part_size: usize,
        extra: &[u8],
    ) -> Result<(BroadcastId, bool)> {
        let bcast_id = Self::calc_broadcast_id(
            sha256_digest(ctx.data.object),
            date,
            data_size,
            part_size,
            &ctx.src_key,
            &ctx.src_adnl_key_id,
            ctx.flags,
            extra,
        )?;
        Ok((bcast_id, true))
    }

    fn calc_broadcast_neighbours(overlay: &Arc<Overlay>) -> BroadcastNeighbours {
        let neighbours = overlay.select_broadcast_twostep_neighbours(None);
        BroadcastNeighbours {
            hops: None,
            expected_count: neighbours.len() as u32,
            ids: Some(neighbours),
        }
    }

    fn select_broadcast_neighbours(
        neighbours: &mut BroadcastNeighbours,
        skip: Option<&Arc<KeyId>>,
    ) {
        neighbours.skip(skip);
    }
}

// Broadcast FEC **************************************************************

#[rustfmt::skip]
impl BroadcastParsed for BroadcastFec {
    fn date(&self) -> i32 { self.date }
    fn src_key(&self) -> Result<Arc<dyn KeyOption>> { (&self.src).try_into() }
    #[cfg(feature = "telemetry")]
    fn default_tag(&self) -> u32 { BroadcastFec::constructor_const() }
    #[cfg(feature = "xp25")]
    fn flags(&self) -> u32 { self.flags as u32 }
}

#[rustfmt::skip]
impl FecBroadcastParsed for BroadcastFec {
    fn data_hash(&self) -> &[u8; 32] { self.data_hash.as_slice() }
    fn data_size(&self) -> usize { self.data_size as usize }
    fn part_data(&self) -> &[u8] { &self.data }
    fn seqno(&self) -> u32 { self.seqno as u32 }
    fn signature(&self) -> &[u8] { &self.signature }

    fn fec_type(&self) -> Option<FecTypeRaptorQ> {
        if let FecType::Fec_RaptorQ(fec_type) = &self.fec {
            Some(fec_type.clone())
        } else {
            None
        }
    }
}

pub(crate) struct BroadcastFecProtocol {
    encoder: Option<RaptorqEncoder>,
}

impl BroadcastFecProtocol {
    const SIZE_WAVE: u32 = 4;
    const TIMEOUT_WAVE_MS: u64 = 3;

    pub(crate) fn for_recv() -> Self {
        Self { encoder: None }
    }

    pub(crate) fn for_send(ctx: &BroadcastSendContext) -> Self {
        Self { encoder: Some(RaptorqEncoder::with_data(ctx.data.object, None)) }
    }

    fn calc_broadcast_id(
        data_hash: [u8; 32],
        data_size: usize,
        src_key: &Arc<dyn KeyOption>,
        flags: u32,
        fec_type: FecTypeRaptorQ,
    ) -> Result<BroadcastId> {
        let bcast_id = BroadcastFecId {
            src: if ((flags as u32) & OverlayNode::FLAG_BCAST_ANY_SENDER) != 0 {
                UInt256::ZERO
            } else {
                UInt256::from_slice(src_key.id().data())
            },
            type_: UInt256::with_array(hash(fec_type)?),
            data_hash: UInt256::with_array(data_hash),
            size: data_size as i32,
            flags: flags as i32,
        };
        hash(bcast_id)
    }
}

impl BroadcastMultistep for BroadcastFecProtocol {}

#[async_trait::async_trait]
impl BroadcastProtocol<BroadcastFec> for BroadcastFecProtocol {
    // Common

    fn broadcast_type() -> BroadcastType {
        BroadcastType::Fec
    }

    fn calc_broadcast_neighbours(
        &self,
        overlay: &Arc<Overlay>,
        hops: Option<u8>,
    ) -> Result<BroadcastNeighbours> {
        <Self as BroadcastMultistep>::calc_broadcast_neighbours(
            overlay,
            hops,
            Overlay::SIZE_NEIGHBOURS_LONG_BROADCAST,
        )
    }

    fn select_broadcast_neighbours(
        &self,
        overlay: &Arc<Overlay>,
        neighbours: &mut BroadcastNeighbours,
        skip: Option<&Arc<KeyId>>,
    ) {
        <Self as BroadcastMultistep>::select_broadcast_neighbours(overlay, neighbours, skip);
    }

    fn send_method(&self) -> BroadcastSendMethod {
        BroadcastSendMethod::Fast
    }

    // Receive side

    fn check_broadcast(
        &self,
        bcast: &BroadcastFec,
        _ctx: &BroadcastRecvContext,
    ) -> Result<BroadcastCheckInfo> {
        let fec_type = if let FecType::Fec_RaptorQ(fec_type) = &bcast.fec {
            fec_type.clone()
        } else {
            fail!("Unsupported FEC type {} broadcast", Self::broadcast_type())
        };
        Ok(BroadcastCheckInfo {
            bcast_id: Self::calc_broadcast_id(
                *bcast.data_hash.as_slice(),
                bcast.data_size as usize,
                &bcast.src_key()?,
                bcast.flags as u32,
                fec_type,
            )?,
            dup: false,
            data_len: bcast.data.len(),
            seqno: bcast.seqno(),
            #[cfg(feature = "telemetry")]
            maybe_tag: None,
        })
    }

    async fn process_broadcast(
        &self,
        bcast: BroadcastFec,
        ctx: &mut BroadcastRecvContext,
        bcast_id: &BroadcastId,
    ) -> Result<(Option<BroadcastRecvInfo>, bool)> {
        <Self as FecProtocol<BroadcastFec>>::process_broadcast(bcast, ctx, bcast_id).await
    }

    // Send side

    fn calc_broadcast_id(
        &self,
        ctx: &BroadcastSendContext,
        _date: i32,
    ) -> Result<(BroadcastId, bool)> {
        let fec_type = if let Some(encoder) = &self.encoder {
            encoder.params().clone()
        } else {
            fail!("INTERNAL ERROR: no RaptorQ encoder set in FEC broadcast");
        };
        let bcast_id = Self::calc_broadcast_id(
            sha256_digest(ctx.data.object),
            ctx.data.object.len(),
            &ctx.src_key,
            ctx.flags,
            fec_type,
        )?;
        Ok((bcast_id, false))
    }

    fn build_broadcast(
        &mut self,
        ctx: &BroadcastSendContext,
        bcast_id: &BroadcastId,
        date: i32,
        neighbours: &mut BroadcastNeighbours,
    ) -> Result<BroadcastJob> {
        #[cfg(feature = "telemetry")]
        let tag = ctx.data.tag;
        let data_size = ctx.data.object.len() as u32;
        let Some(encoder) = self.encoder.take() else {
            fail!("INTERNAL ERROR: no RaptorQ encoder set in FEC broadcast");
        };
        let mut transfer = SendTransferFec {
            bcast_id: *bcast_id,
            data_hash: sha256_digest(ctx.data.object),
            date,
            encoder,
            extra: Vec::new(),
            flags: ctx.flags,
            seqno: 0,
            src_key: ctx.src_key.clone(),
            src_adnl_key_id: ctx.src_adnl_key_id.clone(),
            counter: ctx.overlay.allocated.send_transfers.clone().into(),
        };
        #[cfg(feature = "telemetry")]
        ctx.overlay
            .telemetry
            .send_transfers
            .update(ctx.overlay.allocated.send_transfers.load(Ordering::Relaxed));
        let bcast_type = Self::broadcast_type();
        let max_seqno = (data_size / transfer.encoder.params().symbol_size as u32 + 1) * 3 / 2;
        let overlay = ctx.overlay.clone();
        let (sender, mut reader) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while transfer.seqno <= max_seqno {
                for _ in 0..Self::SIZE_WAVE {
                    let seqno = transfer.seqno;
                    let result = Self::prepare_part(&overlay, &mut transfer).and_then(|data| {
                        sender.send((data, seqno))?;
                        Ok(())
                    });
                    if let Err(e) = result {
                        log::warn!(
                            target: TARGET,
                            "Error when sending overlay {} {bcast_type} broadcast: {e}",
                            overlay.overlay_id
                        );
                        return;
                    }
                    if transfer.seqno > max_seqno {
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(Self::TIMEOUT_WAVE_MS)).await;
            }
        });
        let bcast_id = *bcast_id;
        let bcast_type = Self::broadcast_type();
        #[cfg(feature = "xp25")]
        let flags = ctx.flags;
        let hops = neighbours.hops;
        let neighbours_count = neighbours.expected_count;
        self.select_broadcast_neighbours(ctx.overlay, neighbours, None);
        let mut neighbours =
            if let Some(ids) = neighbours.get_ids() { ids.clone() } else { Vec::new() };
        let overlay = ctx.overlay.clone();
        let send_method = self.send_method();
        let src_adnl_key_id = ctx.src_adnl_key_id.clone();
        tokio::spawn(async move {
            while let Some((mut buf, seqno)) = reader.recv().await {
                if let Some(hops) = &hops {
                    buf.push(*hops)
                }
                if let Err(e) = Self::distribute_broadcast(
                    &overlay,
                    &buf,
                    &bcast_id,
                    seqno,
                    &neighbours,
                    &src_adnl_key_id,
                    &send_method,
                    #[cfg(feature = "telemetry")]
                    tag,
                )
                .await
                {
                    log::warn!(
                        target: TARGET,
                        "Error when sending overlay {} {bcast_type} broadcast: {e}",
                        overlay.overlay_id
                    );
                }
                neighbours = overlay.select_broadcast_neighbours(neighbours_count, None);
            }
            // Graceful close
            reader.close();
            while reader.recv().await.is_some() {}
            if let Err(e) = overlay.setup_broadcast_purge(
                bcast_id,
                #[cfg(feature = "xp25")]
                flags,
            ) {
                log::warn!(
                    target: TARGET,
                    "Cannot setup {bcast_type} broadcast {} purge after send: {e}",
                    base64_encode(&bcast_id)
                )
            }
        });
        Ok(BroadcastJob::Background(max_seqno))
    }
}

impl FecProtocol<BroadcastFec> for BroadcastFecProtocol {
    fn build_broadcast_part(
        data: Vec<u8>,
        transfer: &SendTransferFec,
        signature: Vec<u8>,
    ) -> Result<Broadcast> {
        Ok(BroadcastFec {
            src: (&transfer.src_key).try_into()?,
            certificate: OverlayCertificate::Overlay_EmptyCertificate,
            data_hash: UInt256::with_array(transfer.data_hash),
            data_size: transfer.encoder.params().data_size,
            flags: transfer.flags as i32,
            data,
            seqno: transfer.seqno as i32,
            fec: transfer.encoder.params().clone().into_boxed(),
            date: transfer.date,
            signature,
        }
        .into_boxed())
    }

    fn calc_to_sign(
        bcast_id: &BroadcastId,
        _data_size: usize,
        part_data: &[u8],
        seqno: u32,
        date: i32,
    ) -> Result<Vec<u8>> {
        let part_data_hash: [u8; 32] = sha256_digest(part_data);
        let part_id = BroadcastFecPartId {
            broadcast_hash: UInt256::from_slice(bcast_id),
            data_hash: UInt256::with_array(part_data_hash),
            seqno: seqno as i32,
        };
        let part_hash = hash(part_id)?;
        let to_sign = BroadcastToSign { hash: UInt256::with_array(part_hash), date }.into_boxed();
        serialize_boxed(&to_sign)
    }

    fn unwrap_transfer(wrapped: &OwnedBroadcast) -> Option<&RecvTransferFec<BroadcastFec>> {
        match wrapped {
            OwnedBroadcast::RecvFec(transfer) => Some(transfer),
            _ => None,
        }
    }

    fn wrap_transfer(transfer: RecvTransferFec<BroadcastFec>) -> OwnedBroadcast {
        OwnedBroadcast::RecvFec(transfer)
    }
}

// Broadcast simple ***********************************************************

#[rustfmt::skip]
impl BroadcastParsed for BroadcastSimple {
    fn date(&self) -> i32 { self.date }
    fn src_key(&self) -> Result<Arc<dyn KeyOption>> { (&self.src).try_into() }
    #[cfg(feature = "telemetry")]
    fn default_tag(&self) -> u32 { BroadcastSimple::constructor_const() }
    #[cfg(feature = "xp25")]
    fn flags(&self) -> u32 { self.flags as u32 }
}

pub(crate) struct BroadcastSimpleProtocol;

impl BroadcastSimpleProtocol {
    fn build_broadcast(
        ctx: &BroadcastSendContext,
        bcast_id: &BroadcastId,
        date: i32,
    ) -> Result<BroadcastSimple> {
        let to_sign = Self::calc_to_sign(bcast_id, date)?;
        let signature = ctx.src_key.sign(&to_sign)?;
        Ok(BroadcastSimple {
            src: ctx.src_key.try_into()?,
            certificate: OverlayCertificate::Overlay_EmptyCertificate,
            flags: ctx.flags as i32,
            data: ctx.data.object.to_vec(),
            date,
            signature,
        })
    }

    fn calc_broadcast_id(
        data: &[u8],
        src_key: &Arc<dyn KeyOption>,
        flags: u32,
    ) -> Result<BroadcastId> {
        let data_hash = sha256_digest(data);
        let bcast_id = BroadcastSimpleId {
            src: if (flags & OverlayNode::FLAG_BCAST_ANY_SENDER) != 0 {
                UInt256::ZERO
            } else {
                UInt256::from_slice(src_key.id().data())
            },
            data_hash: UInt256::with_array(data_hash),
            flags: flags as i32,
        };
        hash(bcast_id)
    }

    fn calc_to_sign(bcast_id: &BroadcastId, date: i32) -> Result<Vec<u8>> {
        let to_sign = BroadcastToSign { hash: UInt256::from(bcast_id), date }.into_boxed();
        serialize_boxed(&to_sign)
    }

    fn check_broadcast(
        bcast: &BroadcastSimple,
        ctx: &BroadcastRecvContext,
    ) -> Result<BroadcastCheckInfo> {
        let src_key: Arc<dyn KeyOption> = (&bcast.src).try_into()?;
        let bcast_id = Self::calc_broadcast_id(&bcast.data, &src_key, bcast.flags as u32)?;
        let dup = if add_unbound_object_to_map(&ctx.overlay.owned_broadcasts, bcast_id, || {
            Ok(OwnedBroadcast::Send)
        })? {
            let to_sign = Self::calc_to_sign(&bcast_id, bcast.date)?;
            src_key.verify(&to_sign, &bcast.signature)?;
            false
        } else {
            true
        };
        let data_len = bcast.data.len();
        Ok(BroadcastCheckInfo {
            bcast_id,
            dup,
            data_len,
            seqno: 0,
            #[cfg(feature = "telemetry")]
            maybe_tag: if data_len >= 4 {
                let data = &bcast.data;
                Some(u32::from_le_bytes([data[0], data[1], data[2], data[3]]))
            } else {
                None
            },
        })
    }

    fn process_broadcast(bcast: BroadcastSimple) -> Result<(Option<BroadcastRecvInfo>, bool)> {
        let src_key: Arc<dyn KeyOption> = (&bcast.src).try_into()?;
        let info = BroadcastRecvInfo {
            packets: 1,
            data: bcast.data.into(),
            extra: None,
            recv_from: src_key.id().clone(),
        };
        Ok((Some(info), true))
    }
}

impl BroadcastMultistep for BroadcastSimpleProtocol {}

#[async_trait::async_trait]
impl BroadcastProtocol<BroadcastSimple> for BroadcastSimpleProtocol {
    // Common

    fn broadcast_type() -> BroadcastType {
        BroadcastType::Simple
    }

    fn calc_broadcast_neighbours(
        &self,
        overlay: &Arc<Overlay>,
        hops: Option<u8>,
    ) -> Result<BroadcastNeighbours> {
        <Self as BroadcastMultistep>::calc_broadcast_neighbours(
            overlay,
            hops,
            Overlay::SIZE_NEIGHBOURS_SHORT_BROADCAST,
        )
    }

    fn select_broadcast_neighbours(
        &self,
        overlay: &Arc<Overlay>,
        neighbours: &mut BroadcastNeighbours,
        skip: Option<&Arc<KeyId>>,
    ) {
        <Self as BroadcastMultistep>::select_broadcast_neighbours(overlay, neighbours, skip);
    }

    fn send_method(&self) -> BroadcastSendMethod {
        BroadcastSendMethod::Fast
    }

    // Receive side

    fn check_broadcast(
        &self,
        bcast: &BroadcastSimple,
        ctx: &BroadcastRecvContext,
    ) -> Result<BroadcastCheckInfo> {
        Self::check_broadcast(bcast, ctx)
    }

    async fn process_broadcast(
        &self,
        bcast: BroadcastSimple,
        _ctx: &mut BroadcastRecvContext,
        _bcast_id: &BroadcastId,
    ) -> Result<(Option<BroadcastRecvInfo>, bool)> {
        Self::process_broadcast(bcast)
    }

    // Send side

    fn calc_broadcast_id(
        &self,
        ctx: &BroadcastSendContext,
        _date: i32,
    ) -> Result<(BroadcastId, bool)> {
        Ok((Self::calc_broadcast_id(ctx.data.object, &ctx.src_key, ctx.flags)?, false))
    }

    fn build_broadcast(
        &mut self,
        ctx: &BroadcastSendContext,
        bcast_id: &BroadcastId,
        date: i32,
        _neighbours: &mut BroadcastNeighbours,
    ) -> Result<BroadcastJob> {
        Ok(BroadcastJob::Foreground(Self::build_broadcast(ctx, bcast_id, date)?.into_boxed()))
    }
}

// Broadcast stream simple ****************************************************

#[rustfmt::skip]
impl BroadcastParsed for BroadcastStream {
    fn date(&self) -> i32 { self.data.date }
    fn src_key(&self) -> Result<Arc<dyn KeyOption>> { (&self.data.src).try_into() }
    #[cfg(feature = "telemetry")]
    fn default_tag(&self) -> u32 { BroadcastStream::constructor_const() }
    #[cfg(feature = "xp25")]
    fn flags(&self) -> u32 { self.data.flags as u32 }
}

pub(crate) struct BroadcastStreamSimpleProtocol;

impl BroadcastMultistep for BroadcastStreamSimpleProtocol {}

#[async_trait::async_trait]
impl BroadcastProtocol<BroadcastStream> for BroadcastStreamSimpleProtocol {
    // Common

    fn broadcast_type() -> BroadcastType {
        BroadcastType::StreamSimple
    }

    fn calc_broadcast_neighbours(
        &self,
        overlay: &Arc<Overlay>,
        hops: Option<u8>,
    ) -> Result<BroadcastNeighbours> {
        <Self as BroadcastMultistep>::calc_broadcast_neighbours(
            overlay,
            hops,
            Overlay::SIZE_NEIGHBOURS_STREAM_BROADCAST,
        )
    }

    fn select_broadcast_neighbours(
        &self,
        overlay: &Arc<Overlay>,
        neighbours: &mut BroadcastNeighbours,
        skip: Option<&Arc<KeyId>>,
    ) {
        <Self as BroadcastMultistep>::select_broadcast_neighbours(overlay, neighbours, skip);
    }

    fn send_method(&self) -> BroadcastSendMethod {
        BroadcastSendMethod::Safe
    }

    // Receive side

    fn check_broadcast(
        &self,
        bcast: &BroadcastStream,
        ctx: &BroadcastRecvContext,
    ) -> Result<BroadcastCheckInfo> {
        BroadcastSimpleProtocol::check_broadcast(&bcast.data, ctx)
    }

    fn build_rebroadcast(
        &self,
        ctx: &mut BroadcastRecvContext,
        neighbours: &mut BroadcastNeighbours,
    ) -> Result<()> {
        let BroadcastData::Stream(bcast) = &mut ctx.data else {
            fail!(
                "INTERNAL ERROR: unexpected broadcast data type for {} rebroadcast",
                Self::broadcast_type()
            );
        };
        let Some(mut bcast) = bcast.take() else {
            fail!("INTERNAL ERROR: no stream data for rebroadcast");
        };
        self.select_broadcast_neighbours(ctx.overlay, neighbours, Some(ctx.peers.other()));
        let hops = neighbours.hops;
        let Some(neighbours) = neighbours.get_ids() else {
            ctx.data = BroadcastData::None;
            return Ok(());
        };
        neighbours.retain(|n| !bcast.trace.iter().any(|k| k.as_slice() == n.data()));
        for neighbour in neighbours.iter() {
            bcast.trace.push(UInt256::from_slice(neighbour.data()));
        }
        let mut buf = ctx.overlay.serialize_broadcast(&bcast.into_boxed())?;
        if let Some(hops) = hops {
            buf.push(hops);
        }
        ctx.data = BroadcastData::Buf(buf);
        Ok(())
    }

    async fn process_broadcast(
        &self,
        bcast: BroadcastStream,
        ctx: &mut BroadcastRecvContext,
        _bcast_id: &BroadcastId,
    ) -> Result<(Option<BroadcastRecvInfo>, bool)> {
        let ret = BroadcastSimpleProtocol::process_broadcast(bcast.data.clone())?;
        ctx.data = BroadcastData::Stream(Some(bcast));
        Ok(ret)
    }

    // Send side

    fn calc_broadcast_id(
        &self,
        ctx: &BroadcastSendContext,
        _date: i32,
    ) -> Result<(BroadcastId, bool)> {
        let bcast_id =
            BroadcastSimpleProtocol::calc_broadcast_id(ctx.data.object, &ctx.src_key, ctx.flags)?;
        Ok((bcast_id, false))
    }

    fn build_broadcast(
        &mut self,
        ctx: &BroadcastSendContext,
        bcast_id: &BroadcastId,
        date: i32,
        neighbours: &mut BroadcastNeighbours,
    ) -> Result<BroadcastJob> {
        let bcast = BroadcastSimpleProtocol::build_broadcast(ctx, bcast_id, date)?;
        self.select_broadcast_neighbours(ctx.overlay, neighbours, None);
        let trace = if let Some(ids) = neighbours.get_ids() {
            ids.iter().map(|k| UInt256::from_slice(k.data())).collect()
        } else {
            Vec::new()
        };
        Ok(BroadcastJob::Foreground(BroadcastStream { data: bcast, trace }.into_boxed()))
    }
}

// Broadcast two-step FEC *****************************************************

#[rustfmt::skip]
impl BroadcastParsed for BroadcastTwostepFec {
    fn date(&self) -> i32 { self.date }
    fn src_key(&self) -> Result<Arc<dyn KeyOption>> { (&self.src).try_into() }
    #[cfg(feature = "telemetry")]
    fn default_tag(&self) -> u32 { BroadcastTwostepFec::constructor_const() }
    #[cfg(feature = "xp25")]
    fn flags(&self) -> u32 { self.flags as u32 }
}

#[rustfmt::skip]
impl FecBroadcastParsed for BroadcastTwostepFec {
    fn data_hash(&self) -> &[u8; 32] { self.data_hash.as_slice() }
    fn data_size(&self) -> usize { self.data_size as usize }
    fn fec_type(&self) -> Option<FecTypeRaptorQ> {
        if self.part.is_empty() {
            return None;
        }
        let symbol_size = self.part.len() as i32;
        Some(FecTypeRaptorQ {
            data_size: self.data_size,
            symbols_count: (self.data_size + symbol_size - 1) / symbol_size,
            symbol_size,
        })
    }
    fn extra(&self) -> Option<&[u8]> { Some(&self.extra) }
    fn part_data(&self) -> &[u8] { &self.part }
    fn seqno(&self) -> u32 { self.seqno as u32 }
    fn signature(&self) -> &[u8] { &self.signature }
}

struct BroadcastTwostepSendContext {
    neighbours: u32,
    part_size: usize,
}

pub(crate) struct BroadcastTwostepFecProtocol {
    extra: Option<Vec<u8>>,
    send_ctx: Option<BroadcastTwostepSendContext>,
}

impl BroadcastTwostepFecProtocol {
    const MAX_PART_SIZE: usize = 65536;

    pub(crate) fn for_recv() -> Self {
        Self { extra: None, send_ctx: None }
    }

    pub(crate) fn for_send(data: &[u8], neighbours: u32, extra: Vec<u8>) -> Result<Self> {
        if neighbours <= 3 {
            fail!("Not enough neighbours to build {} broadcast", Self::broadcast_type());
        }
        if data.is_empty() {
            fail!("Empty payload for {} broadcast", Self::broadcast_type());
        }
        let k = ((neighbours as usize) * 2 - 2) / 3;
        let part_size = (data.len() + k - 1) / k;
        if part_size >= Self::MAX_PART_SIZE {
            fail!("Too big part size {part_size} in {} broadcast", Self::broadcast_type());
        }
        let ctx = BroadcastTwostepSendContext { neighbours, part_size };
        Ok(Self { extra: Some(extra), send_ctx: Some(ctx) })
    }
}

impl BroadcastTwostep for BroadcastTwostepFecProtocol {}

#[async_trait::async_trait]
impl BroadcastProtocol<BroadcastTwostepFec> for BroadcastTwostepFecProtocol {
    // Common

    fn broadcast_type() -> BroadcastType {
        BroadcastType::TwostepFec
    }

    fn calc_broadcast_neighbours(
        &self,
        overlay: &Arc<Overlay>,
        _hops: Option<u8>,
    ) -> Result<BroadcastNeighbours> {
        Ok(<Self as BroadcastTwostep>::calc_broadcast_neighbours(overlay))
    }

    fn select_broadcast_neighbours(
        &self,
        _overlay: &Arc<Overlay>,
        neighbours: &mut BroadcastNeighbours,
        skip: Option<&Arc<KeyId>>,
    ) {
        <Self as BroadcastTwostep>::select_broadcast_neighbours(neighbours, skip);
    }

    fn send_method(&self) -> BroadcastSendMethod {
        BroadcastSendMethod::QuicOrRldp
    }

    // Receive side

    fn check_broadcast(
        &self,
        bcast: &BroadcastTwostepFec,
        _ctx: &BroadcastRecvContext,
    ) -> Result<BroadcastCheckInfo> {
        let src_key: Arc<dyn KeyOption> = (&bcast.src).try_into()?;
        let src_adnl_key_id = KeyId::from_data(*bcast.src_adnl_id.as_slice());
        Ok(BroadcastCheckInfo {
            bcast_id: <Self as BroadcastTwostep>::calc_broadcast_id(
                *bcast.data_hash.as_slice(),
                bcast.date,
                bcast.data_size as usize,
                bcast.part.len(),
                &src_key,
                &src_adnl_key_id,
                bcast.flags as u32,
                &bcast.extra,
            )?,
            dup: false,
            data_len: bcast.part.len(),
            seqno: bcast.seqno(),
            #[cfg(feature = "telemetry")]
            maybe_tag: None,
        })
    }

    async fn process_broadcast(
        &self,
        bcast: BroadcastTwostepFec,
        ctx: &mut BroadcastRecvContext,
        bcast_id: &BroadcastId,
    ) -> Result<(Option<BroadcastRecvInfo>, bool)> {
        <Self as FecProtocol<BroadcastTwostepFec>>::process_broadcast(bcast, ctx, bcast_id).await
    }

    // Send side

    fn calc_broadcast_id(
        &self,
        ctx: &BroadcastSendContext,
        date: i32,
    ) -> Result<(BroadcastId, bool)> {
        let Some(send_ctx) = self.send_ctx.as_ref() else {
            fail!("No send context set for {} broadcast", Self::broadcast_type());
        };
        <Self as BroadcastTwostep>::calc_broadcast_id_when_send(
            ctx,
            date,
            ctx.data.object.len(),
            send_ctx.part_size,
            self.extra.as_deref().unwrap_or_default(),
        )
    }

    fn build_broadcast(
        &mut self,
        ctx: &BroadcastSendContext,
        bcast_id: &BroadcastId,
        date: i32,
        neighbours: &mut BroadcastNeighbours,
    ) -> Result<BroadcastJob> {
        let Some(neighbours) = neighbours.get_ids() else {
            fail!(
                "No neighbours to build {} broadcast {} to",
                Self::broadcast_type(),
                base64_encode(&bcast_id)
            );
        };
        let Some(send_ctx) = self.send_ctx.take() else {
            fail!(
                "Send context is not set for {} broadcast {}",
                Self::broadcast_type(),
                base64_encode(&bcast_id)
            );
        };
        if send_ctx.neighbours != neighbours.len() as u32 {
            fail!(
                "Expected {} neighbours for {} broadcast {}, but got {}",
                send_ctx.neighbours,
                Self::broadcast_type(),
                base64_encode(&bcast_id),
                neighbours.len(),
            );
        }
        let mut transfer = SendTransferFec {
            bcast_id: *bcast_id,
            data_hash: sha256_digest(ctx.data.object),
            date,
            encoder: RaptorqEncoder::with_data(ctx.data.object, Some(send_ctx.part_size as u16)),
            extra: self.extra.take().unwrap_or_default(),
            flags: ctx.flags,
            seqno: 0,
            src_key: ctx.src_key.clone(),
            src_adnl_key_id: ctx.src_adnl_key_id.clone(),
            counter: ctx.overlay.allocated.send_transfers.clone().into(),
        };
        #[cfg(feature = "telemetry")]
        ctx.overlay
            .telemetry
            .send_transfers
            .update(ctx.overlay.allocated.send_transfers.load(Ordering::Relaxed));
        let bcast_id = *bcast_id;
        let neighbours = neighbours.clone();
        let overlay = ctx.overlay.clone();
        let send_method = self.send_method();
        #[cfg(feature = "xp25")]
        let flags = ctx.flags;
        #[cfg(feature = "telemetry")]
        let tag = ctx.data.tag;
        tokio::spawn(async move {
            for i in 0..neighbours.len() {
                let bcast = match Self::prepare_part(&overlay, &mut transfer) {
                    Err(e) => {
                        log::warn!(
                            target: TARGET,
                            "Error when preparing two-step FEC broadcast: {e}"
                        );
                        return;
                    }
                    Ok(part) => part,
                };
                if let Err(e) = Self::distribute_broadcast(
                    &overlay,
                    &bcast,
                    &bcast_id,
                    i as u32,
                    &neighbours[i..i + 1],
                    &transfer.src_adnl_key_id,
                    &send_method,
                    #[cfg(feature = "telemetry")]
                    tag,
                )
                .await
                {
                    log::warn!(
                        target: TARGET,
                        "Error when distributing two-step FEC broadcast to {}: {e}",
                        neighbours[i]
                    );
                }
            }
            if let Err(e) = overlay.setup_broadcast_purge(
                bcast_id,
                #[cfg(feature = "xp25")]
                flags,
            ) {
                log::warn!(
                    target: TARGET,
                    "Cannot setup {} broadcast {} purge after send: {e}",
                    Self::broadcast_type(),
                    base64_encode(&bcast_id)
                )
            }
        });
        Ok(BroadcastJob::Background(1))
    }
}

impl FecProtocol<BroadcastTwostepFec> for BroadcastTwostepFecProtocol {
    fn build_broadcast_part(
        data: Vec<u8>,
        transfer: &SendTransferFec,
        signature: Vec<u8>,
    ) -> Result<Broadcast> {
        Ok(BroadcastTwostepFec {
            date: transfer.date,
            flags: transfer.flags as i32,
            src: (&transfer.src_key).try_into()?,
            src_adnl_id: UInt256::from_slice(transfer.src_adnl_key_id.data()),
            certificate: OverlayCertificate::Overlay_EmptyCertificate,
            data_hash: UInt256::with_array(transfer.data_hash),
            data_size: transfer.encoder.params().data_size as i32,
            seqno: transfer.seqno as i32,
            part: data,
            extra: transfer.extra.clone(),
            signature,
        }
        .into_boxed())
    }

    fn calc_to_sign(
        bcast_id: &BroadcastId,
        _data_size: usize,
        part_data: &[u8],
        seqno: u32,
        _date: i32,
    ) -> Result<Vec<u8>> {
        let to_sign = BroadcastTwostepFecToSign {
            id: UInt256::from_slice(bcast_id),
            seqno: seqno as i32,
            part: part_data.to_vec(),
        };
        serialize_bare(&to_sign)
    }

    fn unwrap_transfer(wrapped: &OwnedBroadcast) -> Option<&RecvTransferFec<BroadcastTwostepFec>> {
        match wrapped {
            OwnedBroadcast::RecvTwostepFec(transfer) => Some(transfer),
            _ => None,
        }
    }

    fn wrap_transfer(transfer: RecvTransferFec<BroadcastTwostepFec>) -> OwnedBroadcast {
        OwnedBroadcast::RecvTwostepFec(transfer)
    }
}

// Broadcast two-step simple **************************************************

#[rustfmt::skip]
impl BroadcastParsed for BroadcastTwostepSimple {
    fn date(&self) -> i32 { self.date }
    fn src_key(&self) -> Result<Arc<dyn KeyOption>> { (&self.src).try_into() }
    #[cfg(feature = "telemetry")]
    fn default_tag(&self) -> u32 { BroadcastTwostepSimple::constructor_const() }
    #[cfg(feature = "xp25")]
    fn flags(&self) -> u32 { self.flags as u32 }
}

pub(crate) struct BroadcastTwostepSimpleProtocol {
    big_data: bool,
    extra: Option<Vec<u8>>,
}

impl BroadcastTwostepSimpleProtocol {
    pub(crate) fn for_recv(big_data: bool) -> Self {
        Self { big_data, extra: None }
    }
    pub(crate) fn for_send(big_data: bool, extra: Vec<u8>) -> Self {
        Self { big_data, extra: Some(extra) }
    }
    fn calc_to_sign(bcast_id: BroadcastId, data: &[u8]) -> Result<Vec<u8>> {
        let to_sign =
            BroadcastTwostepSimpleToSign { id: UInt256::with_array(bcast_id), data: data.to_vec() };
        serialize_bare(&to_sign)
    }
}

impl BroadcastTwostep for BroadcastTwostepSimpleProtocol {}

#[async_trait::async_trait]
impl BroadcastProtocol<BroadcastTwostepSimple> for BroadcastTwostepSimpleProtocol {
    // Common

    fn broadcast_type() -> BroadcastType {
        BroadcastType::TwostepSimple
    }

    fn calc_broadcast_neighbours(
        &self,
        overlay: &Arc<Overlay>,
        _hops: Option<u8>,
    ) -> Result<BroadcastNeighbours> {
        Ok(<Self as BroadcastTwostep>::calc_broadcast_neighbours(overlay))
    }

    fn select_broadcast_neighbours(
        &self,
        _overlay: &Arc<Overlay>,
        neighbours: &mut BroadcastNeighbours,
        skip: Option<&Arc<KeyId>>,
    ) {
        <Self as BroadcastTwostep>::select_broadcast_neighbours(neighbours, skip);
    }

    fn send_method(&self) -> BroadcastSendMethod {
        if self.big_data {
            BroadcastSendMethod::QuicOrRldp
        } else {
            BroadcastSendMethod::Fast
        }
    }

    // Receive side

    fn check_broadcast(
        &self,
        bcast: &BroadcastTwostepSimple,
        ctx: &BroadcastRecvContext,
    ) -> Result<BroadcastCheckInfo> {
        let data_hash = sha256_digest(&bcast.data);
        let data_size = bcast.data.len();
        let src_key: Arc<dyn KeyOption> = (&bcast.src).try_into()?;
        let src_adnl_key_id = KeyId::from_data(*bcast.src_adnl_id.as_slice());
        let bcast_id = <Self as BroadcastTwostep>::calc_broadcast_id(
            data_hash,
            bcast.date,
            data_size,
            data_size,
            &src_key,
            &src_adnl_key_id,
            bcast.flags as u32,
            &bcast.extra,
        )?;
        let dup = if add_unbound_object_to_map(&ctx.overlay.owned_broadcasts, bcast_id, || {
            Ok(OwnedBroadcast::Send)
        })? {
            let to_sign = Self::calc_to_sign(bcast_id, &bcast.data)?;
            src_key.verify(&to_sign, &bcast.signature)?;
            false
        } else {
            true
        };
        let data_len = bcast.data.len();
        Ok(BroadcastCheckInfo {
            bcast_id,
            dup,
            data_len,
            seqno: 0,
            #[cfg(feature = "telemetry")]
            maybe_tag: if data_len >= 4 {
                let data = &bcast.data;
                Some(u32::from_le_bytes([data[0], data[1], data[2], data[3]]))
            } else {
                None
            },
        })
    }

    async fn process_broadcast(
        &self,
        bcast: BroadcastTwostepSimple,
        ctx: &mut BroadcastRecvContext,
        _bcast_id: &BroadcastId,
    ) -> Result<(Option<BroadcastRecvInfo>, bool)> {
        let src_adnl_key_id = KeyId::from_data(*bcast.src_adnl_id.as_slice());
        let resend = ctx.peers.other() == &src_adnl_key_id;
        let info = BroadcastRecvInfo {
            packets: 1,
            data: bcast.data.into(),
            extra: Some(bcast.extra),
            recv_from: src_adnl_key_id,
        };
        Ok((Some(info), resend))
    }

    // Send side

    fn calc_broadcast_id(
        &self,
        ctx: &BroadcastSendContext,
        date: i32,
    ) -> Result<(BroadcastId, bool)> {
        let data_size = ctx.data.object.len();
        <Self as BroadcastTwostep>::calc_broadcast_id_when_send(
            ctx,
            date,
            data_size,
            data_size,
            self.extra.as_deref().unwrap_or_default(),
        )
    }

    fn build_broadcast(
        &mut self,
        ctx: &BroadcastSendContext,
        bcast_id: &BroadcastId,
        date: i32,
        _neighbours: &mut BroadcastNeighbours,
    ) -> Result<BroadcastJob> {
        let data = ctx.data.object.to_vec();
        let to_sign = Self::calc_to_sign(*bcast_id, &data)?;
        let signature = ctx.src_key.sign(&to_sign)?;
        Ok(BroadcastJob::Foreground(
            BroadcastTwostepSimple {
                date,
                flags: ctx.flags as i32,
                src: ctx.src_key.try_into()?,
                src_adnl_id: UInt256::from_slice(ctx.src_adnl_key_id.data()),
                certificate: OverlayCertificate::Overlay_EmptyCertificate,
                data,
                extra: self.extra.take().unwrap_or_default(),
                signature,
            }
            .into_boxed(),
        ))
    }
}
