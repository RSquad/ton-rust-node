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
use crate::executor::accounts::{
    SimpleAddress, SortedList, StakeAndFactor, Validator, ValidatorKey
};
use num::BigUint;
use rand::{prelude::SliceRandom, thread_rng};
use ton_block::Grams;
use ton_block::UInt256;

impl Validator {
    fn with_stake(stake: u64) -> Validator {
        let key = ValidatorKey {
            stake: Grams::from(stake),
            time: 0,
            pub_key: UInt256::ZERO,
        };
        Validator {
            key,
            true_stake: 0,
            max_factor: 0,
            addr: SimpleAddress::default(),
            adnl_addr: UInt256::ZERO,
            mc_seq_no_since: 0,
        }
    }
    fn with_params(stake: u64, time: u32, pub_key: [u8; 32]) -> Validator {
        let key = ValidatorKey {
            stake: Grams::from(stake),
            time: !time,
            pub_key: UInt256::with_array(pub_key),
        };
        Validator {
            key,
            true_stake: 0,
            max_factor: 0,
            addr: SimpleAddress::default(),
            adnl_addr: UInt256::ZERO,
            mc_seq_no_since: 0,
        }
    }
}

impl<'a> SortedList<'a> {
    #[cfg(test)]
    fn check_order(&self) -> bool {
        let mut iter = self.list.iter();
        let mut item = iter.next();
        let mut next = iter.next();
        while next.is_some() {
            if item.unwrap().stake > next.unwrap().stake {
                return false;
            }
            item = next;
            next = iter.next();
        }
        true
    }
}

#[test]
fn test_sorted_list() {
    let mut list = SortedList::new();
    let items = [
        (0u32, Validator::with_stake(0)),
        (5, Validator::with_stake(1)),
        (5, Validator::with_stake(2)),
        (7, Validator::with_stake(3)),
    ];
    for (stake, validator) in &items {
        list.insert(StakeAndFactor {
            stake: BigUint::from(*stake),
            validator,
        });
    }
    for item in &list.list {
        println!("{}: {}", item.stake, item.validator.key.stake);
    }
    assert!(list.check_order());
}

#[test]
fn test_sorted_validator_list() {
    let mut items = [
        Validator::with_params(1001, 100, [!0; 32]),
        Validator::with_params(1001, 101, [!1; 32]),
        Validator::with_params(1000, 101, [!2; 32]),
        Validator::with_params(1000, 101, [!3; 32]),
    ];
    items.shuffle(&mut thread_rng());
    items.sort();
    for (i, validator) in items.iter().enumerate() {
        println!(
            "{}: {} {} {}",
            i,
            validator.key.stake,
            validator.key.time,
            validator.key.pub_key.as_slice()[0]
        );
    }
    for (i, validator) in items.iter().enumerate() {
        assert_eq!(!(i as u8), validator.key.pub_key.as_slice()[0]);
    }
}

