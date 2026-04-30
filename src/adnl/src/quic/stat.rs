use crate::common::add_unbound_object_to_map;
use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Instant,
};
use ton_api::deserialize_boxed_with_suffix;

/// Extract the "inner" TL constructor tag from message data.
///
/// QUIC message payloads are typically wrapped in an overlay prefix
/// (`overlay.message` or `overlay.query`). The outer tag is not useful
/// for diagnostics. This function skips past the overlay wrapper and
/// returns the constructor tag of the actual inner payload.
///
/// `overlay.message` and `overlay.query` have a fixed layout:
///   constructor(4 bytes) + int256(32 bytes) = 36 bytes prefix.
/// `WithExtra` variants have a variable-length extra field, so we
/// fall back to `deserialize_boxed_with_suffix` for those.
pub(super) fn extract_inner_tag(data: &[u8]) -> u32 {
    if data.len() < 4 {
        return 0;
    }
    let outer = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    // overlay.message / overlay.query: fixed 36-byte prefix (constructor + int256)
    const FIXED_PREFIX: usize = 4 + 32;
    match outer {
        0x75252420 | 0xccfd8443 => {
            // overlay.message, overlay.query
            if data.len() >= FIXED_PREFIX + 4 {
                let s = &data[FIXED_PREFIX..];
                return u32::from_le_bytes([s[0], s[1], s[2], s[3]]);
            }
            outer
        }
        0xa232233d | 0x94ffc3e9 => {
            // overlay.messageWithExtra, overlay.queryWithExtra
            if let Ok((_obj, suffix_offset)) = deserialize_boxed_with_suffix(data) {
                if suffix_offset + 4 <= data.len() {
                    let s = &data[suffix_offset..];
                    return u32::from_le_bytes([s[0], s[1], s[2], s[3]]);
                }
            }
            outer
        }
        _ => outer,
    }
}

/// Map well-known TL constructor tags to short human-readable names for log output.
pub(super) fn tl_tag_name(tag: u32) -> &'static str {
    match tag {
        0x75252420 => "overlay.message",
        0xa232233d => "overlay.messageWithExtra",
        0xccfd8443 => "overlay.query",
        0x94ffc3e9 => "overlay.queryWithExtra",
        0xb15a2b6b => "overlay.broadcast",
        0xbad7c36a => "overlay.broadcastFec",
        0xf1881342 => "overlay.broadcastFecShort",
        0x46efae62 => "overlay.broadcastStream",
        0xf99fd63d => "overlay.broadcastTwostepFec",
        0x80b859b0 => "overlay.broadcastTwostepSimple",
        0x33534e24 => "overlay.unicast",
        0xd55c14ec => "overlay.fec.received",
        0x09d76914 => "overlay.fec.completed",
        0x48ee64ab => "overlay.getRandomPeers",
        0xa58e7ecc => "overlay.getRandomPeersV2",
        0x690cb481 => "overlay.ping",
        0x236758c4 => "catchain.blockUpdate",
        0x9283ce37 => "validatorSession.blockUpdate",
        0xbe7b573a => "consensus.simplex.certificate",
        0xc37ef4f3 => "consensus.simplex.vote",
        0x543fba6c => "consensus.simplex.requestCandidate",
        _ => "unknown",
    }
}

/// Aggregate error counters for the QUIC transport layer (lock-free, reset on each stats dump).
pub(super) struct TransportErrors {
    /// send_via_stream / send_via_stream_nowait failures (stream open, write, finish errors)
    pub send_failed: AtomicU64,
    /// query timeouts (deadline exceeded waiting for response)
    pub query_timeout: AtomicU64,
    /// connection establishment failures (connect / handshake errors)
    pub connect_failed: AtomicU64,
    /// messages dropped because the per-peer send queue was full
    pub queue_full: AtomicU64,
    /// dead connections removed (by checker or on send failure)
    pub dead_conn_removed: AtomicU64,
    /// connections rejected by per-IP rate limiter
    pub rate_limited_per_ip: AtomicU64,
    /// connections rejected by global rate limiter
    pub rate_limited_global: AtomicU64,
    /// stateless Retry packets sent
    pub retry_sent: AtomicU64,
    /// connections accepted into handle_connection()
    pub accepted: AtomicU64,
    /// connections that went through the delayed accept path
    pub delayed: AtomicU64,
    /// connections refused because the delayed-accept limit was reached
    pub delayed_refused: AtomicU64,
}

impl TransportErrors {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            send_failed: AtomicU64::new(0),
            query_timeout: AtomicU64::new(0),
            connect_failed: AtomicU64::new(0),
            queue_full: AtomicU64::new(0),
            dead_conn_removed: AtomicU64::new(0),
            rate_limited_per_ip: AtomicU64::new(0),
            rate_limited_global: AtomicU64::new(0),
            retry_sent: AtomicU64::new(0),
            accepted: AtomicU64::new(0),
            delayed: AtomicU64::new(0),
            delayed_refused: AtomicU64::new(0),
        })
    }

    /// Atomically take current values and reset all counters to zero.
    pub fn take(&self) -> TransportErrorsSnapshot {
        TransportErrorsSnapshot {
            send_failed: self.send_failed.swap(0, Ordering::Relaxed),
            query_timeout: self.query_timeout.swap(0, Ordering::Relaxed),
            connect_failed: self.connect_failed.swap(0, Ordering::Relaxed),
            queue_full: self.queue_full.swap(0, Ordering::Relaxed),
            dead_conn_removed: self.dead_conn_removed.swap(0, Ordering::Relaxed),
            rate_limited_per_ip: self.rate_limited_per_ip.swap(0, Ordering::Relaxed),
            rate_limited_global: self.rate_limited_global.swap(0, Ordering::Relaxed),
            retry_sent: self.retry_sent.swap(0, Ordering::Relaxed),
            accepted: self.accepted.swap(0, Ordering::Relaxed),
            delayed: self.delayed.swap(0, Ordering::Relaxed),
            delayed_refused: self.delayed_refused.swap(0, Ordering::Relaxed),
        }
    }
}

/// Point-in-time snapshot returned by `TransportErrors::take()`.
pub(super) struct TransportErrorsSnapshot {
    pub send_failed: u64,
    pub query_timeout: u64,
    pub connect_failed: u64,
    pub queue_full: u64,
    pub dead_conn_removed: u64,
    pub rate_limited_per_ip: u64,
    pub rate_limited_global: u64,
    pub retry_sent: u64,
    pub accepted: u64,
    pub delayed: u64,
    pub delayed_refused: u64,
}

/// Snapshot of cumulative counters from a single connection, used to compute deltas.
#[derive(Clone, Copy)]
pub(super) struct ConnSnapshot {
    pub tx_bytes: u64,
    pub tx_dgrams: u64,
    pub rx_bytes: u64,
    pub rx_dgrams: u64,
    pub lost_pkts: u64,
    /// When this connection was first observed by the stats dumper.
    pub connected_since: Instant,
}

impl ConnSnapshot {
    pub fn new(s: &quinn::ConnectionStats, connected_since: Instant) -> Self {
        Self {
            tx_bytes: s.udp_tx.bytes,
            tx_dgrams: s.udp_tx.datagrams,
            rx_bytes: s.udp_rx.bytes,
            rx_dgrams: s.udp_rx.datagrams,
            lost_pkts: s.path.lost_packets,
            connected_since,
        }
    }

    pub fn delta(&self, prev: &Self) -> Self {
        Self {
            tx_bytes: self.tx_bytes.saturating_sub(prev.tx_bytes),
            tx_dgrams: self.tx_dgrams.saturating_sub(prev.tx_dgrams),
            rx_bytes: self.rx_bytes.saturating_sub(prev.rx_bytes),
            rx_dgrams: self.rx_dgrams.saturating_sub(prev.rx_dgrams),
            lost_pkts: self.lost_pkts.saturating_sub(prev.lost_pkts),
            connected_since: self.connected_since,
        }
    }

    pub fn uptime_str(&self) -> String {
        let secs = self.connected_since.elapsed().as_secs();
        if secs < 60 {
            format!("{secs}s")
        } else if secs < 3600 {
            format!("{}m{}s", secs / 60, secs % 60)
        } else if secs < 36 * 3600 {
            format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
        } else {
            let days = secs / 86400;
            let hours = (secs % 86400) / 3600;
            let mins = (secs % 3600) / 60;
            format!("{days}d{hours}h{mins}m")
        }
    }
}

/// Per-TL-tag message counters (lock-free atomics, collected per dump interval).
pub(super) struct MsgTagCounters {
    count: AtomicU64,
    bytes: AtomicU64,
}

impl MsgTagCounters {
    pub fn new() -> Self {
        Self { count: AtomicU64::new(0), bytes: AtomicU64::new(0) }
    }

    pub fn record(&self, size: usize) {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.bytes.fetch_add(size as u64, Ordering::Relaxed);
    }

    /// Take current values and reset to zero.
    pub fn take(&self) -> (u64, u64) {
        (self.count.swap(0, Ordering::Relaxed), self.bytes.swap(0, Ordering::Relaxed))
    }
}

/// Classification of a QUIC message for telemetry.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) enum MsgKind {
    Message,
    Query,
    Answer,
    NoAnswer,
}

impl MsgKind {
    pub fn label(&self) -> &'static str {
        match self {
            MsgKind::Message => "msg   ",
            MsgKind::Query => "query ",
            MsgKind::Answer => "ans   ",
            MsgKind::NoAnswer => "no_ans",
        }
    }
}

/// Per-peer, per-TL-tag message statistics key.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct MsgStatsKey {
    pub addr: SocketAddr,
    pub tag: u32,
    pub is_outbound: bool,
    pub kind: MsgKind,
}

/// Tracks per-peer, per-message-kind statistics for QUIC traffic.
pub(super) struct MsgStats {
    counters: lockfree::map::Map<MsgStatsKey, MsgTagCounters>,
}

impl MsgStats {
    pub fn new() -> Arc<Self> {
        Arc::new(Self { counters: lockfree::map::Map::new() })
    }

    pub fn record(
        &self,
        tag: u32,
        size: usize,
        addr: SocketAddr,
        is_outbound: bool,
        kind: MsgKind,
    ) {
        let key = MsgStatsKey { addr, tag, is_outbound, kind };
        if let Some(entry) = self.counters.get(&key) {
            entry.val().record(size);
            return;
        }
        let _ = add_unbound_object_to_map(&self.counters, key, || Ok(MsgTagCounters::new()));
        if let Some(entry) = self.counters.get(&key) {
            entry.val().record(size);
        }
    }

    /// Drain all counters and return entries sorted by peer then bytes desc.
    /// Entries with zero activity since the last drain are removed
    pub fn drain(&self) -> Vec<(MsgStatsKey, u64, u64)> {
        let mut result = Vec::new();
        let mut stale = Vec::new();
        for entry in self.counters.iter() {
            let (count, bytes) = entry.val().take();
            if count > 0 {
                result.push((*entry.key(), count, bytes));
            } else {
                stale.push(*entry.key());
            }
        }
        for key in stale {
            self.counters.remove(&key);
        }
        result.sort_by(|a, b| a.0.addr.cmp(&b.0.addr).then(b.2.cmp(&a.2)));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- extract_inner_tag ---

    /// Helper: build an overlay.message (0x75252420) wrapping the given inner tag.
    fn make_overlay_message(inner_tag: u32) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&0x75252420u32.to_le_bytes()); // outer tag
        buf.extend_from_slice(&[0u8; 32]); // overlay int256
        buf.extend_from_slice(&inner_tag.to_le_bytes()); // inner payload tag
        buf
    }

    /// Helper: build an overlay.query (0xccfd8443) wrapping the given inner tag.
    fn make_overlay_query(inner_tag: u32) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&0xccfd8443u32.to_le_bytes());
        buf.extend_from_slice(&[0u8; 32]);
        buf.extend_from_slice(&inner_tag.to_le_bytes());
        buf
    }

    #[test]
    fn test_extract_inner_tag_empty() {
        assert_eq!(extract_inner_tag(&[]), 0);
        assert_eq!(extract_inner_tag(&[1, 2, 3]), 0);
    }

    #[test]
    fn test_extract_inner_tag_unknown_outer() {
        let data = 0xDEADBEEFu32.to_le_bytes();
        assert_eq!(extract_inner_tag(&data), 0xDEADBEEF);
    }

    #[test]
    fn test_extract_inner_tag_overlay_message() {
        let data = make_overlay_message(0x236758c4); // catchain.blockUpdate
        assert_eq!(extract_inner_tag(&data), 0x236758c4);
    }

    #[test]
    fn test_extract_inner_tag_overlay_query() {
        let data = make_overlay_query(0x48ee64ab); // overlay.getRandomPeers
        assert_eq!(extract_inner_tag(&data), 0x48ee64ab);
    }

    #[test]
    fn test_extract_inner_tag_overlay_message_too_short() {
        // outer tag + partial overlay id (not enough for inner tag)
        let mut data = Vec::new();
        data.extend_from_slice(&0x75252420u32.to_le_bytes());
        data.extend_from_slice(&[0u8; 30]); // only 30 bytes, need 32 + 4
        assert_eq!(extract_inner_tag(&data), 0x75252420); // falls back to outer
    }

    // --- MsgStats ---

    fn test_addr(port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }

    #[test]
    fn test_msg_stats_record_and_drain() {
        let stats = MsgStats::new();
        let addr = test_addr(1000);

        stats.record(0xAA, 100, addr, true, MsgKind::Message);
        stats.record(0xAA, 200, addr, true, MsgKind::Message);
        stats.record(0xBB, 50, addr, true, MsgKind::Query);

        let entries = stats.drain();
        assert_eq!(entries.len(), 2);

        // Sorted by addr (same), then bytes desc: AA(300) before BB(50)
        assert_eq!(entries[0].0.tag, 0xAA);
        assert_eq!(entries[0].1, 2); // count
        assert_eq!(entries[0].2, 300); // bytes

        assert_eq!(entries[1].0.tag, 0xBB);
        assert_eq!(entries[1].1, 1);
        assert_eq!(entries[1].2, 50);
    }

    #[test]
    fn test_msg_stats_drain_sorts_by_addr_then_bytes() {
        let stats = MsgStats::new();
        let addr_a = test_addr(1000);
        let addr_b = test_addr(2000);

        stats.record(0xAA, 10, addr_b, true, MsgKind::Message);
        stats.record(0xBB, 500, addr_a, true, MsgKind::Message);
        stats.record(0xCC, 100, addr_a, true, MsgKind::Message);

        let entries = stats.drain();
        assert_eq!(entries.len(), 3);

        // addr_a (port 1000) first, sorted by bytes desc
        assert_eq!(entries[0].0.addr, addr_a);
        assert_eq!(entries[0].0.tag, 0xBB); // 500 bytes
        assert_eq!(entries[1].0.addr, addr_a);
        assert_eq!(entries[1].0.tag, 0xCC); // 100 bytes

        // addr_b (port 2000) last
        assert_eq!(entries[2].0.addr, addr_b);
        assert_eq!(entries[2].0.tag, 0xAA);
    }

    #[test]
    fn test_msg_stats_drain_resets_counters() {
        let stats = MsgStats::new();
        let addr = test_addr(1000);

        stats.record(0xAA, 100, addr, true, MsgKind::Message);
        let entries = stats.drain();
        assert_eq!(entries.len(), 1);

        // Second drain: no new activity, should return empty
        let entries = stats.drain();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_msg_stats_drain_evicts_stale_keys() {
        let stats = MsgStats::new();
        let addr = test_addr(1000);

        stats.record(0xAA, 100, addr, true, MsgKind::Message);
        stats.record(0xBB, 50, addr, false, MsgKind::Message);

        // First drain: both active, counters reset
        let _ = stats.drain();

        // Only record on 0xAA
        stats.record(0xAA, 200, addr, true, MsgKind::Message);

        // Second drain: 0xBB was idle → evicted
        let _ = stats.drain();

        // Record on 0xBB again — must re-insert (was evicted)
        stats.record(0xBB, 30, addr, false, MsgKind::Message);
        let entries = stats.drain();

        // 0xAA was idle since last drain (evicted), only 0xBB with activity is returned
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0.tag, 0xBB);
        assert_eq!(entries[0].2, 30);
    }

    #[test]
    fn test_msg_stats_distinguishes_direction_and_kind() {
        let stats = MsgStats::new();
        let addr = test_addr(1000);

        stats.record(0xAA, 100, addr, true, MsgKind::Message); // outbound msg
        stats.record(0xAA, 200, addr, false, MsgKind::Message); // inbound msg
        stats.record(0xAA, 300, addr, true, MsgKind::Query); // outbound query

        let entries = stats.drain();
        assert_eq!(entries.len(), 3);
    }
}
