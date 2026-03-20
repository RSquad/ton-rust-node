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
use crate::rng::random::secure_bytes;

#[test]
fn test_gen_rand() {
    let mut orig: Vec<u8> = Vec::new();
    secure_bytes(&mut orig, 112);
    assert!(orig.len() == 112);
    println!("{:?}", &orig);
    secure_bytes(&mut orig, 630);
    assert!(orig.len() == 630);
    println!("{:?}", &orig);
    secure_bytes(&mut orig, 490);
    assert!(orig.len() == 490);
    println!("{:?}", &orig);
    secure_bytes(&mut orig, 110);
    assert!(orig.len() == 110);
    println!("{:?}", &orig);
}
