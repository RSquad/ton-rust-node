extern crate lockfree;

use lockfree::{
    map::{Map as LfMap, Preview},
    stack::Stack as LfStack,
};
use std::{
    collections::{hash_map::RandomState, HashMap},
    hash::{BuildHasher, Hash, Hasher},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

fn hash_of(data: &[u8], state: &RandomState) -> u64 {
    let mut hasher = state.build_hasher();
    data.hash(&mut hasher);
    hasher.finish()
}

fn mine_hash(init: &mut Vec<u8>, req: Requirement, state: &RandomState) {
    while hash_of(&init[..], state) % req.divisor != req.remaining {
        let mut i = 0;
        let mut carry = true;

        while carry {
            if i >= init.len() {
                init.push(1);
            } else {
                let (new_val, new_carry) = init[i].overflowing_add(1);
                carry = new_carry;
                init[i] = new_val;
                i += 1;
            }
        }
    }
}

fn measure<S>(nthread: u64, nhash: u64, ndiv: u64) -> Duration
where
    S: Storage + Send + Sync + 'static,
{
    let mut threads = Vec::with_capacity(nthread as usize);

    let storage = Arc::new(S::default());
    let state = RandomState::default();

    let then = Instant::now();

    for i in 0 .. nthread {
        let storage = storage.clone();
        let state = state.clone();
        threads.push(thread::spawn(move || {
            let start = i * nhash / nthread;
            let end = if i + 1 == nthread {
                nhash
            } else {
                (i + 1) * nhash / nthread
            };

            for j in start .. end {
                let mut data = Vec::with_capacity((i + j) as usize);

                for n in 0 .. i + j {
                    data.push(n as u8);
                }

                let divisor = i / ndiv + 1;
                let remaining = j % divisor;
                mine_hash(
                    &mut data,
                    Requirement { divisor, remaining },
                    &state,
                );

                storage.add(Requirement { divisor, remaining }, data.into())
            }
        }))
    }

    for thread in threads {
        thread.join().expect("thread failed");
    }

    let ret = then.elapsed();

    ret
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash, PartialOrd, Ord)]
struct Requirement {
    divisor: u64,
    remaining: u64,
}

trait Storage: Default {
    fn add(&self, req: Requirement, data: Arc<[u8]>);
}

impl Storage for LfMap<Requirement, LfStack<Arc<[u8]>>> {
    fn add(&self, req: Requirement, data: Arc<[u8]>) {
        let mut cache = Some(data);

        self.insert_with(req, |_, _, found| {
            let data = match cache.take() {
                Some(data) => data,
                None => return Preview::Discard,
            };

            match found {
                Some((_, stack)) => {
                    stack.push(data);
                    Preview::Discard
                },

                None => {
                    let stack = LfStack::new();
                    stack.push(data.clone());
                    cache = Some(data);
                    Preview::New(stack)
                },
            }
        });
    }
}

impl Storage for Mutex<HashMap<Requirement, Vec<Arc<[u8]>>>> {
    fn add(&self, req: Requirement, data: Arc<[u8]>) {
        let mut map = self.lock().unwrap();

        map.entry(req).or_insert(Vec::new()).push(data);
    }
}

fn main() {
    println!("A program simulating a hash miner.");

    const NHASH: u64 = 0x2000;

    for &nthread in &[2, 4, 8, 16, 32, 64, 128] {
        println!();

        let mut mutexed = Duration::default();
        let mut lockfree = Duration::default();

        for i in 0x40 .. 0x48 {
            mutexed += measure::<Mutex<_>>(nthread, NHASH, i);
            lockfree += measure::<LfMap<_, _>>(nthread, NHASH, i);
        }

        println!(
            "Mutexed structures with {} threads total time: {:?}",
            nthread, mutexed
        );
        println!(
            "Lockfree structures with {} threads total time: {:?}",
            nthread, lockfree
        );
    }
}
