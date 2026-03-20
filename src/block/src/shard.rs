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
#[cfg(feature = "xp25")]
use crate::WcExtra;
use crate::{
    accounts::ShardAccount,
    config_params::CatchainConfig,
    define_HashmapE,
    dictionary::hashmapaug::{Augmentation, HashmapAugType},
    envelope_message::FULL_BITS,
    error,
    error::BlockError,
    fail,
    master::{BlkMasterInfo, LibDescr, McStateExtra},
    messages::MsgAddressInt,
    outbound_messages::{OutMsgQueueInfo, OutMsgQueuesInfo},
    shard_accounts::ShardAccounts,
    types::{ChildCell, CurrencyCollection},
    validators::ValidatorSet,
    Account, AccountId, BuilderData, Cell, ConfigParamEnum, Deserializable, IBitstring,
    IntermediateAddress, Result, Serializable, SliceData,
};
use std::{
    any::type_name,
    fmt::{self, Display, Formatter},
};

#[cfg(test)]
#[path = "tests/test_shard.rs"]
mod tests;

pub const MAX_SPLIT_DEPTH: u8 = 60;
pub const MASTERCHAIN_ID: i32 = -1;
pub const BASE_WORKCHAIN_ID: i32 = 0;
pub const INVALID_WORKCHAIN_ID: i32 = 0x8000_0000u32 as i32;
pub const SHARD_FULL: u64 = 0x8000_0000_0000_0000u64;

#[derive(Clone, Eq, PartialEq, Hash)]
pub struct AccountIdPrefixFull {
    pub workchain_id: i32,
    pub prefix: u64,
}

impl Default for AccountIdPrefixFull {
    fn default() -> Self {
        Self { workchain_id: INVALID_WORKCHAIN_ID, prefix: 0 }
    }
}

impl fmt::Debug for AccountIdPrefixFull {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{}:{:016X}", self.workchain_id, self.prefix)
    }
}

impl TryFrom<&AccountIdPrefixFull> for ShardIdent {
    type Error = crate::Error;
    fn try_from(value: &AccountIdPrefixFull) -> std::result::Result<Self, Self::Error> {
        value.shard_ident()
    }
}
impl From<&ShardIdent> for AccountIdPrefixFull {
    fn from(value: &ShardIdent) -> Self {
        value.account_id_prefix()
    }
}

impl AccountIdPrefixFull {
    /// Tests address for validity (workchain_id != 0x80000000)
    pub fn is_valid(&self) -> bool {
        self.workchain_id != INVALID_WORKCHAIN_ID
    }

    pub fn shard_ident(&self) -> Result<ShardIdent> {
        ShardIdent::with_tagged_prefix(
            self.workchain_id,
            self.prefix & (!0 << (64 - MAX_SPLIT_DEPTH)),
        )
    }

    pub fn shard_ident_with_len(&self, len: u8) -> Result<ShardIdent> {
        ShardIdent::with_prefix_len(len, self.workchain_id, self.prefix)
    }

    /// Is address belongs to masterchain (workchain_id == MASTERCHAIN_ID)
    pub fn is_masterchain(&self) -> bool {
        self.workchain_id == MASTERCHAIN_ID
    }

    pub fn workchain_id(&self) -> i32 {
        self.workchain_id
    }

    pub fn shard_key(&self, include_workchain: bool) -> SliceData {
        let mut cell = BuilderData::new();
        if include_workchain {
            cell.append_i32(self.workchain_id).unwrap();
        }
        cell.append_u64(self.prefix).unwrap();
        SliceData::load_bitstring(cell).unwrap()
    }

    /// Constructs AccountIdPrefixFull prefix for specified address.
    /// Returns Err in a case of insufficient bits (less than 64) in the address slice.
    pub fn prefix(address: &MsgAddressInt) -> Result<Self> {
        let (workchain_id, account_id) = address.extract_std_address(true)?;

        Ok(Self { workchain_id, prefix: account_id.get_int(64)? })
    }

    /// Constructs AccountIdPrefixFull prefix for specified address with checking for validity (workchain_id != 0x80000000).
    /// Returns Err in a case of insufficient bits (less than 64) in the address slice or invalid address.
    pub fn checked_prefix(address: &MsgAddressInt) -> Result<Self> {
        let (workchain_id, account_id) = address.extract_std_address(true)?;
        if workchain_id == INVALID_WORKCHAIN_ID {
            fail!("Address is invalid")
        }
        Ok(Self { workchain_id, prefix: account_id.get_int(64)? })
    }

    pub fn std_prefix(workchain_id: i32, account_id: &AccountId) -> Result<Self> {
        let prefix = account_id.get_int(64)?;
        Ok(Self { workchain_id, prefix })
    }

    pub fn any_masterchain() -> Self {
        Self { workchain_id: MASTERCHAIN_ID, prefix: SHARD_FULL }
    }

    pub fn workchain(workchain_id: i32, prefix: u64) -> Self {
        Self { workchain_id, prefix }
    }

    /// Constructs AccountIdPrefixFull prefix for specified address and stores it in the "to" argument.
    /// Returns true if there are sufficient bits in the address (64 or more) and address is valid
    /// (workchain_id != 0x80000000); false otherwise.
    pub fn prefix_to(address: &MsgAddressInt, to: &mut AccountIdPrefixFull) -> bool {
        if let Ok(result) = Self::prefix(address) {
            *to = result;
            return to.is_valid();
        }
        false
    }

    /// Combines dest_bits bits from dest, remaining 64 - dest_bits bits from self
    pub fn interpolate_addr(&self, dest: &Self, dest_bits: u8) -> Self {
        if dest_bits == 0 {
            self.clone()
        } else if dest_bits >= FULL_BITS {
            dest.clone()
        } else if dest_bits >= 32 {
            let mask = u64::MAX >> (dest_bits - 32);
            Self {
                workchain_id: dest.workchain_id,
                prefix: (dest.prefix & !mask) | (self.prefix & mask),
            }
        } else {
            let mask = u32::MAX >> dest_bits;
            Self {
                workchain_id: (dest.workchain_id & (!mask as i32))
                    | (self.workchain_id & (mask as i32)),
                prefix: self.prefix,
            }
        }
    }

    /// Combines count bits from dest, remaining 64 - count bits from self
    /// (using count from IntermediateAddress::Regular)
    pub fn interpolate_addr_intermediate(
        &self,
        dest: &Self,
        ia: &IntermediateAddress,
    ) -> Result<Self> {
        if let IntermediateAddress::Regular(regular) = ia {
            Ok(self.interpolate_addr(dest, regular.use_dest_bits()))
        } else {
            fail!("IntermediateAddress::Regular is expected")
        }
    }

    /// Returns count of the first bits matched in both addresses
    pub fn count_matching_bits(&self, other: &Self) -> u8 {
        if self.workchain_id != other.workchain_id {
            (self.workchain_id ^ other.workchain_id).leading_zeros() as u8
        } else if self.prefix != other.prefix {
            32 + (self.prefix ^ other.prefix).leading_zeros() as u8
        } else {
            96
        }
    }

    /// Performs Hypercube Routing from self to dest address.
    /// Result: (transit_addr_dest_bits, nh_addr_dest_bits)
    #[allow(clippy::many_single_char_names)]
    pub fn perform_hypercube_routing(
        &self,
        dest: &AccountIdPrefixFull,
        cur_shard: &ShardIdent,
        ia: IntermediateAddress,
    ) -> Result<(IntermediateAddress, IntermediateAddress)> {
        let transit = self.interpolate_addr_intermediate(dest, &ia)?;
        if !cur_shard.contains_full_prefix(&transit) {
            fail!("Shard {} must fully contain transit prefix {}", cur_shard, transit)
        }

        if cur_shard.contains_full_prefix(dest) {
            // If destination is in this shard, set cur:=next_hop:=dest
            return Ok((IntermediateAddress::full_dest(), IntermediateAddress::full_dest()));
        }

        if transit.is_masterchain() || dest.is_masterchain() {
            // Route messages to/from masterchain directly
            return Ok((ia, IntermediateAddress::full_dest()));
        }

        if transit.workchain_id != dest.workchain_id {
            return Ok((ia, IntermediateAddress::use_dest_bits(32)?));
        }

        let x = cur_shard.prefix & (cur_shard.prefix - 1);
        let y = cur_shard.prefix | (cur_shard.prefix - 1);
        let t = transit.prefix;
        let q = dest.prefix ^ t;
        // Top i bits match, next 4 bits differ:
        let mut i = q.leading_zeros() as u8 & 0xFC;
        let mut m = u64::MAX >> i;
        loop {
            m >>= 4;
            let h = t ^ (q & !m);
            i += 4;
            if h < x || h > y {
                let cur_prefix = IntermediateAddress::use_dest_bits(28 + i)?;
                let next_prefix = IntermediateAddress::use_dest_bits(32 + i)?;
                return Ok((cur_prefix, next_prefix));
            }
        }
    }
}

impl fmt::Display for AccountIdPrefixFull {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}:{:016X}", self.workchain_id, self.prefix)
    }
}

/*
shard_ident$00
    shard_pfx_bits: (#<= 60)
    workchain_id: int32
    shard_prefix: uint64
= ShardIdent;
*/
#[derive(Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct ShardIdent {
    workchain_id: i32,
    prefix: u64, // with terminated bit!
}

impl Default for ShardIdent {
    fn default() -> Self {
        ShardIdent { workchain_id: 0, prefix: SHARD_FULL }
    }
}

impl ShardIdent {
    pub const MASTERCHAIN: Self = Self::masterchain();
    pub const BASE_WORKCHAIN: Self = Self::full(BASE_WORKCHAIN_ID);

    pub const fn masterchain() -> Self {
        ShardIdent { workchain_id: MASTERCHAIN_ID, prefix: SHARD_FULL }
    }
    pub const fn full(workchain_id: i32) -> Self {
        ShardIdent { workchain_id, prefix: SHARD_FULL }
    }
    pub fn with_prefix_len(
        shard_pfx_len: u8,
        workchain_id: i32,
        shard_prefix: u64,
    ) -> Result<Self> {
        if shard_pfx_len > MAX_SPLIT_DEPTH {
            fail!(BlockError::InvalidArg(format!(
                "Shard prefix length can't greater than {}",
                MAX_SPLIT_DEPTH
            )))
        }
        Self::check_workchain_id(workchain_id)?;
        Ok(ShardIdent { workchain_id, prefix: Self::add_tag(shard_prefix, shard_pfx_len) })
    }

    pub fn with_tagged_prefix(workchain_id: i32, shard_prefix_tagged: u64) -> Result<Self> {
        if (shard_prefix_tagged & (!0 >> (MAX_SPLIT_DEPTH + 1))) != 0 {
            fail!(BlockError::InvalidArg(format!(
                "Shard prefix {:16X} cannot be longer than {}",
                shard_prefix_tagged, MAX_SPLIT_DEPTH
            )))
        }
        Self::check_workchain_id(workchain_id)?;
        Ok(ShardIdent { workchain_id, prefix: shard_prefix_tagged })
    }

    pub fn with_prefix_slice(workchain_id: i32, mut shard_prefix_slice: SliceData) -> Result<Self> {
        let mut shard_pfx_bits = 0;
        let mut shard_prefix = 0;
        while let Some(bit) = shard_prefix_slice.get_next_bit_opt() {
            shard_pfx_bits += 1;
            shard_prefix |= (bit as u64) << (64 - shard_pfx_bits)
        }
        if shard_pfx_bits > MAX_SPLIT_DEPTH {
            fail!(BlockError::InvalidArg(format!(
                "Shard prefix length can't greater than {}",
                MAX_SPLIT_DEPTH
            )))
        }
        Self::check_workchain_id(workchain_id)?;
        Ok(ShardIdent { workchain_id, prefix: Self::add_tag(shard_prefix, shard_pfx_bits) })
    }

    pub fn with_workchain_id(workchain_id: i32) -> Result<Self> {
        Self::check_workchain_id(workchain_id)?;
        Ok(Self { workchain_id, prefix: SHARD_FULL })
    }

    pub fn relative_with_len(&self, prefix_len: u8) -> Result<Self> {
        Self::with_prefix_len(prefix_len, self.workchain_id(), self.shard_prefix_with_tag())
    }

    pub fn check_workchain_id(workchain_id: i32) -> Result<()> {
        if workchain_id == INVALID_WORKCHAIN_ID {
            fail!(BlockError::InvalidArg(format!(
                "Workchain id 0x{:x} is invalid",
                INVALID_WORKCHAIN_ID
            )))
        }
        Ok(())
    }

    /// Get bitstring-key for BinTree operation for Shard
    pub fn shard_key(&self, include_workchain: bool) -> SliceData {
        let mut cell = BuilderData::new();
        if include_workchain {
            cell.append_i32(self.workchain_id).unwrap();
        }
        if self.shard_prefix_with_tag() != SHARD_FULL {
            let prefix_len = self.prefix_len();
            let prefix = self.shard_prefix_with_tag() >> (64 - prefix_len);
            cell.append_bits(prefix as usize, prefix_len as usize).unwrap();
        }
        SliceData::load_bitstring(cell).unwrap()
    }

    /// Get bitstring-key for BinTree operation for Shard
    pub fn parent_shard_key(&self, include_workchain: bool) -> SliceData {
        let mut cell = BuilderData::new();
        if include_workchain {
            cell.append_i32(self.workchain_id).unwrap();
        }
        if self.shard_prefix_with_tag() != SHARD_FULL {
            let prefix_len = self.prefix_len() - 1;
            let prefix = self.shard_prefix_with_tag() >> (64 - prefix_len);
            cell.append_bits(prefix as usize, prefix_len as usize).unwrap();
        }
        SliceData::load_bitstring(cell).unwrap()
    }

    /// Get bitstring-key for BinTree operation for Shard
    pub fn full_key(&self) -> Result<SliceData> {
        let mut cell = BuilderData::new();
        cell.append_i32(self.workchain_id)?.append_u64(self.shard_prefix_without_tag())?;
        SliceData::load_bitstring(cell)
    }

    pub fn full_key_with_tag(&self) -> Result<SliceData> {
        let mut cell = BuilderData::new();
        cell.append_i32(self.workchain_id)?.append_u64(self.shard_prefix_with_tag())?;
        SliceData::load_bitstring(cell)
    }

    pub fn workchain_id(&self) -> i32 {
        self.workchain_id
    }

    pub fn is_child_for(&self, parent: &ShardIdent) -> bool {
        parent.is_parent_for(self)
    }

    pub fn is_parent_for(&self, child: &ShardIdent) -> bool {
        if child.is_full() {
            return false;
        }
        let parent = child.merge();
        self.workchain_id() == child.workchain_id()
            && parent.is_ok()
            && self.shard_prefix_with_tag() == parent.unwrap().shard_prefix_with_tag()
    }

    pub fn is_left_child(&self) -> bool {
        !self.is_right_child()
    }
    pub fn is_right_child(&self) -> bool {
        (self.prefix & (self.prefix_lower_bits() << 1)) != 0
    }

    pub fn is_ancestor_for(&self, descendant: &ShardIdent) -> bool {
        self.workchain_id() == descendant.workchain_id()
            && Self::is_ancestor(self.prefix, descendant.prefix)
    }

    // returns all 0 and first 1 from right to left
    // i.e. 1010000 -> 10000
    pub fn lower_bits(prefix: u64) -> u64 {
        prefix & Self::negate_bits(prefix)
    }

    pub fn negate_bits(prefix: u64) -> u64 {
        (!prefix).wrapping_add(1)
    }

    // pub fn is_ancestor_prefix(prefix: u64, descendant: u64) -> bool {
    //     prefix == SHARD_FULL ||
    //         ((descendant & !((Self::lower_bits(prefix) << 1) - 1)) == prefix - Self::lower_bits(prefix))
    // }

    pub fn contains(parent: u64, child: u64) -> bool {
        let x = Self::lower_bits(parent);
        ((parent ^ child) & (Self::negate_bits(x) << 1)) == 0
    }

    pub fn is_ancestor(parent: u64, child: u64) -> bool {
        let x = Self::lower_bits(parent);
        let y = Self::lower_bits(child);
        x >= y && ((parent ^ child) & (Self::negate_bits(x) << 1)) == 0
    }

    pub fn intersect_with(&self, other: &Self) -> bool {
        if self.workchain_id != other.workchain_id {
            return false;
        }
        Self::shard_intersects(self.shard_prefix_with_tag(), other.shard_prefix_with_tag())
    }

    pub fn account_id_prefix(&self) -> AccountIdPrefixFull {
        AccountIdPrefixFull {
            workchain_id: self.workchain_id,
            prefix: self.shard_prefix_without_tag(),
        }
    }

    // 1 =           10010    11000    11100    11100    01100
    // 2 =           01100    00110    01110    11110    01110
    // z =           00100    01000    00100    00100    00100
    // !z =          11011    10111    11011    11011    11011
    // !z + 1 =      11100    11000    11100    11100    11100
    // !z + 1 << 1 = 11000    10000    11000    11000    11000
    // x =           11110    11110    10010    00010    00010
    // r =           11000    10000    10000    00000    00000
    /// cheks if one shard fully includes other
    pub fn shard_intersects(x: u64, y: u64) -> bool {
        let z = std::cmp::max(Self::lower_bits(x), Self::lower_bits(y));
        let z = Self::negate_bits(z) << 1;
        let x = x ^ y;
        x & z == 0
    }

    pub fn shard_intersection(x: u64, y: u64) -> u64 {
        match Self::lower_bits(x) < Self::lower_bits(y) {
            true => x,
            false => y,
        }
    }

    /// It is copy from t-node. TODO: investigate, add comment and tests
    pub fn is_neighbor_for(&self, other: &Self) -> bool {
        if self.is_masterchain() || other.is_masterchain() {
            return true;
        }
        let xs = self.shard_prefix_with_tag();
        let ys = other.shard_prefix_with_tag();
        let xl = self.prefix_lower_bits();
        let yl = other.prefix_lower_bits();
        let z = (xs ^ ys) & Self::negate_bits(std::cmp::max(xl, yl) << 1);
        if z == 0 {
            return true;
        }
        if self.workchain_id() != other.workchain_id() {
            return false;
        }
        let c1 = z.leading_zeros() >> 2;
        let c2 = z.trailing_zeros() >> 2;
        c1 + c2 == 15
    }

    pub fn can_split(&self) -> bool {
        self.prefix_len() < MAX_SPLIT_DEPTH
    }

    pub fn is_full(&self) -> bool {
        self.prefix == SHARD_FULL
    }

    pub fn is_masterchain(&self) -> bool {
        self.workchain_id == MASTERCHAIN_ID
    }

    pub fn is_masterchain_ext(&self) -> bool {
        self.is_masterchain() && self.is_full()
    }

    pub fn is_base_workchain(&self) -> bool {
        self.workchain_id() == BASE_WORKCHAIN_ID
    }

    pub fn is_standard_workchain(&self) -> bool {
        self.workchain_id() >= BASE_WORKCHAIN_ID && self.workchain_id() <= 255
    }

    pub fn contains_address(&self, addr: &MsgAddressInt) -> Result<bool> {
        Ok(self.workchain_id == addr.workchain_id() && self.contains_account(addr.address())?)
    }
    pub fn contains_account(&self, acc_addr: &AccountId) -> Result<bool> {
        Ok(if self.prefix == SHARD_FULL {
            true
        } else {
            // compare shard prefix and first bits of address
            // (take as many bits of the address as the bits in the prefix)
            let len = self.prefix_len();
            let addr_pfx = acc_addr.get_int(len as usize)?;
            let shard_pfx = self.prefix >> (64 - len);
            addr_pfx == shard_pfx
        })
    }

    pub fn contains_full_prefix(&self, prefix: &AccountIdPrefixFull) -> bool {
        self.contains_prefix(prefix.workchain_id, prefix.prefix)
    }

    pub fn contains_prefix(&self, workchain_id: i32, prefix_without_tag: u64) -> bool {
        if self.workchain_id == workchain_id {
            if self.prefix == SHARD_FULL {
                return true;
            }
            let shift = 64 - self.prefix_len();
            return self.prefix >> shift == prefix_without_tag >> shift;
        }
        false
    }

    pub fn shard_prefix_as_str_with_tag(&self) -> String {
        format!("{:016x}", self.shard_prefix_with_tag())
    }

    pub fn shard_prefix_with_tag(&self) -> u64 {
        self.prefix
    }

    pub fn shard_prefix_without_tag(&self) -> u64 {
        self.prefix - self.prefix_lower_bits()
    }

    pub fn sibling(&self) -> ShardIdent {
        let prefix = self.prefix ^ ((self.prefix & Self::negate_bits(self.prefix)) << 1);
        Self { workchain_id: self.workchain_id, prefix }
    }

    pub fn merge(&self) -> Result<ShardIdent> {
        let lb = self.prefix_lower_bits();
        if self.prefix == SHARD_FULL {
            fail!(BlockError::InvalidArg(format!(
                "Can't merge shard {}",
                self.shard_prefix_as_str_with_tag()
            )))
        } else {
            Ok(ShardIdent {
                workchain_id: self.workchain_id,
                prefix: (self.prefix - lb) | (lb << 1),
            })
        }
    }

    pub fn split(&self) -> Result<(ShardIdent, ShardIdent)> {
        let lb = self.prefix_lower_bits() >> 1;
        if lb & (!0 >> (MAX_SPLIT_DEPTH + 1)) != 0 {
            fail!(BlockError::InvalidArg(format!(
                "Can't split shard {}, because of max split depth is {}",
                self.shard_prefix_as_str_with_tag(),
                MAX_SPLIT_DEPTH
            )))
        } else {
            Ok((
                ShardIdent { workchain_id: self.workchain_id, prefix: self.prefix - lb },
                ShardIdent { workchain_id: self.workchain_id, prefix: self.prefix + lb },
            ))
        }
    }

    // TODO: need to check max split first
    pub fn left_ancestor_mask(&self) -> Result<Self> {
        Self::with_tagged_prefix(
            self.workchain_id,
            (self.prefix - 1) & (!0 << (64 - MAX_SPLIT_DEPTH)),
        )
    }

    // TODO: need to check max split first
    pub fn right_ancestor_mask(&self) -> Result<Self> {
        Self::with_tagged_prefix(self.workchain_id, self.prefix + (1 << (64 - MAX_SPLIT_DEPTH)))
    }

    // returns all 0 and first 1 from right to left
    // i.e. 1010000 -> 10000
    fn prefix_lower_bits(&self) -> u64 {
        Self::lower_bits(self.prefix)
    }

    fn add_tag(prefix: u64, len: u8) -> u64 {
        let tag = 1 << (63 - len);
        (prefix & Self::negate_bits(tag)) | tag
    }

    pub fn prefix_len(&self) -> u8 {
        match self.prefix {
            0 => 64,
            prefix => 63 - prefix.trailing_zeros() as u8,
        }
    }
}

impl Display for ShardIdent {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{}:{}", self.workchain_id, self.shard_prefix_as_str_with_tag())
    }
}

impl fmt::Debug for ShardIdent {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{}, {}", self.workchain_id, self.shard_prefix_as_str_with_tag())
    }
}

impl Deserializable for ShardIdent {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        let constructor_and_pfx = cell.get_next_byte()?;
        // check for 2 high bits to be zero
        if constructor_and_pfx & 0xC0 != 0 {
            fail!(BlockError::InvalidData(
                "2 high bits in ShardIdent's first byte have to be zero".to_string()
            ))
        }
        let shard_pfx_bits = constructor_and_pfx & 0x3F;
        if shard_pfx_bits > MAX_SPLIT_DEPTH {
            fail!(BlockError::InvalidArg(format!(
                "Shard prefix bits {} cannot be longer than {}",
                shard_pfx_bits, MAX_SPLIT_DEPTH
            )))
        }
        let workchain_id = cell.get_next_u32()? as i32;
        let shard_prefix = cell.get_next_u64()?;

        *self = Self::with_prefix_len(shard_pfx_bits, workchain_id, shard_prefix)?;

        Ok(())
    }
}

impl Serializable for ShardIdent {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.prefix_len().write_to(cell)?;
        self.workchain_id.write_to(cell)?;
        self.shard_prefix_without_tag().write_to(cell)?;
        Ok(())
    }
}

/*
_ ShardStateUnsplit = ShardState;
split_state#5f327da5
    left:^ShardStateUnsplit
    right:^ShardStateUnsplit
= ShardState;
*/

///
/// Enum ShardState
///
#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum ShardState {
    UnsplitState(ShardStateUnsplit),
    SplitState(ShardStateSplit),
}

impl Default for ShardState {
    fn default() -> Self {
        ShardState::UnsplitState(ShardStateUnsplit::default())
    }
}

impl Deserializable for ShardState {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        let tag = cell.clone().get_next_u32()?;
        *self = match tag {
            SHARD_STATE_UNSPLIT_PFX => {
                let mut ss = ShardStateUnsplit::default();
                ss.read_from(cell)?;
                ShardState::UnsplitState(ss)
            }
            SHARD_STATE_SPLIT_PFX => {
                let mut ss = ShardStateSplit::default();
                ss.read_from(cell)?;
                ShardState::SplitState(ss)
            }
            _ => {
                fail!(BlockError::InvalidConstructorTag { t: tag, s: "ShardState".to_string() })
            }
        };

        Ok(())
    }
}

const SHARD_STATE_SPLIT_PFX: u32 = 0x5f327da5;
const SHARD_STATE_UNSPLIT_PFX: u32 = 0x9023afe2;
#[cfg(feature = "xp25")]
const SHARD_STATE_UNSPLIT_PFX_2: u32 = 0x9023afd4;

impl Serializable for ShardState {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        match self {
            ShardState::UnsplitState(ss) => {
                ss.write_to(cell)?;
            }
            ShardState::SplitState(ss) => {
                ss.write_to(cell)?;
            }
        }
        Ok(())
    }
}

///
/// Struct ShardStateSplit
///
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct ShardStateSplit {
    pub left: Cell,
    pub right: Cell,
}

impl ShardStateSplit {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_left_right(left: Cell, right: Cell) -> Self {
        ShardStateSplit { left, right }
    }
}

impl Deserializable for ShardStateSplit {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        let tag = cell.get_next_u32()?;
        if tag != SHARD_STATE_SPLIT_PFX {
            fail!(BlockError::InvalidConstructorTag { t: tag, s: "ShardStateSplit".to_string() })
        }
        self.left = cell.checked_drain_reference()?;
        self.right = cell.checked_drain_reference()?;
        Ok(())
    }
}

impl Serializable for ShardStateSplit {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_u32(SHARD_STATE_SPLIT_PFX)?;
        cell.checked_append_reference(self.left.clone())?;
        cell.checked_append_reference(self.right.clone())?;
        Ok(())
    }
}

define_HashmapE!(Libraries, 256, LibDescr);

///
/// Struct ShardStateUnsplit
///
// shard_state#9023afe2
//     global_id:int32
//     shard_id:ShardIdent
//     seq_no:uint32
//     vert_seq_no:#
//     gen_utime:uint32
//     gen_lt:uint64
//     min_ref_mc_seqno:uint32
//     out_msg_queue_info:^OutMsgQueueInfo
//     before_split:(## 1)
//     accounts:^ShardAccounts
//     ^[
//         overload_history:uint64
//         underload_history:uint64
//         total_balance:CurrencyCollection
//         total_validator_fees:CurrencyCollection
//         libraries:(HashmapE 256 LibDescr)
//         master_ref:(Maybe BlkMasterInfo)
//     ]
//     custom:(Maybe ^McStateExtra)
// = ShardStateUnsplit;
#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct ShardStateUnsplit {
    global_id: i32,
    shard_id: ShardIdent,
    seq_no: u32,
    vert_seq_no: u32,
    gen_time: u32,
    gen_lt: u64,
    min_ref_mc_seqno: u32,
    out_msg_queues_info: ChildCell<OutMsgQueuesInfo>,
    before_split: bool,
    accounts: ChildCell<ShardAccounts>,
    // next fields in separate cell
    overload_history: u64,
    underload_history: u64,
    total_balance: CurrencyCollection,
    total_validator_fees: CurrencyCollection,
    libraries: Libraries, // currently can be present only in masterchain blocks.
    master_ref: Option<BlkMasterInfo>,

    // This field is present only in the masterchain and contains all the masterchain-specific data.
    custom: Option<ChildCell<McStateExtra>>,

    #[cfg(feature = "xp25")]
    // This field is present only in workchain blocks (in case of xp25 consensus)
    // and contains shard blocks this shardchain block refers to.
    wc_custom: Option<ChildCell<WcExtra>>,
}

impl ShardStateUnsplit {
    pub fn with_ident(shard_id: ShardIdent) -> Self {
        Self { shard_id, ..ShardStateUnsplit::default() }
    }

    pub fn id(&self) -> String {
        format!("shard: {}, seq_no: {}", self.shard(), self.seq_no)
    }

    pub fn global_id(&self) -> i32 {
        self.global_id
    }

    pub fn set_global_id(&mut self, value: i32) {
        self.global_id = value
    }

    pub fn shard(&self) -> &ShardIdent {
        &self.shard_id
    }

    pub fn set_shard(&mut self, shard: ShardIdent) {
        self.shard_id = shard;
    }

    pub fn shard_mut(&mut self) -> &mut ShardIdent {
        &mut self.shard_id
    }

    pub fn seq_no(&self) -> u32 {
        self.seq_no
    }

    pub fn set_seq_no(&mut self, seq_no: u32) {
        assert!(seq_no != 0);
        self.seq_no = seq_no
    }

    pub fn vert_seq_no(&self) -> u32 {
        self.vert_seq_no
    }

    pub fn set_vert_seq_no(&mut self, value: u32) {
        self.vert_seq_no = value
    }

    pub fn gen_time(&self) -> u32 {
        self.gen_time
    }

    pub fn set_gen_time(&mut self, value: u32) {
        self.gen_time = value;
    }

    pub fn gen_lt(&self) -> u64 {
        self.gen_lt
    }

    pub fn set_gen_lt(&mut self, value: u64) {
        self.gen_lt = value
    }

    pub fn min_ref_mc_seqno(&self) -> u32 {
        self.min_ref_mc_seqno
    }

    pub fn set_min_ref_mc_seqno(&mut self, value: u32) {
        self.min_ref_mc_seqno = value
    }

    pub fn out_msg_queue_info_cell(&self) -> Cell {
        self.out_msg_queues_info.cell()
    }

    pub fn read_out_msg_queue_info(&self) -> Result<OutMsgQueueInfo> {
        self.out_msg_queues_info.read_struct().map(|qs| qs.local_queue)
    }

    pub fn write_out_msg_queue_info(&mut self, value: &OutMsgQueueInfo) -> Result<()> {
        self.out_msg_queues_info.write_struct(&OutMsgQueuesInfo::with_local_queue(value.clone()))
    }

    pub fn before_split(&self) -> bool {
        self.before_split
    }

    pub fn set_before_split(&mut self, value: bool) {
        self.before_split = value
    }

    pub fn accounts_cell(&self) -> Cell {
        self.accounts.cell()
    }

    pub fn read_accounts(&self) -> Result<ShardAccounts> {
        self.accounts.read_struct()
    }

    pub fn write_accounts(&mut self, value: &ShardAccounts) -> Result<()> {
        self.accounts.write_struct(value)
    }

    pub fn insert_account(&mut self, account_id: &AccountId, acc: &ShardAccount) -> Result<()> {
        let account = acc.read_account()?;
        let mut accounts = self.read_accounts()?;
        accounts.set(account_id, acc, &account.aug()?)?;
        self.write_accounts(&accounts)
    }

    pub fn replace_accounts_cell(&mut self, cell: Cell) {
        self.accounts = ChildCell::with_cell(cell);
    }

    pub fn overload_history(&self) -> u64 {
        self.overload_history
    }

    pub fn set_overload_history(&mut self, value: u64) {
        self.overload_history = value
    }

    pub fn underload_history(&self) -> u64 {
        self.underload_history
    }

    pub fn set_underload_history(&mut self, value: u64) {
        self.underload_history = value
    }

    pub fn total_balance(&self) -> &CurrencyCollection {
        &self.total_balance
    }

    pub fn set_total_balance(&mut self, total_balance: CurrencyCollection) {
        self.total_balance = total_balance;
    }

    pub fn total_validator_fees(&self) -> &CurrencyCollection {
        &self.total_validator_fees
    }

    pub fn set_total_validator_fees(&mut self, total_validator_fees: CurrencyCollection) {
        self.total_validator_fees = total_validator_fees;
    }

    pub fn libraries(&self) -> &Libraries {
        &self.libraries
    }

    pub fn libraries_mut(&mut self) -> &mut Libraries {
        &mut self.libraries
    }

    pub fn master_ref(&self) -> Option<&BlkMasterInfo> {
        self.master_ref.as_ref()
    }

    pub fn set_master_ref(&mut self, master_ref: Option<BlkMasterInfo>) {
        self.master_ref = master_ref;
    }

    pub fn master_ref_mut(&mut self) -> &mut Option<BlkMasterInfo> {
        &mut self.master_ref
    }

    pub fn custom_cell(&self) -> Option<Cell> {
        self.custom.as_ref().map(|c| c.cell())
    }

    pub fn is_master_state(&self) -> bool {
        self.custom.is_some()
    }

    pub fn read_custom(&self) -> Result<Option<McStateExtra>> {
        self.custom.as_ref().map(|c| c.read_struct()).transpose()
    }

    pub fn write_custom(&mut self, value: Option<&McStateExtra>) -> Result<()> {
        self.custom = value.map(ChildCell::with_struct).transpose()?;
        Ok(())
    }

    #[cfg(feature = "xp25")]
    pub fn wc_custom_cell(&self) -> Option<Cell> {
        self.wc_custom.as_ref().map(|c| c.cell())
    }

    #[cfg(feature = "xp25")]
    pub fn read_wc_custom(&self) -> Result<Option<WcExtra>> {
        self.wc_custom.as_ref().map(|c| c.read_struct()).transpose()
    }

    #[cfg(feature = "xp25")]
    pub fn write_wc_custom(&mut self, value: Option<&WcExtra>) -> Result<()> {
        self.wc_custom = value.map(|v| ChildCell::with_struct(v)).transpose()?;
        Ok(())
    }

    pub fn read_cur_validator_set_and_cc_conf(&self) -> Result<(ValidatorSet, CatchainConfig)> {
        self.read_custom()?
            .ok_or_else(|| {
                error!(BlockError::InvalidArg("State doesn't contain `custom` field".to_string()))
            })?
            .config
            .read_cur_validator_set_and_cc_conf()
    }

    pub fn update_config_param<F>(&mut self, index: u32, update: F) -> Result<()>
    where
        F: FnOnce(&mut Option<ConfigParamEnum>),
    {
        let mut custom =
            self.read_custom()?.ok_or_else(|| error!("masterchain state must contain config"))?;
        custom.config.update_param(index, update)?;
        self.write_custom(Some(&custom))
    }

    fn update_smc_internal(
        &mut self,
        address: &AccountId,
        op: impl FnOnce(&mut Account) -> Result<()>,
    ) -> Result<()> {
        let mut accounts = self.read_accounts()?;
        let mut shard_smc =
            accounts.get(address)?.ok_or_else(|| error!("SMC {:x} isn't present", address))?;
        let mut smc = shard_smc.read_account()?;
        op(&mut smc)?;
        shard_smc.write_account(&smc)?;
        accounts.set(address, &shard_smc, &smc.aug()?)?;
        self.write_accounts(&accounts)
    }

    pub fn update_smc(
        &mut self,
        address: &AccountId,
        code: Option<&Cell>,
        data: Option<&Cell>,
    ) -> Result<()> {
        self.update_smc_internal(address, |smc| {
            if let Some(code) = code {
                smc.set_code(code.clone());
            }
            if let Some(data) = data {
                smc.set_data(data.clone());
            }
            Ok(())
        })
    }

    pub fn update_config_smc(&mut self) -> Result<()> {
        self.update_config_smc_with_code(None)
    }

    pub fn update_config_smc_with_code(&mut self, new_code: Option<Cell>) -> Result<()> {
        let config = self
            .read_custom()?
            .ok_or_else(|| error!("masterchain state must contain config"))?
            .config;
        let address = config.config_address()?;
        self.update_smc_internal(&address, |smc| {
            smc.update_config_smc(&config)?;
            if let Some(new_code) = new_code {
                smc.set_code(new_code);
            }
            smc.update_storage_stat(config.size_limits_config()?.acc_state_cells_for_storage_dict)?;
            Ok(())
        })
    }

    pub fn update_elector_smc(
        &mut self,
        new_code: Option<Cell>,
        new_data: Option<Cell>,
    ) -> Result<()> {
        let config = self
            .read_custom()?
            .ok_or_else(|| error!("masterchain state must contain config"))?
            .config;
        let address = config.elector_address()?;
        self.update_smc_internal(&address, |smc| {
            if let Some(new_code) = new_code {
                smc.set_code(new_code);
            }
            if let Some(new_data) = new_data {
                smc.set_data(new_data);
            }
            Ok(())
        })
    }
}

impl Deserializable for ShardStateUnsplit {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        let tag = slice.get_next_u32()?;
        #[cfg(not(feature = "xp25"))]
        let wrong_tag = tag != SHARD_STATE_UNSPLIT_PFX;
        #[cfg(feature = "xp25")]
        let wrong_tag = tag != SHARD_STATE_UNSPLIT_PFX && tag != SHARD_STATE_UNSPLIT_PFX_2;
        if wrong_tag {
            fail!(BlockError::InvalidConstructorTag { t: tag, s: type_name::<Self>().to_string() })
        }
        self.global_id.read_from(slice)?;
        self.shard_id.read_from(slice)?;
        self.seq_no.read_from(slice)?;
        self.vert_seq_no.read_from(slice)?;
        self.gen_time.read_from(slice)?;
        self.gen_lt.read_from(slice)?;
        self.min_ref_mc_seqno.read_from(slice)?;
        self.out_msg_queues_info.read_from(slice)?;
        self.before_split = slice.get_next_bit()?;
        self.accounts.read_from(slice)?;

        let slice1 = &mut SliceData::load_cell(slice.checked_drain_reference()?)?;
        self.overload_history.read_from(slice1)?;
        self.underload_history.read_from(slice1)?;
        self.total_balance.read_from(slice1)?;
        self.total_validator_fees.read_from(slice1)?;
        self.libraries.read_from(slice1)?;
        self.master_ref.read_from(slice1)?;
        self.custom.read_from(slice)?;
        #[cfg(feature = "xp25")]
        if tag == SHARD_STATE_UNSPLIT_PFX_2 {
            self.wc_custom.read_from(slice)?;
        }
        Ok(())
    }
}

impl Serializable for ShardStateUnsplit {
    fn write_to(&self, builder: &mut BuilderData) -> Result<()> {
        #[cfg(not(feature = "xp25"))]
        let tag = SHARD_STATE_UNSPLIT_PFX;
        #[cfg(feature = "xp25")]
        let tag = SHARD_STATE_UNSPLIT_PFX_2;
        builder.append_u32(tag)?;
        self.global_id.write_to(builder)?;
        self.shard_id.write_to(builder)?;
        self.seq_no.write_to(builder)?;
        self.vert_seq_no.write_to(builder)?;
        self.gen_time.write_to(builder)?;
        self.gen_lt.write_to(builder)?;
        self.min_ref_mc_seqno.write_to(builder)?;
        builder.checked_append_reference(self.out_msg_queues_info.cell())?;
        builder.append_bit_bool(self.before_split)?;

        builder.checked_append_reference(self.accounts.cell())?;

        let mut b2 = BuilderData::new();
        self.overload_history.write_to(&mut b2)?;
        self.underload_history.write_to(&mut b2)?;
        self.total_balance.write_to(&mut b2)?;
        self.total_validator_fees.write_to(&mut b2)?;
        self.libraries.write_to(&mut b2)?;
        self.master_ref.write_to(&mut b2)?;
        builder.checked_append_reference(b2.into_cell()?)?;
        self.custom.write_to(builder)?;
        #[cfg(feature = "xp25")]
        self.wc_custom.write_to(builder)?;
        Ok(())
    }
}
