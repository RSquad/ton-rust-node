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
// We need big size buffer for good rand bytes

#![allow(dead_code)]

use openssl::rand::rand_bytes;

#[cfg(test)]
#[path = "tests/test_random.rs"]
mod tests;

/*
pub struct Randbuf {
    buf: Vec<u8>,
    buf_size: usize
}

impl Randbuf {
    pub fn init (value: usize) -> Self {
        Self {
            buf: vec![0 as u8; 0],
            buf_size: value
        }
    }

    fn gen(&mut self) {
        rand_bytes(&mut self.buf).unwrap();
    }

    pub fn secure_bytes (&mut self, mut orig: &mut Vec<u8>, mut size: usize){
        if self.buf.len() == 0 {
            self.buf.resize(self.buf_size, 0 as u8);
            self.gen();
        }
        if orig.len() != size {
            orig.resize(size, 0 as u8);
        }
        let ready = std::cmp::min(size, self.buf.len());
        if size > self.buf_size {
            rand_bytes(&mut orig).unwrap();
            return;
        }
        if ready != 0 {
            orig[..ready - 1].copy_from_slice(&self.buf[self.buf.len() - ready..self.buf.len() - 1]);
            size -= ready;
            self.buf.resize(size, 0 as u8);
            if size == 0 {
                return;
            }
        }
        self.buf.resize(self.buf_size, 0 as u8);
        self.gen();
        orig[ready..].copy_from_slice(&self.buf[self.buf.len() - size..self.buf.len() - 1]);
        return;
    }
}
*/

pub fn secure_bytes(orig: &mut Vec<u8>, size: usize) {
    orig.resize(size, 0);
    rand_bytes(orig).unwrap();
}

pub fn secure_256_bits() -> [u8; 32] {
    let mut buf = [0; 32];
    rand_bytes(&mut buf).unwrap();
    buf
}
