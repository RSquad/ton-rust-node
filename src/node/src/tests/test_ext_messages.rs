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
use super::*;
use std::{cmp::min, time::Instant};
use ton_block::{
    write_boc, BocWriter, BuilderData, CellType, ExternalInboundMessageHeader,
    GetRepresentationHash, IBitstring, InternalMessageHeader, MsgAddressExt, MsgAddressInt,
    Serializable, SliceData,
};

#[test]
fn test_create_ext_message_big_data() {
    let big_data = [0; MAX_EXTERNAL_MESSAGE_SIZE + 6];
    create_ext_message(&big_data).expect_err("it must return error");
}

#[test]
fn test_create_ext_message_bad_data() {
    let big_data = [0; 100];
    create_ext_message(&big_data).expect_err("it must not accept wrong format BOC");
}

#[test]
fn test_create_ext_message_bad_boc_roots() {
    let cell1 = 0xfff1u32.serialize().unwrap();
    let cell2 = 0xfff3u32.serialize().unwrap();

    let boc = BocWriter::with_roots([cell1, cell2]).unwrap();
    let mut data = vec![];
    boc.write(&mut data).unwrap();

    create_ext_message(&data).expect_err("it must accept BOC only with one root");
}

#[test]
fn test_create_ext_message_bad_boc_level() {
    let mut root1 = BuilderData::new();
    root1.append_u8(1).unwrap();
    root1.append_u8(1).unwrap();
    root1.append_u8(0).unwrap();
    root1.append_u8(0).unwrap();
    root1.append_u128(0).unwrap();
    root1.append_u128(0).unwrap();
    root1.set_type(CellType::PrunedBranch);
    let cell1 = root1.into_cell().unwrap();

    let data = write_boc(&cell1).unwrap();

    create_ext_message(&data).expect_err("it must accept BOC only with one root");
}

#[test]
fn test_create_ext_message_bad_boc_depth() {
    let mut root = BuilderData::new();
    for _ in 0..600 {
        root.append_u32(0xfff0).unwrap();
        let mut new_root = BuilderData::new();
        new_root.checked_append_reference(root.into_cell().unwrap()).unwrap();
        root = new_root;
    }
    let cell = root.into_cell().unwrap();
    let data = write_boc(&cell).unwrap();

    create_ext_message(&data).expect_err("it must accept BOC only with correct depth size");
}

#[test]
fn test_create_ext_message_bad_boc_cells() {
    let mut root1 = BuilderData::new();
    root1.append_u32(0xfff1).unwrap();
    let cell1 = root1.into_cell().unwrap();
    let data = write_boc(&cell1).unwrap();

    create_ext_message(&data).expect_err("it must accept BOC only with external inbound message");
}

#[test]
fn test_create_ext_message_bad_message_format() {
    let msg = Message::with_int_header(InternalMessageHeader::default());
    let b = msg.serialize().unwrap();
    let data = write_boc(&b).unwrap();

    create_ext_message(&data).expect_err("it must accept BOC only with external inbound message");
}

#[test]
fn test_create_ext_message() {
    let msg = Message::with_ext_in_header(ExternalInboundMessageHeader::default());
    let b = msg.serialize().unwrap();
    let data = write_boc(&b).unwrap();

    create_ext_message(&data).unwrap();
}

#[test]
fn test_message_keeper() {
    let m = Message::with_ext_in_header(ExternalInboundMessageHeader::default());
    let mk = MessageKeeper::new(Arc::new(m), Default::default(), 0).unwrap();

    assert!(mk.check_active(10000));

    assert!(mk.can_postpone());
    // gen=0: postpone delays by (0+1)*5 = 5s -> reactivate_at=205
    mk.postpone(200);
    mk.postpone(300); // no-op while inactive
    mk.postpone(400); // no-op while inactive

    assert!(!mk.check_active(204));
    assert!(mk.check_active(205));
    assert!(mk.check_active(206));

    assert!(mk.can_postpone());
    // gen=1: delay by 10s -> reactivate_at=316
    mk.postpone(306);

    assert!(!mk.check_active(307));
    assert!(!mk.check_active(315));
    assert!(mk.check_active(316));
    assert!(mk.check_active(317));

    assert!(mk.can_postpone());
    // gen=2: delay by 15s -> reactivate_at=335
    mk.postpone(320);
    assert!(mk.check_active(335));

    assert!(!mk.can_postpone());
}

#[test]
fn test_message_keeper_multithread() {
    let m = Message::with_ext_in_header(ExternalInboundMessageHeader::default());
    let mk = Arc::new(MessageKeeper::new(Arc::new(m), Default::default(), 0).unwrap());

    let mut hs = vec![];
    for _ in 0..50 {
        let mk = mk.clone();
        let h = std::thread::spawn(move || {
            assert!(mk.check_active(10000));
            std::thread::sleep(std::time::Duration::from_millis(200));

            assert!(mk.can_postpone());
            // gen=0: reactivate at 10001+5 = 10006
            mk.postpone(10001);
            std::thread::sleep(std::time::Duration::from_millis(200));

            assert!(mk.check_active(10006));
            std::thread::sleep(std::time::Duration::from_millis(200));

            assert!(mk.can_postpone());
            // gen=1: reactivate at 10007+10 = 10017
            mk.postpone(10007);
            assert!(!mk.check_active(10009));
            std::thread::sleep(std::time::Duration::from_millis(200));

            assert!(mk.check_active(10017));
            std::thread::sleep(std::time::Duration::from_millis(200));

            assert!(mk.can_postpone());
            // gen=2: reactivate at 10018+15 = 10033
            mk.postpone(10018);
            std::thread::sleep(std::time::Duration::from_millis(200));

            assert!(mk.check_active(10033));
            assert!(!mk.can_postpone());
        });
        hs.push(h);
    }

    for h in hs {
        h.join().unwrap();
    }
}

fn create_external_message(dst_shard: u8, salt: Vec<u8>) -> Arc<Message> {
    create_external_message_to([dst_shard; 32], salt)
}

fn create_external_message_to(dst_account: [u8; 32], salt: Vec<u8>) -> Arc<Message> {
    let mut hdr = ExternalInboundMessageHeader::default();
    let length_in_bits = salt.len() * 8;
    let address = SliceData::from_raw(salt, length_in_bits);
    hdr.src = MsgAddressExt::with_extern(address).unwrap();
    hdr.dst = MsgAddressInt::with_standart(None, 0, dst_account.into()).unwrap();
    hdr.import_fee = 10u64.into();
    Arc::new(Message::with_ext_in_header(hdr))
}

#[test]
fn test_messages_pool() {
    //init_log_without_config(log::LevelFilter::Trace, None);
    let mp = Arc::new(MessagesPool::new(0, None).0);

    // create 3 messages, 2 of them are with the prefix 0x01 and one with 0x22
    let m = create_external_message(1, vec![1]);
    let id1 = m.hash().unwrap();
    mp.new_message(&id1, m, 0).unwrap();

    let m = create_external_message(1, vec![2]);
    let id2 = m.hash().unwrap();
    mp.new_message(&id2, m, 0).unwrap();

    let m = create_external_message(0x22, vec![2]);
    let id3 = m.hash().unwrap();
    mp.new_message(&id3, m.clone(), 0).unwrap();

    // get messages for shard 0x8000_0000_0000_0000 - total 3 messages
    let m1 = mp
        .get_messages(&ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).unwrap(), 1)
        .unwrap();
    assert_eq!(m1.len(), 3);

    // get messages for shard 0x3000_0000_0000_0000 - total 1 message with prefix 0x22
    let m1 = mp
        .get_messages(&ShardIdent::with_tagged_prefix(0, 0x3000_0000_0000_0000).unwrap(), 1)
        .unwrap();
    assert_eq!(m1.len(), 1);
    assert_eq!(m1[0].0, m);

    // postpone message with prefix 0x22 for first time (gen=0 -> reactivate at 6)
    mp.complete_messages(&[id3.clone()], &[], 1).unwrap();

    // get messages for shard 0x1000_0000_0000_0000 - total 2 messages with prefix 0x01
    let m1 = mp
        .get_messages(&ShardIdent::with_tagged_prefix(0, 0x1000_0000_0000_0000).unwrap(), 3)
        .unwrap();
    assert_eq!(m1.len(), 2);
    assert_eq!(m1[0].1, id1);
    assert_eq!(m1[1].1, id2);

    // at t=6 the postponed message has just reactivated (gen->1)
    let m1 = mp
        .get_messages(&ShardIdent::with_tagged_prefix(0, 0x3000_0000_0000_0000).unwrap(), 6)
        .unwrap();
    assert_eq!(m1.len(), 1);
    assert_eq!(m1[0].0, m);

    // postpone for second time (gen=1 -> reactivate at 16)
    mp.complete_messages(&[id3.clone()], &[], 6).unwrap();

    // at t=15 still inactive
    let m1 = mp
        .get_messages(&ShardIdent::with_tagged_prefix(0, 0x3000_0000_0000_0000).unwrap(), 15)
        .unwrap();
    assert_eq!(m1.len(), 0);

    // at t=16 reactivates (gen->2)
    let m1 = mp
        .get_messages(&ShardIdent::with_tagged_prefix(0, 0x3000_0000_0000_0000).unwrap(), 16)
        .unwrap();
    assert_eq!(m1.len(), 1);

    // postpone for third time (gen=2 -> reactivate at 31)
    mp.complete_messages(&[id3.clone()], &[], 16).unwrap();

    // at t=31 reactivates (gen->3)
    let m1 = mp
        .get_messages(&ShardIdent::with_tagged_prefix(0, 0x3000_0000_0000_0000).unwrap(), 31)
        .unwrap();
    assert_eq!(m1.len(), 1);

    // fourth postpone: gen==3, can_postpone returns false -> erased
    mp.complete_messages(&[id3.clone()], &[], 32).unwrap();

    // get messages for shard 0x3000_0000_0000_0000 - no messages
    let m1 = mp
        .get_messages(&ShardIdent::with_tagged_prefix(0, 0x3000_0000_0000_0000).unwrap(), 100)
        .unwrap();
    assert_eq!(m1.len(), 0);

    // get messages for shard 0x1000_0000_0000_0000 - no messages because it was expired
    let m1 = mp
        .get_messages(&ShardIdent::with_tagged_prefix(0, 0x1000_0000_0000_0000).unwrap(), 601)
        .unwrap();
    assert_eq!(m1.len(), 0);
}

// Remove + re-add into a NEWER bucket leaves a stale slot in the old bucket
// that still resolves to the re-added (live) keeper, so the same id is
// reachable through two slots in different buckets. The iterator must dedup by
// id and yield it only once.
#[test]
fn test_remove_and_readd_does_not_double_yield() {
    let mp = Arc::new(MessagesPool::new(0, None).0);
    let shard = ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).unwrap();

    let msg = create_external_message(1, vec![42]);
    let id = msg.hash().unwrap();

    mp.new_message(&id, msg.clone(), 1).unwrap();
    mp.complete_messages(&[], &[id.clone()], 1).unwrap();
    // Check removal without iterating: an iter() here would sweep the stale t=1
    // slot as a tombstone and the test would no longer exercise the bug.
    assert_eq!(mp.total_messages(), 0);

    // Re-add at t=2 (a different `order` bucket) before any iteration, so the
    // stale t=1 slot survives and still resolves to the live keeper.
    mp.new_message(&id, msg, 2).unwrap();
    assert_eq!(mp.clone().iter(shard, 2, u64::MAX).count(), 1);
}

// Remove + re-add at the SAME timestamp with no iteration in between leaves two
// slots in the same `order` bucket, both resolving to the re-added keeper. The
// iterator must still yield it only once.
#[test]
fn test_remove_and_readd_same_timestamp_does_not_double_yield() {
    let mp = Arc::new(MessagesPool::new(0, None).0);
    let shard = ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).unwrap();

    let msg = create_external_message(1, vec![99]);
    let id = msg.hash().unwrap();

    mp.new_message(&id, msg.clone(), 1).unwrap();
    mp.complete_messages(&[], &[id.clone()], 1).unwrap();
    // No iteration here, so the stale slot at seqno=0 is not swept.
    mp.new_message(&id, msg, 1).unwrap();

    assert_eq!(mp.clone().iter(shard, 1, u64::MAX).count(), 1);
}

// A message removed and re-added in a newer bucket must survive expiry of the
// old bucket: its stale slot there must not evict the re-added keeper.
#[test]
fn test_readd_survives_old_bucket_expiry() {
    let mp = Arc::new(MessagesPool::new(0, None).0);
    let shard = ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).unwrap();

    let msg = create_external_message(1, vec![7]);
    let id = msg.hash().unwrap();

    // Add at t=1, then remove -> leaves a stale slot in bucket t=1.
    mp.new_message(&id, msg.clone(), 1).unwrap();
    mp.complete_messages(&[], &[id.clone()], 1).unwrap();

    // Re-add in bucket t=100 (home_ts = 100). Don't iterate before the sweep:
    // a full iter would reach bucket t=1 and drop the stale slot itself, hiding
    // the early-eviction path through clear_expired_messages.
    mp.new_message(&id, msg, 100).unwrap();
    assert_eq!(mp.total_messages(), 1);

    // Sweep the old t=1 bucket. The stale slot there points at id, but the live
    // keeper's home_ts is 100, so it must not be evicted.
    mp.clear_expired_messages(1, u64::MAX);
    assert_eq!(
        mp.clone().iter(shard, 100, u64::MAX).count(),
        1,
        "re-added message must survive old-bucket expiry"
    );
}

// A cross-bucket stale slot (remove + re-add into a newer bucket) must be
// dropped by the iterator, not left to be re-scanned every pass until TTL.
#[test]
fn test_iter_drops_cross_bucket_stale_slot() {
    let mp = Arc::new(MessagesPool::new(0, None).0);
    let shard = ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).unwrap();

    let msg = create_external_message(1, vec![5]);
    let id = msg.hash().unwrap();

    mp.new_message(&id, msg.clone(), 1).unwrap(); // slot in bucket t=1
    mp.complete_messages(&[], &[id.clone()], 1).unwrap(); // remove -> stale slot at t=1
    mp.new_message(&id, msg, 2).unwrap(); // re-add into bucket t=2

    // The stale slot still sits in bucket t=1 until something walks it.
    let before = mp.order.get(&1).map(|g| g.val().map.iter().count()).unwrap_or(0);
    assert_eq!(before, 1, "stale slot should still be present before iteration");

    // One iteration pass yields the message once and removes the stale slot.
    assert_eq!(mp.clone().iter(shard, 2, u64::MAX).count(), 1);
    let after = mp.order.get(&1).map(|g| g.val().map.iter().count()).unwrap_or(0);
    assert_eq!(after, 0, "iterator must drop the stale cross-bucket slot");
}

// Concurrent submissions of the same id must collapse to one keeper and one
// order slot: dedup losers must not double-yield or inflate the bucket seqno.
#[test]
fn test_concurrent_same_id_dedup() {
    use std::sync::Barrier;

    let mp = Arc::new(MessagesPool::new(0, None).0);
    let shard = ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).unwrap();

    let msg = create_external_message(1, vec![21]);
    let id = msg.hash().unwrap();

    let n = 8;
    let barrier = Arc::new(Barrier::new(n));
    let handles: Vec<_> = (0..n)
        .map(|_| {
            let mp = mp.clone();
            let msg = msg.clone();
            let id = id.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                mp.new_message(&id, msg, 1).unwrap();
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }

    assert_eq!(mp.total_messages(), 1, "concurrent same-id must yield a single keeper");
    assert_eq!(mp.clone().iter(shard, 1, u64::MAX).count(), 1, "iter must yield exactly one");
    // Only the winner reaches the order insert, so the bucket holds one slot and
    // its seqno counter was not inflated by the losing admissions.
    let guard = mp.order.get(&1).expect("bucket t=1 exists");
    assert_eq!(guard.val().map.iter().count(), 1, "exactly one order slot");
    assert_eq!(
        guard.val().seqno.load(std::sync::atomic::Ordering::Relaxed),
        1,
        "seqno counter not inflated"
    );
}

async fn check_messages(
    mp: Arc<MessagesPool>,
    shard: ShardIdent,
    now: u32,
    expected_count: usize,
) -> Result<()> {
    let iterations = 20;
    for i in 1..=iterations {
        // let count = mp.clone().get_messages(&shard, now).unwrap().len();
        let count = mp.clone().iter(shard.clone(), now, u64::MAX).count();
        if count != expected_count {
            fail!(
                "Wrong messages count for shard {} expected {}, got {} on {} iteration",
                shard,
                expected_count,
                count,
                i
            )
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_ext_messages_multi_threads() {
    const M: usize = 50;
    let mp = Arc::new(MessagesPool::new(0, None).0);

    // total 8 prefixes by 50 messages in each
    for prefix in [0, 0x20, 0x40, 0x60, 0x80, 0xA0, 0xC0, 0xE0] {
        for i in 0..M {
            let m = create_external_message(prefix, vec![i as u8]);
            let id = m.hash().unwrap();
            mp.new_message(&id, m, 0).unwrap();
        }
    }

    // get messages to destination shards
    let tests = [
        [(0x20, M * 2), (0x60, M * 2), (0xA0, M * 2), (0xE0, M * 2)],
        [(0x40, M * 4), (0xA0, M * 2), (0xD0, M), (0xF0, M)],
    ];

    for test_case in tests {
        let mut handles = Vec::new();
        for (shard, expected_count) in test_case {
            let shard = ShardIdent::with_tagged_prefix(0, shard << 56).unwrap();
            let mp = Arc::clone(&mp);
            let handle = tokio::spawn(check_messages(mp, shard, 0, expected_count));
            handles.push(handle);
        }
        for handle in handles {
            handle.await.unwrap().unwrap();
        }
    }
}

#[test]
fn test_external_messages_maximum_queue_length() {
    let maximum_queue_length = 10;
    let mp = Arc::new(MessagesPool::new(0, Some(maximum_queue_length)).0);
    for i in 0..maximum_queue_length {
        let m = create_external_message(0, vec![i as u8]);
        let id = m.hash().unwrap();
        mp.new_message(&id, m, 0).unwrap();
    }
    let m = create_external_message(0, vec![maximum_queue_length as u8]);
    let id = m.hash().unwrap();
    mp.new_message(&id, m, 0).unwrap_err();
}

#[test]
fn test_external_messages_big_load() {
    let now = UnixTime::now() as u32 - MESSAGE_LIFETIME - 1;
    let limit = 100; // milliseconds
    let mp = Arc::new(MessagesPool::new(now, None).0);
    let rate_per_second = 30_000;
    let queue_seconds = min(MESSAGE_LIFETIME, 100);
    for i in 0..queue_seconds {
        for j in 0..rate_per_second {
            let idx = i * rate_per_second + j;
            let mut dst = [0u8; 32];
            dst[..4].copy_from_slice(&idx.to_be_bytes());
            let m = create_external_message_to(dst, idx.to_be_bytes().to_vec());
            let id = m.hash().unwrap();
            mp.new_message(&id, m, now + i).unwrap();
        }
    }
    let (count, n) = {
        let n = Instant::now();
        let now = UnixTime::now_ms() as u64;
        let count = mp
            .clone()
            .iter(ShardIdent::full(0), (now / 1000) as u32, now + limit)
            .take(100)
            .count();
        let n = n.elapsed().as_millis();
        (count, n)
    };
    println!("count = {}, time = {:?}", count, n);
    // With newest-first iteration, non-expired messages are found quickly.
    // The pool contains messages from ~502 to ~601 seconds ago;
    // those within MESSAGE_LIFETIME (600s) are valid and returned.
    assert!(count <= 100);
    assert!((n as u64) < limit * 3);
}
