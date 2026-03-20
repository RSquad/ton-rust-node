/*
 * Copyright (C) 2019-2021 TON Labs. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use adnl::common::AdnlCryptoUtils;
#[cfg(feature = "node")]
use adnl::node::AddressCache;
use rand::Rng;
use std::convert::TryInto;
#[cfg(feature = "node")]
use std::sync::{Arc, Barrier};
use ton_block::Ed25519KeyOption;
#[cfg(feature = "node")]
use ton_block::{base64_encode, KeyId};

#[cfg(feature = "node")]
include!("../../common/src/test.rs");

#[test]
fn sign_verify() {
    let data: [u8; 32] = rand::thread_rng().gen();
    let key = Ed25519KeyOption::generate().unwrap();
    let signature = key.sign(&data).unwrap();
    assert!(key.verify(&data, &signature).is_ok())
}

#[cfg(feature = "node")]
#[test]
fn test_cache() {
    init_test_log();
    let cache = Arc::new(AddressCache::with_limit(3000));
    let count = Arc::new(Barrier::new(4));
    for i in 0..3 {
        let cache = cache.clone();
        let count = count.clone();
        std::thread::spawn(move || {
            let mut key = [0; 32];
            key[31] = i;
            for j in 0..1000u16 {
                key[0] = (j % 256) as u8;
                key[1] = (j / 256) as u8;
                let key_id = KeyId::from_data(key.clone());
                assert!(cache.put(key_id.clone()).unwrap());
                assert!(cache.contains(&key_id))
            }
            count.wait();
        });
    }
    count.wait();
    for i in 0..3 {
        let mut key = [0; 32];
        key[31] = i;
        for j in 0..1000u16 {
            key[0] = (j % 256) as u8;
            key[1] = (j / 256) as u8;
            assert!(cache.contains(&KeyId::from_data(key.clone())))
        }
    }
    assert!(!cache.contains(&KeyId::from_data([1; 32])));
    let (mut iter, mut key) = cache.first();
    let mut cnt = 0;
    while key.is_some() {
        cnt += 1;
        key = cache.next(&mut iter);
    }
    assert_eq!(cnt, 3000);
    cache.dump();
}

#[cfg(feature = "node")]
#[test]
fn test_cache_limit() {
    init_test_log();
    const MAX_LIMIT: u32 = 5;
    const MAX_COUNT: u32 = 13;
    let cache = Arc::new(AddressCache::with_limit(MAX_LIMIT));
    let mut keys = Vec::new();
    let mut key = [0; 32];
    for i in 0..MAX_COUNT {
        key[31] = i as u8;
        keys.push(KeyId::from_data(key.clone()));
    }
    let mut i = 0;
    for j in 0..MAX_LIMIT {
        assert!(cache.put(keys[i].clone()).unwrap());
        assert_eq!(cache.count(), j + 1);
        i += 1;
    }
    let count = Arc::new(Barrier::new(11));
    for _ in 0..10 {
        let cache = cache.clone();
        let count = count.clone();
        let keys = keys.clone();
        std::thread::spawn(move || {
            for _ in 0..1000u16 {
                cache.put(keys[i].clone()).unwrap();
                assert_eq!(cache.count(), MAX_LIMIT);
                i += 1;
                i %= MAX_COUNT as usize;
            }
            count.wait();
        });
    }
    count.wait();
    cache.dump();
}

#[cfg(feature = "node")]
#[test]
fn test_private_to_public() {
    let pvts = [
        "39762fde02a6c54920baff83930ac34b1dc81fc57e3ac1a82e94de365dd963bb",
        "8c37c9dc04018e18a3a8695244be55667ec7607e2c3155486328d38c2dc635c0",
        "d4517e1e4f07a0c56e9dba7091766d0cc97a220303c58b42f8a984002d0dc2fc",
        "fe4d47f24959efc7699e61a9c3618e7268fcca52b6d524a32c06f5449d983552",
    ];
    println!("");
    for pvt in pvts.iter() {
        let pvt = hex::decode(pvt).unwrap();
        let key = Ed25519KeyOption::from_private_key(&pvt.clone().try_into().unwrap()).unwrap();
        println!(
            "Private {} -> {}, id {}",
            base64_encode(&pvt),
            base64_encode(key.pub_key().unwrap()),
            base64_encode(key.id().data())
        )
    }
}

#[test]
fn test_shared_secret() {
    let key_a = [
        0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c,
        0xc4, 0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae,
        0x7f, 0x60,
    ];
    let key_a = Ed25519KeyOption::from_private_key(&key_a).unwrap();
    let key_b = [
        0xc5, 0xaa, 0x8d, 0xf4, 0x3f, 0x9f, 0x83, 0x7b, 0xed, 0xb7, 0x44, 0x2f, 0x31, 0xdc, 0xb7,
        0xb1, 0x66, 0xd3, 0x85, 0x35, 0x07, 0x6f, 0x09, 0x4b, 0x85, 0xce, 0x3a, 0x2e, 0x0b, 0x44,
        0x58, 0xf7,
    ];
    let key_b = Ed25519KeyOption::from_private_key(&key_b).unwrap();
    let shared_secret_a =
        key_a.shared_secret(key_b.pub_key().unwrap().try_into().unwrap()).unwrap();
    println!("secret(PK1, PubK2)={}", hex::encode(&shared_secret_a));
    let shared_secret_b =
        key_b.shared_secret(key_a.pub_key().unwrap().try_into().unwrap()).unwrap();
    println!("secret(PK2, PubK1)={}", hex::encode(&shared_secret_b));
    assert_eq!(shared_secret_a, shared_secret_b);
}

#[test]
fn test_build_cipher_secure() {
    let secret = (0..32).collect::<Vec<u8>>().try_into().unwrap();
    let digest = (64..96).collect::<Vec<u8>>().try_into().unwrap();
    let mut data = (0..10).collect::<Vec<u8>>();
    AdnlCryptoUtils::build_cipher_secure(&secret, &digest).apply_keystream(&mut data);
    assert_eq!(data, vec![78, 191, 242, 41, 41, 184, 209, 18, 29, 31]);
}
