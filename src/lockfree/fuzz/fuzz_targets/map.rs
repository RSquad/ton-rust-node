#![no_main]
#[macro_use]
extern crate libfuzzer_sys;
extern crate fuzzsuite;
extern crate lockfree;

use fuzzsuite::*;
use lockfree::{map::Preview, prelude::*};
use std::{
    hash::{Hash, Hasher},
    sync::Arc,
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct BadHash {
    bytes: Vec<u8>,
}

impl Hash for BadHash {
    fn hash<H>(&self, hasher: &mut H)
    where
        H: Hasher,
    {
        for &byte in self.bytes.iter().step_by(2) {
            hasher.write_u8(byte);
        }
    }
}

impl BadHash {
    fn from_symbol(syms: &[&[u8]]) -> Self {
        let mut res = Vec::new();
        let mut acc = 0;
        for &sym in syms {
            for (i, &byte) in sym.iter().enumerate() {
                if i % sym.len() == 0 {
                    res.push(byte.wrapping_add(acc))
                } else {
                    match byte.wrapping_add(acc) % 8 {
                        0 => acc ^= byte,
                        1 => acc = acc.wrapping_add(byte.wrapping_mul(acc)),
                        2 => acc = acc.wrapping_sub(byte ^ acc >> 4),
                        3 => acc = acc ^ byte >> 4 ^ byte << 4,
                        4 => acc = acc.wrapping_mul(byte),
                        _ => unreachable!(),
                    }
                }
            }
        }
        BadHash { bytes: res }
    }

    fn get(&self, index: usize) -> u8 {
        index
            .checked_rem(self.bytes.len())
            .map(|index| self.bytes[index])
            .unwrap_or(0)
    }
}

#[derive(Debug, Clone, Default)]
struct MapMachine {
    map: Arc<Map<BadHash, u8>>,
    val: u8,
    key0: u8,
    key1: u8,
    key2: u8,
    key3: u8,
}

impl MapMachine {
    fn make_key(&self, bytecode: &mut Bytecode) -> BadHash {
        BadHash::from_symbol(&[
            bytecode.symbol(self.key0),
            bytecode.symbol(self.key1),
            bytecode.symbol(self.key2),
            bytecode.symbol(self.key3),
        ])
    }
}

impl Spawn for MapMachine {
    fn spawn() -> Self {
        Self::default()
    }

    fn fork(&self) -> Self {
        self.clone()
    }
}

impl Machine for MapMachine {
    fn interpret(&mut self, byte: u8, bytecode: &mut Bytecode) {
        match byte % 13 {
            0 => {
                self.val = bytecode.next().unwrap_or(0);
                self.key0 = bytecode.next().unwrap_or(0);
                self.key1 = bytecode.next().unwrap_or(0);
                self.key2 = bytecode.next().unwrap_or(0);
                self.key3 = bytecode.next().unwrap_or(0);
            },

            1 | 5 => {
                let key = self.make_key(bytecode);
                self.map.insert(key, self.val);
            },

            2 => {
                self.key0 = bytecode.next().unwrap_or(0);
                self.key1 = bytecode.next().unwrap_or(0);
                self.key2 = bytecode.next().unwrap_or(0);
                self.key3 = bytecode.next().unwrap_or(0);
                let key = self.make_key(bytecode);
                self.val = self.map.get(&key).map_or(0, |guard| *guard.val());
            },

            3 => {
                let key = self.make_key(bytecode);
                self.val = self.map.get(&key).map_or(0, |guard| *guard.val());
                self.key2 = self.key0;
                self.key0 = bytecode.next().unwrap_or(0);
                self.key3 ^= self.key0;
            },

            4 => {
                let key = self.make_key(bytecode);
                self.val = self.map.get(&key).map_or(0, |guard| *guard.val());
                self.key3 = self.key1;
                self.key1 = bytecode.next().unwrap_or(0);
                self.key2 ^= self.val;
            },

            6 => {
                self.key1 = self.val;
            },

            7 => {
                let key = self.make_key(bytecode);
                let decision = bytecode.next().unwrap_or(0);
                let inc = bytecode.next().unwrap_or(0);
                self.map.insert_with(key, |key, val, stored| {
                    match decision % 8 {
                        0 | 1 => Preview::Discard,
                        2 | 3 => Preview::Keep,
                        4 => Preview::New(
                            inc.wrapping_add(key.get(1))
                                .wrapping_add(stored.map_or(0, |&(_, x)| x))
                                .wrapping_add(val.map_or(0, |x| *x)),
                        ),
                        5 => Preview::New(inc.wrapping_add(key.get(2))),
                        6 => Preview::New(
                            inc.wrapping_add(stored.map_or(0, |&(_, x)| x)),
                        ),
                        7 => Preview::New(
                            key.get(3)
                                .wrapping_add(stored.map_or(0, |&(_, x)| x)),
                        ),
                        _ => unreachable!(),
                    }
                });
            },

            8 => {
                let key = self.make_key(bytecode);
                let removed = match self.map.remove(&key) {
                    Some(x) => x,
                    None => return (),
                };
                let decision = bytecode.next().unwrap_or(0);
                let test = bytecode.next().unwrap_or(0);
                self.map.reinsert_with(removed, |&(_, val), stored| {
                    match decision % 5 {
                        0 => val.wrapping_add(test) % 2 == 0,
                        1 => val.wrapping_mul(test) % 2 == 0,
                        2 => {
                            let res = val.wrapping_mul(
                                test ^ stored.map_or(0, |&(_, x)| x),
                            );
                            res % 2 == 0
                        },
                        3 => stored.is_some(),
                        4 => stored.is_none(),
                        _ => unreachable!(),
                    }
                });
            },

            9 => {
                let mut sum = 0u8;
                for guard in &*self.map {
                    let (k, v) = &*guard;
                    let k = k.get(sum as usize);
                    sum = sum.wrapping_add(k).wrapping_add(*v);
                }
                self.key2 = sum;
            },

            10 => {
                let key = self.make_key(bytecode);
                if let Some(removed) = self.map.remove(&key) {
                    self.map.reinsert(removed);
                }
            },

            11 => {
                let key = self.make_key(bytecode);
                self.map.remove(&key);
            },

            12 => {
                let key = self.make_key(bytecode);
                if let Some(removed) = self.map.remove(&key) {
                    self.map.reinsert_with(removed, |&(_, val), inside| {
                        inside.map_or(val, |&(_, v)| v) % 2 == 1
                    });
                }
            },

            _ => unreachable!(),
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let _ = test::<MapMachine>(Bytecode::new(data));
});
