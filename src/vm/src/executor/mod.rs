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
#[macro_use]
mod microcode;
#[macro_use]
mod engine;
mod blockchain;
mod bls;
mod config;
mod continuation;
mod crypto;
mod currency;
mod deserialization;
mod dictionary;
mod dump;
mod exceptions;
pub mod gas;
mod globals;
mod math;
mod null;
mod rand;
mod serialization;
mod slice_comparison;
mod stack;
mod tuple;
mod types;

pub use engine::*;
use ton_block::{BuilderData, Cell, IBitstring, Result};

#[cfg(test)]
#[path = "../tests/test_executor.rs"]
mod tests;

fn serialize_grams(grams: u128) -> Result<BuilderData> {
    let bytes = 16 - grams.leading_zeros() as usize / 8;
    let mut builder = BuilderData::with_raw(vec![(bytes as u8) << 4], 4)?;
    builder.append_raw(&grams.to_be_bytes()[16 - bytes..], bytes * 8)?;
    Ok(builder)
}

pub fn serialize_currency_collection(grams: u128, other: Option<Cell>) -> Result<BuilderData> {
    let mut builder = serialize_grams(grams)?;
    if let Some(cell) = other {
        builder.append_bit_one()?;
        builder.checked_append_reference(cell)?;
    } else {
        builder.append_bit_zero()?;
    }
    Ok(builder)
}
