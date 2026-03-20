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
use catchain::*;

#[test]
fn test_block_update() {
    let block_update = ton::BlockUpdateEvent::default();
    pretty_assertions::assert_eq!(
        format!(
            "{:?}", block_update), 
            "BlockUpdate { block: Block { incarnation: 0000000000000000000000000000000000000000000000000000000000000000, \
            src: 0, height: 0, data: Data { prev: Dep { src: 0, height: 0, \
            data_hash: 0000000000000000000000000000000000000000000000000000000000000000, signature: [] }, \
            deps: [] }, signature: [] } }"
    );
}
