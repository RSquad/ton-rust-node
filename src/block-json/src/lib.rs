/*
 * Copyright (C) 2019-2023 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
mod serialize;
pub use self::serialize::*;
mod deserialize;

pub use self::deserialize::*;
#[cfg(test)]
use ton_block::{
    Account, AccountId, AccountStorage, Cell, CurrencyCollection, Deserializable, Message,
    MsgAddressInt, Serializable, SliceData, StateInit, StorageInfo, TickTock,
};

include!("../../common/src/info.rs");
#[cfg(test)]
include!("../../block/src/tests/test_utils.rs");
#[cfg(test)]
include!("./tests/test_common.rs");
