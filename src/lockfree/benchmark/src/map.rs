#[macro_use]
extern crate benchsuite;
extern crate lockfree;

use benchsuite::exec::Target;
use lockfree::map::Map;
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    mem,
    sync::{Arc, Mutex},
};

type MutexInner = Arc<Mutex<HashMap<BadHash, usize>>>;
type LockfreeInner = Arc<Map<BadHash, usize>>;

fn make_key(i: usize) -> BadHash {
    let i = i as u128;
    BadHash(i.wrapping_mul(i ^ i >> 16).wrapping_add(i))
}

fn prevent_opt<T>(val: T) {
    unsafe {
        let mut _local = mem::uninitialized();
        (&mut _local as *mut T).write_volatile(val);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct BadHash(u128);

impl Hash for BadHash {
    fn hash<H>(&self, hasher: &mut H)
    where
        H: Hasher,
    {
        hasher.write_u8(self.0 as u8);
        hasher.write_u8((self.0 >> 16) as u8);
        hasher.write_u8((self.0 >> 32) as u8);
        hasher.write_u8((self.0 >> 48) as u8);
        hasher.write_u8((self.0 >> 64) as u8);
        hasher.write_u8((self.0 >> 80) as u8);
        hasher.write_u8((self.0 >> 96) as u8);
        hasher.write_u8((self.0 >> 112) as u8);
    }
}

#[derive(Debug, Clone, Default)]
struct MutexInsert {
    inner: MutexInner,
    i: usize,
}

impl Target for MutexInsert {
    #[inline(always)]
    fn round(&mut self) {
        let i = self.i;
        self.i += 1;
        let key = make_key(i);
        let mut map = self.inner.lock().unwrap();
        map.insert(key, i);
    }
}

#[derive(Debug, Clone, Default)]
struct LockfreeInsert {
    inner: LockfreeInner,
    i: usize,
}

impl Target for LockfreeInsert {
    #[inline(always)]
    fn round(&mut self) {
        let i = self.i;
        self.i += 1;
        self.inner.insert(make_key(i), i);
    }
}

#[derive(Debug, Clone, Default)]
struct MutexGet {
    inner: MutexInner,
    i: usize,
}

impl Target for MutexGet {
    #[inline(always)]
    fn round(&mut self) {
        let i = self.i;
        self.i += 1;
        let key = make_key(i);
        let map = self.inner.lock().unwrap();
        let val = map.get(&key);
        prevent_opt(val);
    }
}

#[derive(Debug, Clone, Default)]
struct LockfreeGet {
    inner: LockfreeInner,
    i: usize,
}

impl Target for LockfreeGet {
    #[inline(always)]
    fn round(&mut self) {
        let i = self.i;
        self.i += 1;
        prevent_opt(self.inner.get(&make_key(i)));
    }
}

#[derive(Debug, Clone, Default)]
struct MutexRemove {
    inner: MutexInner,
    i: usize,
}

impl Target for MutexRemove {
    #[inline(always)]
    fn round(&mut self) {
        let i = self.i;
        self.i += 1;
        let key = make_key(i);
        let mut map = self.inner.lock().unwrap();
        map.remove(&key);
    }
}

#[derive(Debug, Clone, Default)]
struct LockfreeRemove {
    inner: LockfreeInner,
    i: usize,
}

impl Target for LockfreeRemove {
    #[inline(always)]
    fn round(&mut self) {
        let i = self.i;
        self.i += 1;
        self.inner.remove(&make_key(i));
    }
}

#[derive(Debug, Clone, Default)]
struct MutexMixed {
    inner: MutexInner,
    i: usize,
}

impl Target for MutexMixed {
    #[inline(always)]
    fn round(&mut self) {
        let i = self.i;
        self.i += 1;
        let key = make_key(i);
        let mut map = self.inner.lock().unwrap();
        match map.get(&key) {
            Some(&j) => {
                map.insert(key, i.wrapping_add(j));
                map.remove(&make_key(j));
            },
            None => {
                map.insert(key, i);
            },
        }
    }
}

#[derive(Debug, Clone, Default)]
struct LockfreeMixed {
    inner: LockfreeInner,
    i: usize,
}

impl Target for LockfreeMixed {
    #[inline(always)]
    fn round(&mut self) {
        let i = self.i;
        self.i += 1;
        let key = make_key(i);
        match self.inner.get(&key).map(|guard| *guard.val()) {
            Some(j) => {
                self.inner.insert(key, i.wrapping_add(j));
                self.inner.remove(&make_key(j));
            },
            None => {
                self.inner.insert(key, i);
            },
        }
    }
}

fn main() {
    let mutex = MutexInner::default();
    let lockfree = LockfreeInner::default();

    bench! {
        levels 1, 2, 4, 8;
        "mutex insert" => MutexInsert {
            inner: mutex.clone(),
            i: 0,
        },
        "lockfree insert" => LockfreeInsert {
            inner: lockfree.clone(),
            i: 0
        },
    }

    bench! {
        levels 1, 2, 4, 8;
        "mutex get" => MutexGet {
            inner: mutex.clone(),
            i: 0,
        },
        "lockfree get" => LockfreeGet {
            inner: lockfree.clone(),
            i: 0
        },
    }

    bench! {
        levels 1, 2, 4, 8;
        "mutex remove" => MutexRemove {
            inner: mutex.clone(),
            i: 0,
        },
        "lockfree remove" => LockfreeRemove {
            inner: lockfree.clone(),
            i: 0
        },
    }

    bench! {
        levels 1, 2, 4, 8;
        "mutex mixed" => MutexMixed {
            inner: mutex,
            i: 0,
        },
        "lockfree mixed" => LockfreeMixed {
            inner: lockfree,
            i: 0,
        },
    }
}
