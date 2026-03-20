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
use std::time::Duration;
use ton_block::ShardIdent;

#[test]
pub fn test_collator_validator_telemetry() {
    let shardes =
        [ShardIdent::masterchain(), ShardIdent::with_tagged_prefix(0, 0x4000000000000000).unwrap()];
    let errors = [
        "shard_states_awaiters: timeout",
        "bla bla Key not found bla bla",
        "only validator set with cc_seqno bla bla",
        "bla bla an unregistered chain of length > 8 bla bla",
        "Given last_mc_seq_no 123 is not actual bla bla",
        "other error",
    ];
    let t = CollatorValidatorTelemetry::default();
    for i in 0..1000 {
        for shard in shardes.iter() {
            if i % 53 < errors.len() {
                t.failed_attempt(shard, errors[i % 53]);
            } else {
                t.succeeded_attempt(
                    shard,
                    Duration::from_millis((500 + i % 100) as u64),
                    i as u32,
                    (i * 1000) as u32,
                );
            }
        }
    }
    let r = t.report();
    println!("{}", r);
    assert_eq!(
        r,
        "***
Master chain:
attempts                        1000  100%
total succeeded                  886   89%
longer than 1000ms                 0    0%
total failed                     114   11%
reasons of fail:
    no wait state                 19   17%
    can't get_block               19   17%
    wrong validator set           19   17%
    8 blocks w/a mc commit        19   17%
    given mc isn't actual         19   17%
    other                         19   17%
transactions per block:
       0..100                     88   10%
     100..200                     88   10%
     200..300                     88   10%
     300..400                     88   10%
     400..500                     88   10%
     500..600                     88   10%
     600..700                     88   10%
     700..800                     89   10%
     800..900                     93   10%
     900..                        88   10%
    avg                          502
gas per block:
           0..500000             440   50%
      500000..1000000            446   50%
     1000000..1500000              0    0%
     1500000..2000000              0    0%
     2000000..2500000              0    0%
     2500000..3000000              0    0%
     3000000..3500000              0    0%
     3500000..4000000              0    0%
     4000000..4500000              0    0%
     4500000..                     0    0%
    avg                          502
time, ms (min avg max)           500 549 599
***
Shard chains total:
attempts                        1000  100%
total succeeded                  886   89%
longer than 1000ms                 0    0%
total failed                     114   11%
reasons of fail:
    no wait state                 19   17%
    can't get_block               19   17%
    wrong validator set           19   17%
    8 blocks w/a mc commit        19   17%
    given mc isn't actual         19   17%
    other                         19   17%
transactions per block:
       0..100                     88   10%
     100..200                     88   10%
     200..300                     88   10%
     300..400                     88   10%
     400..500                     88   10%
     500..600                     88   10%
     600..700                     88   10%
     700..800                     89   10%
     800..900                     93   10%
     900..                        88   10%
    avg                          502
gas per block:
           0..500000             440   50%
      500000..1000000            446   50%
     1000000..1500000              0    0%
     1500000..2000000              0    0%
     2000000..2500000              0    0%
     2500000..3000000              0    0%
     3000000..3500000              0    0%
     3500000..4000000              0    0%
     4000000..4500000              0    0%
     4500000..                     0    0%
    avg                          502
time, ms (min avg max)           500 549 599
***
Shard chain 0:4000000000000000:
attempts                        1000  100%
total succeeded                  886   89%
longer than 1000ms                 0    0%
total failed                     114   11%
reasons of fail:
    no wait state                 19   17%
    can't get_block               19   17%
    wrong validator set           19   17%
    8 blocks w/a mc commit        19   17%
    given mc isn't actual         19   17%
    other                         19   17%
transactions per block:
       0..100                     88   10%
     100..200                     88   10%
     200..300                     88   10%
     300..400                     88   10%
     400..500                     88   10%
     500..600                     88   10%
     600..700                     88   10%
     700..800                     89   10%
     800..900                     93   10%
     900..                        88   10%
    avg                          502
gas per block:
           0..500000             440   50%
      500000..1000000            446   50%
     1000000..1500000              0    0%
     1500000..2000000              0    0%
     2000000..2500000              0    0%
     2500000..3000000              0    0%
     3000000..3500000              0    0%
     3500000..4000000              0    0%
     4000000..4500000              0    0%
     4500000..                     0    0%
    avg                          502
time, ms (min avg max)           500 549 599
"
    );
}
