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
use adnl::common::{add_unbound_object_to_map_with_update, CountedObject};
use std::{
    fmt::Display,
    hash::Hash,
    marker::Sync,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use ton_block::{Result, UnixTime};

pub struct TimeBasedCache<K, V: CountedObject> {
    map: Arc<lockfree::map::Map<K, (V, AtomicU64)>>,
}

#[allow(dead_code)]
impl<K, V> TimeBasedCache<K, V>
where
    K: 'static + Hash + Ord + Sync + Send + Display,
    V: 'static + Clone + Sync + Send + CountedObject,
{
    pub fn new(ttl_sec: u64, name: String) -> Self {
        let map = Arc::new(lockfree::map::Map::new());
        Self::gc(map.clone(), ttl_sec, name);
        Self { map }
    }

    pub fn get(&self, id: &K) -> Option<V> {
        let guard = self.map.get(id)?;
        let now = UnixTime::now();
        guard.val().1.store(now, Ordering::Relaxed);
        Some(guard.val().0.clone())
    }

    pub fn set(&self, key: K, factory: impl Fn(Option<&V>) -> Option<V>) -> Result<bool> {
        add_unbound_object_to_map_with_update(&self.map, key, |prev| {
            let now = UnixTime::now();
            if let Some((v, t)) = prev {
                if let Some(new) = factory(Some(v)) {
                    Ok(Some((new, AtomicU64::new(now))))
                } else {
                    t.store(now, Ordering::Relaxed);
                    Ok(None)
                }
            } else if let Some(new) = factory(None) {
                Ok(Some((new, AtomicU64::new(now))))
            } else {
                Ok(None)
            }
        })
    }

    fn gc(map: Arc<lockfree::map::Map<K, (V, AtomicU64)>>, ttl: u64, name: String) {
        tokio::spawn(async move {
            loop {
                futures_timer::Delay::new(Duration::from_millis(ttl * 100)).await;
                let now = UnixTime::now();
                let mut len = 0;
                for guard in map.iter() {
                    let time = guard.val().1.load(Ordering::Relaxed);
                    if now > time && now - time > ttl {
                        map.remove(guard.key());
                    } else {
                        len += 1;
                    }
                }
                log::trace!("{} capacity: {}", name, len);
            }
        });
    }
}

#[cfg(test)]
#[path = "tests/test_lockfree_cache.rs"]
mod tests;
