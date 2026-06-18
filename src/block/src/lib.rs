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
pub mod types;
pub use types::*;

pub mod cell;
pub use cell::*;

pub mod crypto;
pub use crypto::*;

pub mod dictionary;
pub use dictionary::*;

pub mod boc;
pub use boc::*;
use smallvec::SmallVec;

pub mod wrappers;
pub use wrappers::*;

pub mod bls;
pub use bls::*;

pub mod error;
pub use error::*;

pub mod blocks;
pub use blocks::*;

pub mod shard;
pub use shard::*;

pub mod accounts;
pub use accounts::*;

pub mod messages;
pub use messages::*;

pub mod inbound_messages;
pub use inbound_messages::*;

pub mod master;
pub use master::*;

pub mod envelope_message;
pub use envelope_message::*;

pub mod outbound_messages;
pub use outbound_messages::*;

pub mod shard_accounts;
pub use shard_accounts::*;

pub mod transactions;
pub use transactions::*;

pub mod bintree;
pub use bintree::*;

pub mod out_actions;
pub use out_actions::*;

pub mod merkle_proof;
pub use merkle_proof::*;

pub mod merkle_update;
pub use merkle_update::*;

pub mod validators;
pub use validators::*;

pub mod miscellaneous;
pub use miscellaneous::*;

pub mod signature;
pub use signature::*;

pub mod config_params;
pub use config_params::*;

pub mod storage_stat;
pub use self::storage_stat::*;

pub mod boc_compression;

#[cfg(feature = "xp25")]
pub mod xp25;
#[cfg(feature = "xp25")]
pub use self::xp25::*;
use std::{collections::HashMap, fmt::Debug, hash::Hash};

include!("../../common/src/info.rs");
#[cfg(test)]
include!("./tests/test_utils.rs");

pub trait Mask: Copy + std::ops::BitAnd<Output = Self> + PartialEq {
    fn zero() -> Self;
    fn bits(&self) -> Self;
    fn bit(&self, bits: Self) -> bool {
        (self.bits() & bits) == bits
    }
    fn mask(&self, mask: Self) -> Self {
        self.bits() & mask
    }
    fn any(&self, bits: Self) -> bool {
        (self.bits() & bits) != Self::zero()
    }
    fn non(&self, bits: Self) -> bool {
        (self.bits() & bits) == Self::zero()
    }
}

macro_rules! impl_mask_for {
    ( $( $t:ty ),* ) => {
        $(
            impl Mask for $t {
                fn zero() -> Self {
                    0
                }
                fn bits(&self) -> Self {
                    *self
                }
            }
        )*
    };
}

impl_mask_for!(u8, u64, isize);

pub trait GasConsumer {
    fn finalize_cell(&mut self, builder: BuilderData) -> Result<Cell>;
    fn load_cell(&mut self, cell: Cell) -> Result<SliceData>;
    fn finalize_cell_and_load(&mut self, builder: BuilderData) -> Result<SliceData>;
}

impl GasConsumer for usize {
    fn finalize_cell(&mut self, builder: BuilderData) -> Result<Cell> {
        builder.into_cell()
    }
    fn load_cell(&mut self, cell: Cell) -> Result<SliceData> {
        SliceData::load_cell(cell)
    }
    fn finalize_cell_and_load(&mut self, builder: BuilderData) -> Result<SliceData> {
        SliceData::load_builder(builder)
    }
}

pub fn parse_slice_base(slice: &str, mut bits: usize, base: u32) -> Option<SmallVec<[u8; 128]>> {
    debug_assert!(bits < 8, "it is offset to get slice parsed");
    let mut acc = 0u8;
    let mut data = SmallVec::new();
    let mut completion_tag = false;
    for ch in slice.chars() {
        if completion_tag {
            return None;
        }
        match ch.to_digit(base) {
            Some(x) => {
                if bits < 4 {
                    acc |= (x << (4 - bits)) as u8;
                    bits += 4;
                } else {
                    data.push(acc | (x as u8 >> (bits - 4)));
                    acc = (x << (12 - bits)) as u8;
                    bits -= 4;
                }
            }
            None => match ch {
                '_' => completion_tag = true,
                _ => return None,
            },
        }
    }
    if bits != 0 {
        if !completion_tag {
            acc |= 1 << (7 - bits);
        }
        if acc != 0 || data.is_empty() {
            data.push(acc);
        }
    } else if !completion_tag {
        data.push(0x80);
    }
    Some(data)
}

impl<K, V> Serializable for HashMap<K, V>
where
    K: Clone + Eq + Hash + Default + Deserializable + Serializable,
    V: Serializable,
{
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        let bit_len = K::default().write_to_new_cell()?.length_in_bits();
        let mut dictionary = HashmapE::with_bit_len(bit_len);
        for (key, value) in self.iter() {
            let key = key.write_to_bitstring()?;
            dictionary.set_builder(key, &value.write_to_new_cell()?)?;
        }
        dictionary.write_to(cell)
    }
}

impl<K, V> Deserializable for HashMap<K, V>
where
    K: Eq + Hash + Default + Deserializable + Serializable,
    V: Deserializable + Default,
{
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        let bit_len = K::default().write_to_new_cell()?.length_in_bits();
        let mut dictionary = HashmapE::with_bit_len(bit_len);
        dictionary.read_hashmap_data(slice)?;
        dictionary.iterate_slices(|ref mut key, ref mut value| {
            let key = K::construct_from(key)?;
            let value = V::construct_from(value)?;
            self.insert(key, value);
            Ok(true)
        })?;
        Ok(())
    }
}

pub trait Serializable {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()>;

    fn write_maybe_to(&self, cell: &mut BuilderData, present: bool) -> Result<()> {
        if present {
            cell.append_bit_one()?;
            self.write_to(cell)
        } else {
            cell.append_bit_zero()?;
            Ok(())
        }
    }

    fn write_to_new_cell(&self) -> Result<BuilderData> {
        let mut cell = BuilderData::new();
        self.write_to(&mut cell)?;
        Ok(cell)
    }

    fn write_to_bitstring(&self) -> Result<SliceData> {
        let builder = self.write_to_new_cell()?;
        SliceData::load_bitstring(builder)
    }

    fn write_to_bytes(&self) -> Result<Vec<u8>> {
        let cell = self.serialize()?;
        write_boc(&cell)
    }

    fn write_to_file(&self, file_name: impl AsRef<std::path::Path>) -> Result<()> {
        let bytes = self.write_to_bytes()?;
        match std::fs::write(file_name.as_ref(), bytes) {
            Ok(_) => Ok(()),
            Err(err) => fail!("failed to write to file {}: {}", file_name.as_ref().display(), err),
        }
    }

    fn write_to_base64(&self) -> Result<String> {
        let bytes = self.write_to_bytes()?;
        Ok(base64_encode(&bytes))
    }

    fn serialize(&self) -> Result<Cell> {
        self.write_to_new_cell()?.into_cell()
    }
}

pub trait Deserializable: Default {
    fn construct_from(slice: &mut SliceData) -> Result<Self> {
        let mut x = Self::default();
        x.read_from(slice)?;
        Ok(x)
    }
    fn construct_maybe_from(slice: &mut SliceData) -> Result<Option<Self>> {
        match slice.get_next_bit()? {
            true => Ok(Some(Self::construct_from(slice)?)),
            false => Ok(None),
        }
    }
    fn construct_from_full_cell(cell: Cell) -> Result<Self> {
        let mut slice = SliceData::load_cell(cell)?;
        let obj = Self::construct_from(&mut slice)?;
        if slice.remaining_bits() != 0 || slice.remaining_references() != 0 {
            fail!(
                "cell is not empty after deserializing {}",
                std::any::type_name::<Self>().to_string()
            )
        }
        Ok(obj)
    }
    fn construct_from_cell(cell: Cell) -> Result<Self> {
        Self::construct_from(&mut SliceData::load_cell(cell)?)
    }
    fn construct_from_reference(slice: &mut SliceData) -> Result<Self> {
        Self::construct_from_cell(slice.checked_drain_reference()?)
    }
    /// adapter for tests
    fn construct_from_bytes(bytes: &[u8]) -> Result<Self> {
        Self::construct_from_cell(read_single_root_boc(bytes)?)
    }
    /// adapter for tests
    fn construct_from_file(file_name: impl AsRef<std::path::Path>) -> Result<Self> {
        match std::fs::read(file_name.as_ref()) {
            Ok(bytes) => Self::construct_from_bytes(&bytes),
            Err(err) => fail!("failed to read from file {}: {}", file_name.as_ref().display(), err),
        }
    }
    /// adapter for builder without refs
    fn construct_from_bitstring(builder: BuilderData) -> Result<Self> {
        if builder.references_used() != 0 {
            fail!("references are used in bitstring")
        } else {
            Self::construct_from(&mut SliceData::load_bitstring(builder)?)
        }
    }
    /// adapter for tests
    fn construct_from_base64(string: &str) -> Result<Self> {
        let bytes = base64_decode(string)?;
        Self::construct_from_bytes(&bytes)
    }
    // Override it to implement skipping
    fn skip(slice: &mut SliceData) -> Result<()> {
        Self::construct_from(slice)?;
        Ok(())
    }
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        *self = Self::construct_from(slice)?;
        Ok(())
    }
    fn read_maybe_from(&mut self, slice: &mut SliceData) -> Result<()> {
        if slice.get_next_bit()? {
            self.read_from(slice)
        } else {
            *self = Self::default();
            Ok(())
        }
    }
    fn read_from_cell(&mut self, cell: Cell) -> Result<()> {
        self.read_from(&mut SliceData::load_cell(cell)?)
    }
    // fn read_from_reference(&mut self, slice: &mut SliceData) -> Result<()> {
    //     self.read_from_cell(slice.checked_drain_reference()?)
    // }
    fn invalid_tag(t: u32) -> BlockError {
        let s = std::any::type_name::<Self>().to_string();
        BlockError::InvalidConstructorTag { t, s }
    }
}

impl Serializable for [u8; 32] {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_raw(self, 256)?;
        Ok(())
    }
}

impl Deserializable for [u8; 32] {
    fn construct_from(slice: &mut SliceData) -> Result<Self> {
        slice.get_next_u256()
    }
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        slice.get_next_bytes_to_slice(self)?;
        Ok(())
    }
}

impl Deserializable for Option<Cell> {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        if slice.get_next_bit()? {
            *self = Some(slice.checked_drain_reference()?);
        } else {
            *self = None;
        };
        Ok(())
    }
}

impl Serializable for Option<Cell> {
    fn write_to(&self, builder: &mut BuilderData) -> Result<()> {
        if let Some(cell) = self {
            builder.append_bit_one()?;
            builder.checked_append_reference(cell.clone())?;
        } else {
            builder.append_bit_zero()?;
        }
        Ok(())
    }
}
/* for future use
impl Serializable for SliceData {
    fn write_to_new_cell(&self) -> Result<BuilderData> {
        Ok(BuilderData::from_slice(self))
    }
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.checked_append_references_and_data(self)?;
        Ok(())
    }
}
*/

impl<T: Serializable> Serializable for Option<T> {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        if let Some(x) = self {
            cell.append_bit_one()?;
            x.write_to(cell)?;
        } else {
            cell.append_bit_zero()?;
        }
        Ok(())
    }
}

impl<T: Deserializable> Deserializable for Option<T> {
    fn construct_from(slice: &mut SliceData) -> Result<Self> {
        match slice.get_next_bit_int()? {
            1 => Ok(Some(T::construct_from(slice)?)),
            _ => Ok(None),
        }
    }
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        *self = Self::construct_from(slice)?;
        Ok(())
    }
    fn skip(slice: &mut SliceData) -> Result<()> {
        if slice.get_next_bit()? {
            T::skip(slice)
        } else {
            Ok(())
        }
    }
}

pub trait GetRepresentationHash: Serializable + Debug {
    fn hash(&self) -> Result<UInt256> {
        match self.serialize() {
            Err(err) => {
                log::error!("err: {}, wrong hash calculation for {:?}", err, self);
                Err(err)
            }
            Ok(cell) => Ok(cell.repr_hash().clone()),
        }
    }
}

impl<T: Serializable + Debug> GetRepresentationHash for T {}

impl Deserializable for UInt256 {
    fn construct_from(slice: &mut SliceData) -> Result<Self> {
        slice.get_next_hash()
    }
}

impl Serializable for UInt256 {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_raw(self.as_slice(), 256)?;
        Ok(())
    }
    fn write_to_bitstring(&self) -> Result<SliceData> {
        // it is not optimal
        Ok(AccountId::from(self.as_array()))
    }
}

impl Deserializable for AccountId {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        *self = cell.get_next_slice(256)?;
        Ok(())
    }
}

impl Serializable for AccountId {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        if self.remaining_bits() != 256 {
            fail!("account_id must contain 256 bits, but {}", self.remaining_bits())
        }
        cell.append_bytestring(self)?;
        Ok(())
    }
    fn write_to_bitstring(&self) -> Result<SliceData> {
        if self.remaining_bits() != 256 {
            fail!("account_id must contain 256 bits, but {}", self.remaining_bits())
        }
        Ok(self.clone())
    }
}

#[derive(Clone, Default)]
pub struct EmptyValue;
impl Serializable for EmptyValue {
    fn write_to(&self, _cell: &mut BuilderData) -> Result<()> {
        Ok(())
    }
}
impl Deserializable for EmptyValue {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        if slice.is_empty_cell() {
            Ok(())
        } else {
            fail!(BlockError::InvalidData(format!(
                "It must be True by TLB, but some data is present: {:x}",
                slice
            )))
        }
    }
}
