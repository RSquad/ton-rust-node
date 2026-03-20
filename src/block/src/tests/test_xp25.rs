/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::*;
use crate::write_read_and_assert;

#[test]
fn test_wc_extra() -> Result<()> {
    let mut wc = BinTree::with_item(&ShardBlockRef::default())?;
    wc.split(SliceData::default(), |_| {
        Ok((
            ShardBlockRef {
                seq_no: 123131,
                root_hash: UInt256::rand(),
                file_hash: UInt256::rand(),
                end_lt: 98123045789,
            },
            ShardBlockRef {
                seq_no: 127845,
                root_hash: UInt256::rand(),
                file_hash: UInt256::rand(),
                end_lt: 213489545,
            },
        ))
    })?;
    wc.split(SliceData::new(vec![0b1100_0000]), |_| {
        Ok((
            ShardBlockRef {
                seq_no: 18239475,
                root_hash: UInt256::rand(),
                file_hash: UInt256::rand(),
                end_lt: 981230789,
            },
            ShardBlockRef {
                seq_no: 1278345,
                root_hash: UInt256::rand(),
                file_hash: UInt256::rand(),
                end_lt: 21348957657,
            },
        ))
    })?;

    let mut wc_extra = WcExtra::default();
    wc_extra.ref_shard_blocks.set(&1, &wc)?;

    write_read_and_assert(wc_extra);

    Ok(())
}
