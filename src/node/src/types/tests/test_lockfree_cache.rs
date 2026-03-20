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
use adnl::{common::Counter, declare_counted};

declare_counted!(
    struct Test {
        val: String,
    }
);

#[tokio::test(flavor = "multi_thread")]
async fn test_lockfree_cache() {
    let counter = Arc::new(AtomicU64::new(0));
    let cache = TimeBasedCache::new(4, "test_lockfree_cache".to_string());

    assert!(cache.get(&1).is_none());
    assert!(cache.get(&2).is_none());
    assert!(cache.get(&3).is_none());

    assert!(cache
        .set(1, |prev| {
            assert!(prev.is_none());
            let ret = Test { val: "1".to_string(), counter: counter.clone().into() };
            Some(Arc::new(ret))
        })
        .unwrap());
    assert!(cache
        .set(2, |prev| {
            assert!(prev.is_none());
            let ret = Test { val: "2".to_string(), counter: counter.clone().into() };
            Some(Arc::new(ret))
        })
        .unwrap());
    assert!(cache
        .set(3, |prev| {
            assert!(prev.is_none());
            let ret = Test { val: "3".to_string(), counter: counter.clone().into() };
            Some(Arc::new(ret))
        })
        .unwrap());

    assert!(cache.get(&1).is_some());
    assert!(cache.get(&2).is_some());
    assert!(cache.get(&3).is_some());

    assert!(!cache
        .set(2, |prev| {
            assert!(prev.is_some());
            None
        })
        .unwrap());

    futures_timer::Delay::new(Duration::from_secs(2)).await;

    assert!(cache.get(&1).is_some());

    futures_timer::Delay::new(Duration::from_secs(4)).await;

    assert!(cache.get(&1).is_some());
    assert!(cache.get(&2).is_none());
    assert!(cache.get(&3).is_none());

    futures_timer::Delay::new(Duration::from_secs(6)).await;
    assert!(cache.get(&1).is_none());
}
