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
use crate::{
    address_crc, base64_decode_url_safe, base64_encode, base64_encode_url_safe,
    blocks::Block,
    define_HashmapE,
    dictionary::hashmapaug::HashmapAugType,
    error, fail,
    merkle_proof::MerkleProof,
    read_boc_root,
    shard::MASTERCHAIN_ID,
    types::{Coins, CurrencyCollection, Number5, Number9},
    AccountId, BlockError, BuilderData, Cell, ChildCell, Deserializable, GasConsumer,
    GetRepresentationHash, IBitstring, Mask, Result, Serializable, SliceData, UInt256, UsageTree,
    VarUInteger16, MAX_DATA_BITS, MAX_REFERENCES_COUNT,
};
use num::FromPrimitive;
use std::{fmt, str::FromStr};

#[cfg(test)]
#[path = "tests/test_messages.rs"]
mod tests;

//////////////////////////////////////////////////////////////////////////////
//
// MessageAddress
//
//

/*
3.1.2. TL-B scheme for addresses. The serialization of source and destination addresses is defined by the following TL-B scheme:
addr_none$00 = MsgAddressExt;
addr_extern$01 len:(## 9) external_address:(len * Bit)
= MsgAddressExt;
anycast_info depth:(## 5) rewrite_pfx:(depth * Bit) = Anycast;
addr_std$10 anycast:(Maybe Anycast)
workchain_id:int8 address:uint256 = MsgAddressInt;
addr_var$11 anycast:(Maybe Anycast) addr_len:(## 9)
workchain_id:int32 address:(addr_len * Bit) = MsgAddressInt;
_ MsgAddressInt = MsgAddress;
_ MsgAddressExt = MsgAddress;
 */

impl AnycastInfo {
    pub fn with_rewrite_pfx(pfx: SliceData) -> Result<Self> {
        Ok(Self { depth: Number5::new(pfx.remaining_bits() as u32)?, rewrite_pfx: pfx })
    }
    pub fn set_rewrite_pfx(&mut self, pfx: SliceData) -> Result<()> {
        self.depth = Number5::new(pfx.remaining_bits() as u32)?;
        self.rewrite_pfx = pfx;
        Ok(())
    }
}

impl Serializable for AnycastInfo {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.depth.write_to(cell)?; // write depth
        cell.checked_append_references_and_data(&self.rewrite_pfx)?; // write rewrite_pfx
        Ok(())
    }
}

impl fmt::Display for AnycastInfo {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "AnycastInfo[pfx {}]", self.rewrite_pfx)
    }
}

/*
addr_std$10 anycast:(Maybe Anycast)
workchain_id:int8 address:uint256 = MsgAddressInt;
addr_var$11 anycast:(Maybe Anycast) addr_len:(## 9)
workchain_id:int32 address:(addr_len * Bit) = MsgAddressInt;
_ MsgAddressInt = MsgAddress;
_ MsgAddressExt = MsgAddress;
 */

impl MsgAddrVar {
    pub fn with_address(
        anycast: Option<AnycastInfo>,
        workchain_id: i32,
        address: SliceData,
    ) -> Result<MsgAddrVar> {
        let addr_len = Number9::new(address.remaining_bits() as u32)?;
        Ok(MsgAddrVar { anycast, addr_len, workchain_id, address })
    }
}

impl Serializable for MsgAddrVar {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.anycast.write_to(cell)?; // anycast
        let addr_len = Number9::new(self.address.remaining_bits() as u32)?;
        addr_len.write_to(cell)?; // addr_len
        cell.append_i32(self.workchain_id)?; // workchain_id
        cell.checked_append_references_and_data(&self.address)?; // address
        Ok(())
    }
}

impl fmt::Display for MsgAddrVar {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if let Some(anycast) = &self.anycast {
            write!(f, "{:x}:", anycast.rewrite_pfx)?;
        }
        if (self.workchain_id / 128 == 0) && (self.address.remaining_bits() == 256) {
            write!(f, "{}:{:x}8_", self.workchain_id, self.address)
        } else {
            write!(f, "{}:{:x}", self.workchain_id, self.address)
        }
    }
}

impl MsgAddrStd {
    pub const fn with_address(
        anycast: Option<AnycastInfo>,
        workchain_id: i8,
        address: AccountId,
    ) -> Self {
        MsgAddrStd { anycast, workchain_id, address }
    }
}

impl Default for MsgAddrStd {
    fn default() -> Self {
        MsgAddrStd::with_address(None, 0, [0; 32].into())
    }
}

impl Serializable for MsgAddrStd {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.anycast.write_to(cell)?; // anycast
        self.workchain_id.write_to(cell)?; // workchain_id
        self.address.write_to(cell)?; // address
        Ok(())
    }
}

impl fmt::Display for MsgAddrStd {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if let Some(anycast) = &self.anycast {
            write!(f, "{:x}:", anycast.rewrite_pfx)?;
        }
        write!(f, "{}:{:x}", self.workchain_id, self.address)
    }
}

impl MsgAddrExt {
    pub fn with_address(address: SliceData) -> Result<Self> {
        if address.remaining_bits() > Number9::get_max_len() {
            fail!(BlockError::InvalidArg("address can't be longer than 2^9-1 bits".to_string()))
        }
        Ok(MsgAddrExt {
            len: Number9::new(address.remaining_bits() as u32)?,
            external_address: address,
        })
    }
}

impl Serializable for MsgAddrExt {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        let len = Number9::new(self.external_address.remaining_bits() as u32)?;
        len.write_to(cell)?; // write len
        cell.checked_append_references_and_data(&self.external_address)?; // write address
        Ok(())
    }
}

impl fmt::Display for MsgAddrExt {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, ":{:x}", self.external_address)
    }
}

impl MsgAddressExt {
    pub fn with_extern(address: SliceData) -> Result<Self> {
        Ok(MsgAddressExt::AddrExtern(MsgAddrExt::with_address(address)?))
    }
}

impl FromStr for MsgAddressExt {
    type Err = crate::Error;
    fn from_str(string: &str) -> Result<Self> {
        match MsgAddress::from_str(string)? {
            MsgAddress::AddrNone => Ok(MsgAddressExt::AddrNone),
            MsgAddress::AddrExt(addr) => Ok(MsgAddressExt::AddrExtern(addr)),
            _ => fail!(BlockError::Other("Wrong type of address".to_string())),
        }
    }
}

impl Serializable for MsgAddressExt {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        match self {
            MsgAddressExt::AddrNone => {
                cell.append_raw(&[0x00], 2)?; // prefix AddrNone
            }
            MsgAddressExt::AddrExtern(ext) => {
                cell.append_raw(&[0x40], 2)?; // prefix AddrExtern
                ext.write_to(cell)?; // MsgAddressExt
            }
        }

        Ok(())
    }
}

impl fmt::Display for MsgAddressExt {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            MsgAddressExt::AddrNone => write!(f, ""),
            MsgAddressExt::AddrExtern(addr) => write!(f, "{}", addr),
        }
    }
}

impl MsgAddress {
    pub fn with_extern(address: SliceData) -> Result<Self> {
        Ok(MsgAddress::AddrExt(MsgAddrExt::with_address(address)?))
    }

    pub fn with_variant(
        anycast: Option<AnycastInfo>,
        workchain_id: i32,
        address: SliceData,
    ) -> Result<Self> {
        Ok(MsgAddress::AddrVar(MsgAddrVar::with_address(anycast, workchain_id, address)?))
    }

    pub fn with_standart(
        anycast: Option<AnycastInfo>,
        workchain_id: i8,
        address: AccountId,
    ) -> Result<Self> {
        Ok(MsgAddress::AddrStd(MsgAddrStd::with_address(anycast, workchain_id, address)))
    }

    pub fn address(&self) -> SliceData {
        match self {
            MsgAddress::AddrNone => SliceData::default(),
            MsgAddress::AddrExt(addr_ext) => addr_ext.external_address.clone(),
            MsgAddress::AddrStd(addr_std) => addr_std.address.clone(),
            MsgAddress::AddrVar(addr_var) => addr_var.address.clone(),
        }
    }

    pub fn get_type(&self) -> u8 {
        match self {
            MsgAddress::AddrNone => 0b00,
            MsgAddress::AddrExt(_) => 0b01,
            MsgAddress::AddrStd(_) => 0b10,
            MsgAddress::AddrVar(_) => 0b11,
        }
    }
}

impl FromStr for MsgAddress {
    type Err = crate::Error;
    fn from_str(string: &str) -> Result<Self> {
        if string.len() == 48 {
            if let Ok(address) = base64_decode_url_safe(string) {
                if address.len() != 36 {
                    fail!("decoded address length {} is not 36 bytes", address.len())
                }
                let crc = address_crc(&address[0..34]);
                if crc.to_be_bytes() != address[34..] {
                    fail!("address crc mismatch")
                }
                let workchain_id = address[1] as i8;
                let address = SliceData::from_raw(&address[2..34], 256);
                return MsgAddress::with_standart(None, workchain_id, address);
            }
        }
        let parts: Vec<&str> = string.split(':').take(4).collect();
        let len = parts.len();
        if len > 3 {
            fail!(BlockError::InvalidArg("too many components in address".to_string()))
        }
        if len == 0 {
            fail!(BlockError::InvalidArg("bad split".to_string()))
        }
        if parts[len - 1].is_empty() {
            if len == 1 {
                return Ok(MsgAddress::AddrNone);
            } else {
                fail!(BlockError::InvalidArg("wrong format".to_string()))
            }
        }
        let address = SliceData::from_string(parts[len - 1])?;
        if len == 2 && parts[0].is_empty() {
            return MsgAddress::with_extern(address);
        }
        let workchain_id = len
            .checked_sub(2)
            .map(|index| parts[index].parse::<i32>())
            .transpose()
            .map_err(|err| {
                BlockError::InvalidArg(format!("workchain_id is not correct number: {}", err))
            })?
            .unwrap_or_default();
        let anycast = len
            .checked_sub(3)
            .map(|index| {
                if parts[index].is_empty() {
                    Err(BlockError::InvalidArg("wrong format".to_string()))
                } else {
                    SliceData::from_string(parts[index]).map_err(|err| {
                        BlockError::InvalidArg(format!("anycast is not correct: {}", err))
                    })
                }
            })
            .transpose()?
            .map(AnycastInfo::with_rewrite_pfx)
            .transpose()
            .map_err(|err| BlockError::InvalidArg(format!("anycast is not correct: {}", err)))?;

        if (-128..128).contains(&workchain_id) {
            if address.remaining_bits() != 256 {
                fail!(BlockError::InvalidArg(format!(
                    "account address should be 256 bits long in workchain {}",
                    workchain_id
                )))
            }
            if parts[len - 1].len() == 64 {
                Ok(MsgAddress::with_standart(anycast, workchain_id as i8, address)?)
            } else {
                Ok(MsgAddress::with_variant(anycast, workchain_id, address)?)
            }
        } else {
            Ok(MsgAddress::with_variant(anycast, workchain_id, address)?)
        }
    }
}

impl fmt::Display for MsgAddress {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            MsgAddress::AddrNone => write!(f, ""),
            MsgAddress::AddrExt(addr) => write!(f, "{}", addr),
            MsgAddress::AddrStd(addr) => write!(f, "{}", addr),
            MsgAddress::AddrVar(addr) => write!(f, "{}", addr),
        }
    }
}

impl Serializable for MsgAddress {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_raw(&[self.get_type() << 6], 2)?;
        match self {
            MsgAddress::AddrNone => (),
            MsgAddress::AddrExt(ext) => ext.write_to(cell)?,
            MsgAddress::AddrStd(std) => std.write_to(cell)?,
            MsgAddress::AddrVar(var) => var.write_to(cell)?,
        }
        Ok(())
    }
}

impl Default for MsgAddressInt {
    fn default() -> Self {
        MsgAddressInt::AddrStd(MsgAddrStd::default())
    }
}

impl FromStr for MsgAddressInt {
    type Err = crate::Error;
    fn from_str(string: &str) -> Result<Self> {
        match MsgAddress::from_str(string)? {
            MsgAddress::AddrStd(addr) => Ok(MsgAddressInt::AddrStd(addr)),
            MsgAddress::AddrVar(addr) => Ok(MsgAddressInt::AddrVar(addr)),
            _ => fail!(BlockError::Other("Wrong type of address".to_string())),
        }
    }
}

pub const ADDR_FORMAT_TESTNET: u8 = 0b0000_0001;
pub const ADDR_FORMAT_BOUNCE: u8 = 0b0000_0010;
pub const ADDR_FORMAT_URL_SAFE: u8 = 0b0000_0100;

impl MsgAddressInt {
    pub fn with_params(workchain_id: i32, account_id: impl Into<AccountId>) -> Result<Self> {
        let account_id = account_id.into();
        if workchain_id < MASTERCHAIN_ID {
            fail!("Wrong workchain id {}", workchain_id)
        } else if workchain_id > 255 || account_id.remaining_bits() != 256 {
            MsgAddressInt::with_variant(None, workchain_id, account_id)
        } else {
            MsgAddressInt::with_standart(None, workchain_id as i8, account_id)
        }
    }
    pub fn with_variant(
        anycast: Option<AnycastInfo>,
        workchain_id: i32,
        address: SliceData,
    ) -> Result<Self> {
        Ok(MsgAddressInt::AddrVar(MsgAddrVar::with_address(anycast, workchain_id, address)?))
    }
    pub fn with_standart(
        anycast: Option<AnycastInfo>,
        workchain_id: i8,
        address: AccountId,
    ) -> Result<Self> {
        Ok(MsgAddressInt::AddrStd(MsgAddrStd::with_address(anycast, workchain_id, address)))
    }
    pub fn standard(workchain_id: i8, address: impl Into<AccountId>) -> Self {
        MsgAddressInt::AddrStd(MsgAddrStd::with_address(None, workchain_id, address.into()))
    }
    pub fn address(&self) -> &AccountId {
        match self {
            MsgAddressInt::AddrStd(addr_std) => &addr_std.address,
            MsgAddressInt::AddrVar(addr_var) => &addr_var.address,
        }
    }
    pub fn workchain_id(&self) -> i32 {
        match self {
            MsgAddressInt::AddrStd(addr_std) => addr_std.workchain_id as i32,
            MsgAddressInt::AddrVar(addr_var) => addr_var.workchain_id,
        }
    }
    pub const fn rewrite_pfx(&self) -> Option<&AnycastInfo> {
        match self {
            MsgAddressInt::AddrStd(addr_std) => addr_std.anycast.as_ref(),
            MsgAddressInt::AddrVar(addr_var) => addr_var.anycast.as_ref(),
        }
    }
    pub fn extract_std_address(&self, do_rewrite: bool) -> Result<(i32, AccountId)> {
        let (workchain_id, mut account_id, anycast_opt) = match self {
            MsgAddressInt::AddrStd(addr_std) => {
                (addr_std.workchain_id as i32, addr_std.address.clone(), &addr_std.anycast)
            }
            MsgAddressInt::AddrVar(addr_var) => {
                (addr_var.workchain_id, addr_var.address.clone(), &addr_var.anycast)
            }
        };

        if let Some(ref anycast) = anycast_opt {
            if do_rewrite {
                account_id.overwrite_prefix(&anycast.rewrite_pfx)?;
            }
        }

        Ok((workchain_id, account_id))
    }

    pub fn is_masterchain(&self) -> bool {
        self.workchain_id() == MASTERCHAIN_ID
    }

    pub fn to_string_custom(
        &self,
        mode: u8, // bits 2: urlsafe, 1: bounce, 0: testnet
    ) -> Result<String> {
        let address = if mode != 0 {
            let mut vec = vec![0; 36];
            let mut flag = 0b00010001;
            if !mode.bit(ADDR_FORMAT_BOUNCE) {
                flag |= 0b0100_0000;
            }
            if mode.bit(ADDR_FORMAT_TESTNET) {
                flag |= 0b1000_0000;
            }
            vec[0] = flag;
            vec[1] = self.workchain_id() as u8;
            self.address().get_bytes_to_slice(&mut vec.as_mut_slice()[2..34])?;
            let crc = address_crc(&vec[0..34]);
            vec[34] = (crc >> 8) as u8;
            vec[35] = (crc & 0xff) as u8;
            if mode.bit(ADDR_FORMAT_URL_SAFE) {
                base64_encode_url_safe(&vec)
            } else {
                base64_encode(&vec)
            }
        } else {
            self.to_string()
        };
        Ok(address)
    }
}

impl Serializable for MsgAddressInt {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        match self {
            MsgAddressInt::AddrStd(std) => {
                cell.append_raw(&[0x80], 2)?; // $10 prefix AddrStd
                std.write_to(cell)?; // MsgAddrStd
            }
            MsgAddressInt::AddrVar(var) => {
                cell.append_raw(&[0xC0], 2)?; // $11 prefix AddrVar
                var.write_to(cell)?; // MsgAddressInt
            }
        }

        Ok(())
    }
}

impl fmt::Display for MsgAddressInt {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            MsgAddressInt::AddrStd(addr) => write!(f, "{}", addr),
            MsgAddressInt::AddrVar(addr) => write!(f, "{}", addr),
        }
    }
}

/*
This file contains definitions for internal and external message headers
as defined in Blockchain: 3.1.

In test_messages.rs and contracts/messages/contract.code there are parsers
for these formats.

Known limitations:
1. For account addreses:
    * we don't serialize the workchain id;
    * anycast is not supported (is supposed to be `nothing`);
    * only standard 256-bit addresses are supported.

2. Instead of CurrencyCollection, Coins type is used.

3. In Message X format, only the info field is parsed.

4. External address is supposed to consist of a whole number of bytes.
*/

impl fmt::Display for InternalMessageHeader {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Internal {{src: {}, dst: {}", self.src, self.dst)?;
        if f.alternate() {
            write!(
                f,
                ", ihr_disabled: {}, bounce: {}, bounced: {}, value: {}, \
                extra_flags: {}, fwd_fee: {}, lt: {}, at: {}",
                self.ihr_disabled,
                self.bounce,
                self.bounced,
                self.value,
                self.extra_flags,
                self.fwd_fee,
                self.created_lt,
                self.created_at
            )?;
        }
        write!(f, "}}")
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum MsgAddressIntOrNone {
    #[default]
    None,
    Some(MsgAddressInt),
}

impl MsgAddressIntOrNone {
    pub fn get_type(&self) -> u8 {
        match self {
            MsgAddressIntOrNone::None => 0b00,
            MsgAddressIntOrNone::Some(addr) => match addr {
                MsgAddressInt::AddrStd(_) => 0b10,
                MsgAddressInt::AddrVar(_) => 0b11,
            },
        }
    }
    pub const fn rewrite_pfx(&self) -> Option<&AnycastInfo> {
        match self {
            MsgAddressIntOrNone::None => None,
            MsgAddressIntOrNone::Some(addr) => addr.rewrite_pfx(),
        }
    }
}

impl fmt::Display for MsgAddressIntOrNone {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            MsgAddressIntOrNone::None => Ok(()),
            MsgAddressIntOrNone::Some(addr) => write!(f, "{addr}"),
        }
    }
}

impl Serializable for MsgAddressIntOrNone {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        match self {
            MsgAddressIntOrNone::None => {
                cell.append_raw(&[0x00], 2)?;
            }
            MsgAddressIntOrNone::Some(addr) => addr.write_to(cell)?,
        }
        Ok(())
    }
}

impl Deserializable for MsgAddressIntOrNone {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        let addr_type = cell.get_next_int(2)? as u8;
        match addr_type & 0b11 {
            0b00 => {
                *self = MsgAddressIntOrNone::None;
            }
            0b10 => {
                let mut std = MsgAddrStd::default();
                std.read_from(cell)?;
                *self = MsgAddressIntOrNone::Some(MsgAddressInt::AddrStd(std));
            }
            0b11 => {
                let mut var = MsgAddrVar::default();
                var.read_from(cell)?;
                *self = MsgAddressIntOrNone::Some(MsgAddressInt::AddrVar(var));
            }
            _ => fail!(BlockError::Other("Wrong type of address".to_string())),
        }
        Ok(())
    }
    fn skip(slice: &mut SliceData) -> Result<()> {
        let addr_type = slice.get_next_int(2)? as u8;
        match addr_type & 0b11 {
            0b00 => Ok(()),
            0b10 => MsgAddrStd::skip(slice),
            0b11 => MsgAddrVar::skip(slice),
            _ => fail!(BlockError::Other("Wrong type of address".to_string())),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InternalMessageHeader {
    pub ihr_disabled: bool,
    pub bounce: bool,
    pub bounced: bool,
    pub src: MsgAddressIntOrNone,
    pub dst: MsgAddressInt,
    pub value: CurrencyCollection,
    pub extra_flags: VarUInteger16,
    pub fwd_fee: Coins,
    pub created_lt: u64,
    pub created_at: u32,
}

impl Default for InternalMessageHeader {
    fn default() -> Self {
        InternalMessageHeader {
            ihr_disabled: true,
            bounce: false,
            bounced: false,
            src: MsgAddressIntOrNone::None,
            dst: MsgAddressInt::default(),
            value: CurrencyCollection::new(),
            extra_flags: VarUInteger16::zero(),
            fwd_fee: Coins::zero(),
            created_lt: 0, // Logical Time will be set on block builder
            created_at: 0, // UNIX time too
        }
    }
}

impl InternalMessageHeader {
    ///
    /// Create new instance of InternalMessageHeader
    /// with source and destination address and value
    ///
    pub fn with_addresses(
        src: MsgAddressInt,
        dst: MsgAddressInt,
        value: CurrencyCollection,
    ) -> Self {
        InternalMessageHeader {
            ihr_disabled: true,
            src: MsgAddressIntOrNone::Some(src),
            dst,
            value,
            ..Default::default()
        }
    }

    pub fn with_addresses_and_bounce(
        src: MsgAddressInt,
        dst: MsgAddressInt,
        value: CurrencyCollection,
        bounce: bool,
    ) -> Self {
        let mut hdr = Self::with_addresses(src, dst, value);
        hdr.bounce = bounce;
        hdr
    }

    ///
    /// Get value tansfered message
    ///
    pub fn value(&self) -> &CurrencyCollection {
        &self.value
    }

    ///
    /// Get forwarding fee for message transfer
    ///
    pub fn fwd_fee(&self) -> &Coins {
        &self.fwd_fee
    }

    pub fn src(&self) -> Result<&MsgAddressInt> {
        self.src_ref().ok_or_else(|| error!("incorrect source address"))
    }
    pub fn src_ref(&self) -> Option<&MsgAddressInt> {
        match self.src {
            MsgAddressIntOrNone::Some(ref addr) => Some(addr),
            MsgAddressIntOrNone::None => None,
        }
    }
    pub fn set_src(&mut self, src: MsgAddressInt) {
        self.src = MsgAddressIntOrNone::Some(src)
    }
    pub fn set_dst(&mut self, dst: MsgAddressInt) {
        self.dst = dst
    }
}

impl Serializable for InternalMessageHeader {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_bit_zero()? //tag
            .append_bit_bool(self.ihr_disabled)?
            .append_bit_bool(self.bounce)?
            .append_bit_bool(self.bounced)?;

        self.src.write_to(cell)?;
        self.dst.write_to(cell)?;

        self.value.write_to(cell)?; //value: CurrencyCollection

        self.extra_flags.write_to(cell)?; //extra_flags

        self.fwd_fee.write_to(cell)?; //fwd_fee

        self.created_lt.write_to(cell)?; //created_lt
        self.created_at.write_to(cell)?; //created_at

        Ok(())
    }
}

impl Deserializable for InternalMessageHeader {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        // constructor tag will be readed in Message
        self.ihr_disabled = cell.get_next_bit()?; // ihr_disabled
        self.bounce = cell.get_next_bit()?; // bounce
        self.bounced = cell.get_next_bit()?;

        self.src.read_from(cell)?; // addr src
        self.dst.read_from(cell)?; // addr dst

        self.value.read_from(cell)?; // value - balance

        self.extra_flags.read_from(cell)?; //extra_flags

        self.fwd_fee.read_from(cell)?; //fwd_fee

        self.created_lt.read_from(cell)?; //created_lt
        self.created_at.read_from(cell)?; //created_at
        Ok(())
    }
    fn skip(slice: &mut SliceData) -> Result<()> {
        slice.move_by(3)?; // ihr_disabled, bounce, bounced
        MsgAddressIntOrNone::skip(slice)?; // addr src
        MsgAddressInt::skip(slice)?; // addr dst
        CurrencyCollection::skip(slice)?; // value - balance
        VarUInteger16::skip(slice)?; //extra_flags
        Coins::skip(slice)?; //fwd_fee
        slice.move_by(64)?; //created_lt
        slice.move_by(32)?; //created_at
        Ok(())
    }
}

impl fmt::Display for ExternalInboundMessageHeader {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "External Inbound {{src: {}, dst: {}, fee: {}}}",
            self.src, self.dst, self.import_fee
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExternalInboundMessageHeader {
    pub src: MsgAddressExt,
    pub dst: MsgAddressInt,
    pub import_fee: Coins,
}

impl ExternalInboundMessageHeader {
    pub const fn new(src: MsgAddressExt, dst: MsgAddressInt) -> Self {
        let import_fee = Coins::zero();
        Self { src, dst, import_fee }
    }
}

impl Serializable for ExternalInboundMessageHeader {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_bit_one()?.append_bit_zero()?;

        self.src.write_to(cell)?; // addr src
        self.dst.write_to(cell)?; // addr dst
        self.import_fee.write_to(cell)?; //ihr_fee

        Ok(())
    }
}

impl Deserializable for ExternalInboundMessageHeader {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        // constructor tag will be readed in Message
        self.src.read_from(cell)?; // addr src
        self.dst.read_from(cell)?; // addr dst
        self.import_fee.read_from(cell)?; //ihr_fee
        Ok(())
    }
}

impl fmt::Display for ExtOutMessageHeader {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "External Outbound {{src: {}, dst: {}, lt: {}, at: {}}}",
            self.src, self.dst, self.created_lt, self.created_at
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtOutMessageHeader {
    pub src: MsgAddressIntOrNone,
    pub dst: MsgAddressExt,
    pub created_lt: u64,
    pub created_at: u32,
}

impl ExtOutMessageHeader {
    pub fn with_addresses(src: MsgAddressInt, dst: MsgAddressExt) -> ExtOutMessageHeader {
        ExtOutMessageHeader {
            src: MsgAddressIntOrNone::Some(src),
            dst,
            created_lt: 0, // Logical Time will be set on block builder
            created_at: 0, // UNIX time too
        }
    }
    pub fn src(&self) -> Option<&MsgAddressInt> {
        match self.src {
            MsgAddressIntOrNone::Some(ref src) => Some(src),
            MsgAddressIntOrNone::None => None,
        }
    }
    pub fn set_src(&mut self, src: MsgAddressInt) {
        self.src = MsgAddressIntOrNone::Some(src);
    }
}

impl Serializable for ExtOutMessageHeader {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_bit_one()?.append_bit_one()?;

        self.src.write_to(cell)?; // addr src
        self.dst.write_to(cell)?; // addr dst
        self.created_lt.write_to(cell)?; //created_lt
        self.created_at.write_to(cell)?; //created_at

        Ok(())
    }
}

impl Deserializable for ExtOutMessageHeader {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        // constructor tag will be readed in Message
        self.src.read_from(cell)?; // addr src
        self.dst.read_from(cell)?; // addr dst
        self.created_lt.read_from(cell)?; //created_lt
        self.created_at.read_from(cell)?; //created_at
        Ok(())
    }
}

////////////////////////////////////////////////////////////////////////////////////////////////////
///
/// int_msg_info$0 ihr_disabled:Bool bounce:Bool
/// src:MsgAddressInt dest:MsgAddressInt
/// value:CurrencyCollection ihr_fee:Coins fwd_fee:Coins
/// created_lt:uint64 created_at:uint32 = CommonMsgInfo;
/// ext_in_msg_info$10 src:MsgAddressExt dest:MsgAddressInt
/// import_fee:Coins = CommonMsgInfo;
/// ext_out_msg_info$11 src:MsgAddressInt dest:MsgAddressExt
/// created_lt:uint64 created_at:uint32 = CommonMsgInfo;
///
impl fmt::Display for CommonMsgInfo {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            CommonMsgInfo::IntMsgInfo(hdr) => write!(f, "{}", hdr),
            CommonMsgInfo::ExtInMsgInfo(hdr) => write!(f, "{}", hdr),
            CommonMsgInfo::ExtOutMsgInfo(hdr) => write!(f, "{}", hdr),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum CommonMsgInfo {
    IntMsgInfo(InternalMessageHeader),
    ExtInMsgInfo(ExternalInboundMessageHeader),
    ExtOutMsgInfo(ExtOutMessageHeader),
}

impl CommonMsgInfo {
    ///
    /// Get destination account address
    ///
    pub fn dest_account_address(&self) -> Option<AccountId> {
        match self {
            CommonMsgInfo::IntMsgInfo(header) => {
                match header.dst {
                    MsgAddressInt::AddrStd(ref std) => Some(std.address.clone()),
                    MsgAddressInt::AddrVar(ref _var) => unimplemented!(), // TODO
                }
            }
            CommonMsgInfo::ExtInMsgInfo(header) => {
                match header.dst {
                    MsgAddressInt::AddrStd(ref std) => Some(std.address.clone()),
                    MsgAddressInt::AddrVar(ref _var) => unimplemented!(), // TODO
                }
            }
            _ => None,
        }
    }

    pub fn dest_wc(&self) -> Option<i32> {
        match self {
            CommonMsgInfo::IntMsgInfo(header) => match header.dst {
                MsgAddressInt::AddrStd(ref std) => Some(std.workchain_id as i32),
                MsgAddressInt::AddrVar(ref var) => Some(var.workchain_id),
            },
            CommonMsgInfo::ExtInMsgInfo(header) => match header.dst {
                MsgAddressInt::AddrStd(ref std) => Some(std.workchain_id as i32),
                MsgAddressInt::AddrVar(ref var) => Some(var.workchain_id),
            },
            _ => None,
        }
    }

    ///
    /// Get value transmitted by the value
    /// Value can be transmitted only internal messages
    /// For other types of messages, function returned None
    ///
    pub fn get_value(&self) -> Option<&CurrencyCollection> {
        match self {
            CommonMsgInfo::IntMsgInfo(header) => Some(&header.value),
            _ => None,
        }
    }

    pub fn get_value_mut(&mut self) -> Option<&mut CurrencyCollection> {
        match self {
            CommonMsgInfo::IntMsgInfo(header) => Some(&mut header.value),
            _ => None,
        }
    }

    ///
    /// Get message header fees
    /// Fee collected only for transfer internal and external outbound messages.
    /// for other types of messages, function returned None
    ///
    pub fn fee(&self) -> Result<Option<Coins>> {
        match self {
            CommonMsgInfo::IntMsgInfo(header) => Ok(Some(header.fwd_fee)),
            CommonMsgInfo::ExtInMsgInfo(header) => Ok(Some(header.import_fee)),
            _ => Ok(None),
        }
    }

    ///
    /// Get dest address for Intrenal and Inbound external messages
    ///
    pub fn get_dst_address(&self) -> Option<MsgAddressInt> {
        match self {
            CommonMsgInfo::IntMsgInfo(header) => Some(header.dst.clone()),
            CommonMsgInfo::ExtInMsgInfo(header) => Some(header.dst.clone()),
            _ => None,
        }
    }
}

impl Default for CommonMsgInfo {
    fn default() -> Self {
        CommonMsgInfo::IntMsgInfo(InternalMessageHeader::default())
    }
}

impl Serializable for CommonMsgInfo {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        match self {
            CommonMsgInfo::IntMsgInfo(header) => header.write_to(cell)?,
            CommonMsgInfo::ExtInMsgInfo(header) => header.write_to(cell)?,
            CommonMsgInfo::ExtOutMsgInfo(header) => header.write_to(cell)?,
        }
        Ok(())
    }
}

impl Deserializable for CommonMsgInfo {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        *self = if !cell.get_next_bit()? {
            let mut int_msg = InternalMessageHeader::default();
            int_msg.read_from(cell)?;
            CommonMsgInfo::IntMsgInfo(int_msg)
        } else if !cell.get_next_bit()? {
            let mut ext_in_msg = ExternalInboundMessageHeader::default();
            ext_in_msg.read_from(cell)?;
            CommonMsgInfo::ExtInMsgInfo(ext_in_msg)
        } else {
            let mut ext_out_ms = ExtOutMessageHeader::default();
            ext_out_ms.read_from(cell)?;
            CommonMsgInfo::ExtOutMsgInfo(ext_out_ms)
        };

        Ok(())
    }
    fn skip(slice: &mut SliceData) -> Result<()> {
        if !slice.get_next_bit()? {
            InternalMessageHeader::skip(slice)?;
        } else if !slice.get_next_bit()? {
            ExternalInboundMessageHeader::skip(slice)?;
        } else {
            ExtOutMessageHeader::skip(slice)?;
        }
        Ok(())
    }
}

pub type MessageId = UInt256;

///////////////////////////////////////////////////////////////////////////////////////////
///
/// message$_ {X:Type} info:CommonMsgInfo
/// init:(Maybe (Either StateInit ^StateInit))
/// body:(Either X ^X) = Message X;
///
///

#[derive(Debug, Default, Clone, Eq)]
pub struct Message {
    header: CommonMsgInfo,
    init: Option<StateInit>,
    body: Option<SliceData>,
    body_to_ref: Option<bool>,
    init_to_ref: Option<bool>,
}

impl fmt::Display for Message {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Message {{header: {}", self.header)?;
        match &self.init {
            Some(init) => write!(f, ", init to ref: {:?}, {:?}", self.init_to_ref, init)?,
            None => write!(f, ", init: None")?,
        }
        match &self.body {
            Some(body) => write!(f, ", body to ref: {:?}, {:x}", self.body_to_ref, body)?,
            None => write!(f, ", body: None")?,
        }
        write!(f, "}}")
    }
}

impl PartialEq for Message {
    fn eq(&self, other: &Message) -> bool {
        self.header == other.header && self.init == other.init && self.body == other.body
    }
}

impl Message {
    pub fn int_header(&self) -> Option<&InternalMessageHeader> {
        match self.header() {
            CommonMsgInfo::IntMsgInfo(header) => Some(header),
            _ => None,
        }
    }

    pub fn ext_in_header(&self) -> Option<&ExternalInboundMessageHeader> {
        match self.header() {
            CommonMsgInfo::ExtInMsgInfo(header) => Some(header),
            _ => None,
        }
    }

    pub fn ext_out_header(&self) -> Option<&ExtOutMessageHeader> {
        match self.header() {
            CommonMsgInfo::ExtOutMsgInfo(header) => Some(header),
            _ => None,
        }
    }

    pub fn int_header_mut(&mut self) -> Option<&mut InternalMessageHeader> {
        match self.header {
            CommonMsgInfo::IntMsgInfo(ref mut header) => Some(header),
            _ => None,
        }
    }

    pub fn ext_in_header_mut(&mut self) -> Option<&mut ExternalInboundMessageHeader> {
        match self.header {
            CommonMsgInfo::ExtInMsgInfo(ref mut header) => Some(header),
            _ => None,
        }
    }

    ///
    /// Create new instance internal Message with internal header
    ///
    pub fn with_int_header(h: InternalMessageHeader) -> Message {
        Message {
            header: CommonMsgInfo::IntMsgInfo(h),
            init: None,
            body: None,
            body_to_ref: None,
            init_to_ref: None,
        }
    }

    ///
    /// Create new instance internal Message with internal header and body
    ///
    pub fn with_int_header_and_body(h: InternalMessageHeader, body: SliceData) -> Message {
        Message {
            header: CommonMsgInfo::IntMsgInfo(h),
            init: None,
            body: Some(body),
            body_to_ref: None,
            init_to_ref: None,
        }
    }

    ///
    /// Create new instance of external Message with inbound header
    ///
    pub fn with_ext_in_header(h: ExternalInboundMessageHeader) -> Message {
        Message {
            header: CommonMsgInfo::ExtInMsgInfo(h),
            init: None,
            body: None,
            body_to_ref: None,
            init_to_ref: None,
        }
    }

    ///
    /// Create new instance of external Message with inbound header and body
    ///
    pub fn with_ext_in_header_and_body(
        h: ExternalInboundMessageHeader,
        body: SliceData,
    ) -> Message {
        Message {
            header: CommonMsgInfo::ExtInMsgInfo(h),
            init: None,
            body: Some(body),
            body_to_ref: None,
            init_to_ref: None,
        }
    }

    ///
    /// Create new instance of external Message with outbound header
    ///
    pub fn with_ext_out_header(h: ExtOutMessageHeader) -> Message {
        Message {
            header: CommonMsgInfo::ExtOutMsgInfo(h),
            init: None,
            body: None,
            body_to_ref: None,
            init_to_ref: None,
        }
    }

    pub fn header(&self) -> &CommonMsgInfo {
        &self.header
    }

    /// be careful, this method will reset serialize parameters
    pub fn set_header(&mut self, header: CommonMsgInfo) {
        self.body_to_ref = None;
        self.init_to_ref = None;
        self.header = header;
    }

    /// be careful, this method will reset serialize parameters
    pub fn header_mut(&mut self) -> &mut CommonMsgInfo {
        self.body_to_ref = None;
        self.init_to_ref = None;
        &mut self.header
    }

    pub fn withdraw_header(self) -> CommonMsgInfo {
        self.header
    }

    pub fn state_init(&self) -> Option<&StateInit> {
        self.init.as_ref()
    }

    /// be careful, this method will reset serialize parameters
    pub fn set_state_init(&mut self, init: StateInit) {
        self.body_to_ref = None;
        self.init_to_ref = None;
        self.init = Some(init);
    }

    pub fn has_body(&self) -> bool {
        match &self.body {
            Some(body) => !body.is_empty_cell(),
            None => false,
        }
    }

    pub fn body(&self) -> Option<&SliceData> {
        self.body.as_ref()
    }

    /// be careful, this method will reset serialize parameters
    pub fn set_body(&mut self, body: SliceData) {
        self.body_to_ref = None;
        self.init_to_ref = None;
        self.body = Some(body);
    }

    pub fn set_value(&mut self, value: CurrencyCollection) {
        if let CommonMsgInfo::IntMsgInfo(header) = &mut self.header {
            header.value = value;
        }
    }

    ///
    /// Get source account ID for internal message
    /// For other types of messages, function returned None
    ///
    pub fn get_int_src_account_id(&self) -> Option<AccountId> {
        let addr = match self.header {
            CommonMsgInfo::IntMsgInfo(ref header) => &header.src,
            CommonMsgInfo::ExtOutMsgInfo(ref header) => &header.src,
            _ => &MsgAddressIntOrNone::None,
        };
        if let MsgAddressIntOrNone::Some(MsgAddressInt::AddrStd(addr_std)) = addr {
            return Some(addr_std.address.clone());
            // TODO: What about AddrVar?
        }
        None
    }

    ///
    /// Get destination account ID for internal or inbound external message.
    /// For outbound external messages, function returns None
    ///
    pub fn int_dst_account_id(&self) -> Option<&AccountId> {
        match self.dst_ref() {
            Some(MsgAddressInt::AddrStd(std)) => Some(&std.address),
            _ => None,
        }
    }

    pub fn extract_dst_std_address(&self, do_rewrite: bool) -> Result<(i32, AccountId)> {
        self.dst_ref()
            .ok_or_else(|| error!("message is not internal"))?
            .extract_std_address(do_rewrite)
    }

    ///
    /// Get source internal address.
    ///
    pub fn src(&self) -> Option<MsgAddressInt> {
        self.src_ref().cloned()
    }

    ///
    /// Get destination internal address.
    ///
    pub fn dst(&self) -> Option<MsgAddressInt> {
        self.dst_ref().cloned()
    }

    ///
    /// Get reference to source internal address.
    ///
    pub fn src_ref(&self) -> Option<&MsgAddressInt> {
        let addr1 = match self.header() {
            CommonMsgInfo::IntMsgInfo(ref imi) => &imi.src,
            CommonMsgInfo::ExtOutMsgInfo(ref eimi) => &eimi.src,
            CommonMsgInfo::ExtInMsgInfo(_) => &MsgAddressIntOrNone::None,
        };
        match addr1 {
            MsgAddressIntOrNone::None => None,
            MsgAddressIntOrNone::Some(ref addr) => Some(addr),
        }
    }

    ///
    /// Get reference destination internal address.
    ///
    pub fn dst_ref(&self) -> Option<&MsgAddressInt> {
        match self.header {
            CommonMsgInfo::IntMsgInfo(ref header) => Some(&header.dst),
            CommonMsgInfo::ExtInMsgInfo(ref header) => Some(&header.dst),
            _ => None,
        }
    }

    pub fn src_to_string(&self, mode: u8) -> Result<String> {
        let addr = match &self.header {
            CommonMsgInfo::IntMsgInfo(header) => &header.src,
            CommonMsgInfo::ExtInMsgInfo(header) => return Ok(header.src.to_string()),
            CommonMsgInfo::ExtOutMsgInfo(header) => &header.src,
        };
        match addr {
            MsgAddressIntOrNone::None => fail!("Source address is none"),
            MsgAddressIntOrNone::Some(addr) => addr.to_string_custom(mode),
        }
    }

    pub fn dst_to_string(&self, mode: u8) -> Result<String> {
        match &self.header {
            CommonMsgInfo::IntMsgInfo(header) => header.dst.to_string_custom(mode),
            CommonMsgInfo::ExtInMsgInfo(header) => header.dst.to_string_custom(mode),
            CommonMsgInfo::ExtOutMsgInfo(header) => Ok(header.dst.to_string()),
        }
    }

    ///
    /// Get value transmitted by the message
    /// Set Logical Time and UNIX time for
    /// Internal and External outbound messages
    ///
    pub fn set_at_and_lt(&mut self, utime: u32, lt: u64) {
        match self.header {
            CommonMsgInfo::IntMsgInfo(ref mut header) => {
                header.created_at = utime;
                header.created_lt = lt;
            }
            CommonMsgInfo::ExtOutMsgInfo(ref mut header) => {
                header.created_at = utime;
                header.created_lt = lt;
            }
            _ => (),
        };
    }
    pub fn set_src(&mut self, address: MsgAddressIntOrNone) {
        match self.header {
            CommonMsgInfo::IntMsgInfo(ref mut header) => {
                header.src = address;
            }
            CommonMsgInfo::ExtOutMsgInfo(ref mut header) => {
                header.src = address;
            }
            _ => (),
        };
    }
    pub fn set_src_address(&mut self, src: MsgAddressInt) {
        match &mut self.header {
            CommonMsgInfo::IntMsgInfo(header) => {
                header.src = MsgAddressIntOrNone::Some(src);
            }
            CommonMsgInfo::ExtOutMsgInfo(header) => {
                header.src = MsgAddressIntOrNone::Some(src);
            }
            _ => (),
        };
    }

    ///
    /// Get message's Unix time and logical time
    /// None only for internal and external outbound message
    ///
    pub fn at_and_lt(&self) -> Option<(u32, u64)> {
        match &self.header {
            CommonMsgInfo::IntMsgInfo(header) => Some((header.created_at, header.created_lt)),
            CommonMsgInfo::ExtOutMsgInfo(header) => Some((header.created_at, header.created_lt)),
            _ => None,
        }
    }

    pub fn created_lt(&self) -> Option<u64> {
        match self.header {
            CommonMsgInfo::IntMsgInfo(ref header) => Some(header.created_lt),
            CommonMsgInfo::ExtOutMsgInfo(ref header) => Some(header.created_lt),
            _ => None,
        }
    }

    ///
    /// Get value transmitted by the message
    ///
    pub fn get_value(&self) -> Option<&CurrencyCollection> {
        self.value()
    }

    ///
    /// Get value transmitted by the message
    ///
    pub fn value(&self) -> Option<&CurrencyCollection> {
        self.header.get_value()
    }

    ///
    /// Get value transmitted by the message
    ///
    pub fn value_mut(&mut self) -> Option<&mut CurrencyCollection> {
        self.body_to_ref = None;
        self.init_to_ref = None;
        self.header.get_value_mut()
    }

    ///
    /// Get message fees
    /// Only Internal and External outbound messages has a fee
    /// If the transmittal of a message it is necessary to collect a fee. Otherwise None
    ///
    pub fn get_fee(&self) -> Result<Option<Coins>> {
        self.header.fee()
    }

    ///
    /// Is message an internal?
    ///
    pub fn is_internal(&self) -> bool {
        matches!(self.header, CommonMsgInfo::IntMsgInfo(_))
    }

    pub fn is_bouncable(&self) -> bool {
        match self.header() {
            CommonMsgInfo::IntMsgInfo(ref header) => header.bounce,
            _ => false,
        }
    }

    pub fn is_bounced(&self) -> bool {
        match self.header() {
            CommonMsgInfo::IntMsgInfo(ref header) => header.bounced,
            _ => false,
        }
    }

    ///
    /// Is message an external inbound?
    ///
    pub fn is_inbound_external(&self) -> bool {
        matches!(self.header, CommonMsgInfo::ExtInMsgInfo(_))
    }

    ///
    /// Is message an external outbound?
    ///
    pub fn is_outbound_external(&self) -> bool {
        matches!(self.header, CommonMsgInfo::ExtOutMsgInfo(_))
    }

    ///
    /// is message have state init.
    ///
    pub fn have_state_init(&self) -> bool {
        self.init.is_some()
    }

    ///
    /// Get destination workchain of message
    ///
    pub fn dst_workchain_id(&self) -> Option<i32> {
        match &self.header {
            CommonMsgInfo::IntMsgInfo(ref imi) => Some(imi.dst.workchain_id()),
            CommonMsgInfo::ExtInMsgInfo(ref eimi) => Some(eimi.dst.workchain_id()),
            CommonMsgInfo::ExtOutMsgInfo(_) => None,
        }
    }

    ///
    /// Get destination workchain of message
    ///
    pub fn workchain_id(&self) -> Option<i32> {
        self.dst_workchain_id()
    }

    ///
    /// Get source workchain of message
    ///
    pub fn src_workchain_id(&self) -> Option<i32> {
        let addr1 = match self.header() {
            CommonMsgInfo::IntMsgInfo(ref imi) => &imi.src,
            CommonMsgInfo::ExtOutMsgInfo(ref eimi) => &eimi.src,
            CommonMsgInfo::ExtInMsgInfo(_) => &MsgAddressIntOrNone::None,
        };
        match addr1 {
            MsgAddressIntOrNone::None => None,
            MsgAddressIntOrNone::Some(ref addr) => Some(addr.workchain_id()),
        }
    }

    pub fn is_dst_masterchain(&self) -> bool {
        self.dst_workchain_id() == Some(MASTERCHAIN_ID)
    }

    pub fn is_masterchain(&self) -> bool {
        self.src_workchain_id() == Some(MASTERCHAIN_ID)
            || self.dst_workchain_id() == Some(MASTERCHAIN_ID)
    }

    pub fn prepare_proof(&self, is_inbound: bool, block_root: &Cell) -> Result<Cell> {
        // proof for message and block info in block

        let msg_hash = self.hash()?;
        let usage_tree = UsageTree::with_root(block_root.clone());
        let block = Block::construct_from_cell(usage_tree.root_cell()).unwrap();

        block.read_info()?;

        if is_inbound {
            block
                .read_extra()?
                .read_in_msg_descr()?
                .get(&msg_hash)?
                .ok_or_else(|| {
                    BlockError::InvalidArg(
                        "Message isn't belonged given block's in_msg_descr".to_string(),
                    )
                })?
                .read_message()?;
        } else {
            block
                .read_extra()?
                .read_out_msg_descr()?
                .get(&msg_hash)?
                .ok_or_else(|| {
                    BlockError::InvalidArg(
                        "Message isn't belonged given block's out_msg_descr".to_string(),
                    )
                })?
                .read_message()?;
        }

        MerkleProof::create_by_usage_tree(block_root, &usage_tree)?.serialize()
    }

    pub fn serialization_params(&self) -> (Option<bool>, Option<bool>) {
        (self.body_to_ref, self.init_to_ref)
    }
    #[cfg(test)]
    pub fn set_serialization_params(
        &mut self,
        body_to_ref: Option<bool>,
        init_to_ref: Option<bool>,
    ) {
        self.body_to_ref = body_to_ref;
        self.init_to_ref = init_to_ref;
    }

    pub fn copy_without_extra_currencies(&self) -> Option<Self> {
        if let Some(hdr) = self.int_header() {
            if !hdr.value.other.is_empty() {
                let mut msg = self.clone();
                if let Some(hdr) = msg.int_header_mut() {
                    hdr.value.other.clear();
                    return Some(msg);
                }
            }
        }
        None
    }

    pub fn serialize_as_is(&self) -> Result<(BuilderData, bool, bool)> {
        self.serialize_with_params(self.body_to_ref, self.init_to_ref)
    }

    /// Recalculate serialization parameters for message
    /// If body_to_ref and init_to_ref are not set, then
    /// it will be calculated based on the message size
    /// and references count.
    /// If they are set, then they will be used as is.
    ///
    /// Returns tuple with body_to_ref and init_to_ref
    ///
    /// It uses in SENDMSG opcode to calculate fee
    pub fn recalc_serialization_params(&self) -> Result<(bool, bool)> {
        // write header
        let builder = self.header.write_to_new_cell()?;
        let mut header_bits = builder.length_in_bits() + 2; // 2 is state_init's Maybe bit + body's Either bit
        let header_refs = builder.references_used();

        let (state_bits, state_refs) = if let Some(init) = self.state_init() {
            header_bits += 1; // state_init's Either bit
            let b = init.write_to_new_cell()?;
            (b.bits_used(), b.references_used())
        } else {
            (0, 0)
        };
        let (body_bits, body_refs) = self.body.as_ref().map_or((0, 0), |s| s.remainig());
        let (body_to_ref, init_to_ref) = if header_bits + state_bits + body_bits <= MAX_DATA_BITS
            && header_refs + state_refs + body_refs <= MAX_REFERENCES_COUNT
            && self.body_to_ref != Some(true)
            && self.init_to_ref != Some(true)
        {
            // all fits into one cell
            (false, false)
        } else if header_bits + body_bits <= MAX_DATA_BITS
            && header_refs + body_refs < MAX_REFERENCES_COUNT
            && self.body_to_ref != Some(true)
        {
            // + init cell ref
            // header & body fit
            (false, true)
        } else if header_bits + state_bits <= MAX_DATA_BITS
            && header_refs + state_refs < MAX_REFERENCES_COUNT
            && self.init_to_ref != Some(true)
        {
            // + body cell ref
            // header & state fit
            (true, !self.body_to_ref.unwrap_or(false))
        } else {
            // only header fits
            (true, true)
        };
        Ok((body_to_ref, init_to_ref))
    }

    pub fn serialize_with_params(
        &self,
        body_to_ref: Option<bool>,
        init_to_ref: Option<bool>,
    ) -> Result<(BuilderData, bool, bool)> {
        // write header
        let mut builder = self.header.write_to_new_cell()?;
        let mut header_bits = builder.length_in_bits() + 2; // 2 is state_init's Maybe bit + body's Either bit
        let header_refs = builder.references_used();

        let (state_bits, state_refs, init_builder) = if let Some(init) = self.state_init() {
            header_bits += 1; // state_init's Either bit
            let b = init.write_to_new_cell()?;
            (b.bits_used(), b.references_used(), Some(b))
        } else {
            (0, 0, None)
        };
        let (body_bits, body_refs) = self.body.as_ref().map_or((0, 0), |s| s.remainig());
        let (body_to_ref, init_to_ref) =
            if let (Some(body_to_ref), Some(init_to_ref)) = (body_to_ref, init_to_ref) {
                (body_to_ref, init_to_ref)
            } else if header_bits + state_bits + body_bits <= MAX_DATA_BITS
                && header_refs + state_refs + body_refs <= MAX_REFERENCES_COUNT
            {
                // all fits into one cell
                (false, false)
            } else if header_bits + body_bits <= MAX_DATA_BITS
                && header_refs + body_refs < MAX_REFERENCES_COUNT
            {
                // + init cell ref
                // header & body fit
                (false, true)
            } else if header_bits + state_bits <= MAX_DATA_BITS
                && header_refs + state_refs < MAX_REFERENCES_COUNT
            {
                // + body cell ref
                // header & state fit
                (true, false)
            } else {
                // only header fits
                (true, true)
            };

        // write StateInit
        match init_builder {
            Some(init_builder) => {
                if !init_to_ref {
                    builder
                        .append_bit_one()? //mayby bit
                        .append_bit_zero()?; //either bit
                    builder.append_builder(&init_builder)?;
                } else {
                    // if not enough space in current cell - append as reference
                    builder
                        .append_bit_one()? //mayby bit
                        .append_bit_one()?; //either bit
                    builder.checked_append_reference(init_builder.into_cell()?)?;
                }
            }
            None => {
                // write may be bit
                builder.append_bit_zero()?;
            }
        }

        // write body
        match self.body.as_ref() {
            Some(body) => {
                if !body_to_ref {
                    builder.append_bit_zero()?; //either bit  x:X
                    builder.checked_append_references_and_data(body)?;
                } else {
                    // if not enough space in current cell - append as reference
                    builder.append_bit_one()?; //either bit  x:^X
                    builder.checked_append_reference(body.clone().into_cell()?)?;
                };
            }
            None => {
                // write either be bit
                // otherwise not be able to read
                builder.append_bit_zero()?;
            }
        }
        Ok((builder, body_to_ref, init_to_ref))
    }

    pub fn normalize_external_inbound(&mut self) -> Result<()> {
        let CommonMsgInfo::ExtInMsgInfo(header) = &mut self.header else {
            fail!("Only external inbound messages are supported")
        };
        header.src = MsgAddressExt::default();
        header.import_fee = Coins::default();
        self.init = None;
        self.body_to_ref = Some(true);
        self.init_to_ref = Some(true);
        Ok(())
    }

    pub fn normalized_hash(&self) -> Result<UInt256> {
        let normalized = self.clone().normalize_external_inbound()?;
        GetRepresentationHash::hash(&normalized)
    }

    // The method reads only root cell from given boc and message header from the cell.
    // It doesn't check boc & message correctness and integrity!
    pub fn read_header_fast(data: &[u8]) -> Result<CommonMsgInfo> {
        let mut slice = read_boc_root(data)?;
        let header = CommonMsgInfo::construct_from(&mut slice)?;
        Ok(header)
    }

    pub fn construct_with_gas_consumer(
        cell: Cell,
        gas_consumer: &mut impl GasConsumer,
    ) -> Result<Self> {
        let mut msg = Self::default();
        let mut cell = gas_consumer.load_cell(cell)?;
        msg.read_with_gas_consumer(&mut cell, gas_consumer)?;
        Ok(msg)
    }

    fn read_with_gas_consumer(
        &mut self,
        cell: &mut SliceData,
        gas_consumer: &mut impl GasConsumer,
    ) -> Result<()> {
        // read header
        self.header.read_from(cell)?;

        // read StateInit
        if cell.get_next_bit()? {
            // maybe of init
            let mut init = StateInit::default();
            if cell.get_next_bit()? {
                // either of init
                // read from reference
                let mut r = gas_consumer.load_cell(cell.checked_drain_reference()?)?;
                init.read_from(&mut r)?;
                self.init = Some(init);
                self.init_to_ref = Some(true);
            } else {
                // read from current cell
                init.read_from(cell)?;
                self.init = Some(init);
                self.init_to_ref = Some(false);
            }
        } else {
            self.init_to_ref = Some(false);
        }

        // read body
        // A message is always serialized inside the blockchain as the last field in
        // a cell. Therefore, the blockchain software may assume that whatever bits
        // and references left unparsed after parsing the fields of a Message preceding
        // body belong to the payload body : X, without knowing anything about the
        // serialization of the type X.

        self.body = if cell.get_next_bit()? {
            // body in reference
            self.body_to_ref = Some(true);
            Some(gas_consumer.load_cell(cell.checked_drain_reference()?)?)
        } else {
            self.body_to_ref = Some(false);
            if cell.is_empty_cell() {
                // no body
                None
            } else {
                // body is leftover
                Some(cell.clone())
            }
        };
        Ok(())
    }
}

impl Serializable for Message {
    fn write_to(&self, builder: &mut BuilderData) -> Result<()> {
        // first try to serialize as is
        if self.body_to_ref.is_some() || self.init_to_ref.is_some() {
            if let Ok((b, _, _)) = self.serialize_as_is() {
                if builder.is_empty() {
                    *builder = b;
                } else {
                    builder.append_builder(&b)?;
                }
                return Ok(());
            }
        }
        // now try to repack to possible serilalize
        let (b, _, _) = self.serialize_with_params(None, None)?;
        if builder.is_empty() {
            *builder = b;
        } else {
            builder.append_builder(&b)?;
        }
        Ok(())
    }
}

impl Deserializable for Message {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.read_with_gas_consumer(cell, &mut 0)
    }
    fn skip(slice: &mut SliceData) -> Result<()> {
        // skip header
        CommonMsgInfo::skip(slice)?;

        // skip StateInit
        if slice.get_next_bit()? {
            // maybe of init
            if slice.get_next_bit()? {
                // either of init
                // skip reference
                let cell = slice.checked_drain_reference()?;
                StateInit::skip(&mut SliceData::load_cell(cell)?)?;
            } else {
                // skip from current cell
                StateInit::skip(slice)?;
            }
        }

        // skip body
        if slice.get_next_bit()? {
            // body in reference
            slice.checked_drain_reference()?;
        }
        Ok(())
    }
}

////////////////////////////////////////////////////////////////
///
/// 3.1.7. Message layout.
/// tick_tock$_ tick:Boolean tock:Boolean = TickTock;
///
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TickTock {
    pub tick: bool,
    pub tock: bool,
}

impl TickTock {
    pub fn with_values(tick: bool, tock: bool) -> Self {
        TickTock { tick, tock }
    }

    pub fn set_tick(&mut self, tick: bool) {
        self.tick = tick;
    }

    pub fn set_tock(&mut self, tock: bool) {
        self.tock = tock;
    }
    pub fn as_usize(&self) -> usize {
        let mut result = 0;
        if self.tick {
            result += 2;
        }
        if self.tock {
            result += 1;
        }
        result
    }
}

impl Serializable for TickTock {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_bit_bool(self.tick)?;
        cell.append_bit_bool(self.tock)?;
        Ok(())
    }
}

impl Deserializable for TickTock {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.tick = cell.get_next_bit()?;
        self.tock = cell.get_next_bit()?;
        Ok(())
    }
}

impl fmt::Display for TickTock {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "TickTock[Tick {}, Tock {}]", self.tick, self.tock)
    }
}

/// simple_lib$_ public:Bool root:^Cell = SimpleLib;
#[derive(Default, Debug, Clone, Eq, PartialEq)]
pub struct SimpleLib {
    pub public: bool,
    pub root: Cell,
}

impl SimpleLib {
    pub fn new(root: Cell, public: bool) -> Self {
        Self { public, root }
    }
    pub fn is_public_library(&self) -> bool {
        self.public
    }
}

impl Serializable for SimpleLib {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.public.write_to(cell)?;
        self.root.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for SimpleLib {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        self.public.read_from(slice)?;
        self.root.read_from(slice)?;
        Ok(())
    }
}

// HashmapE 256 SimpleLib
define_HashmapE! {StateInitLib, 256, SimpleLib}

///////////////////////////////////////////////////////////////////////////////
///
/// 3.1.7. Message layout.
/// fixed_prefix_length:(Maybe (## 5)) special:(Maybe TickTock)
/// code:(Maybe ^Cell) data:(Maybe ^Cell)
/// library:(HashmapE 256 SimpleLib) = StateInit;
///
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StateInit {
    pub fixed_prefix_length: Option<Number5>,
    pub special: Option<TickTock>,
    pub code: Option<Cell>,
    pub data: Option<Cell>,
    pub library: StateInitLib,
}

impl StateInit {
    pub fn with_code_and_data(code: Cell, data: Cell) -> Self {
        StateInit {
            fixed_prefix_length: None,
            special: None,
            code: Some(code),
            data: Some(data),
            library: StateInitLib::default(),
        }
    }

    pub fn set_fixed_prefix_length(&mut self, val: Number5) {
        self.fixed_prefix_length = Some(val);
    }

    pub fn fixed_prefix_length(&self) -> Option<&Number5> {
        self.fixed_prefix_length.as_ref()
    }

    pub fn set_special(&mut self, val: TickTock) {
        self.special = Some(val);
    }

    pub fn special(&self) -> Option<&TickTock> {
        self.special.as_ref()
    }

    pub fn set_code(&mut self, val: Cell) {
        self.code = Some(val);
    }

    pub fn code(&self) -> Option<&Cell> {
        self.code.as_ref()
    }

    pub fn set_data(&mut self, val: Cell) {
        self.data = Some(val);
    }

    pub fn data(&self) -> Option<&Cell> {
        self.data.as_ref()
    }

    pub fn libraries(&self) -> &StateInitLib {
        &self.library
    }

    pub fn set_library(&mut self, val: Cell) {
        self.library = StateInitLib::with_hashmap(Some(val));
    }

    pub fn set_library_code(&mut self, code: Cell, public: bool) -> Result<()> {
        self.library.set(&code.repr_hash(), &SimpleLib::new(code, public))?;
        Ok(())
    }
}

impl Serializable for StateInit {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.fixed_prefix_length.write_to(cell)?;
        self.special.write_to(cell)?;
        self.code.write_to(cell)?;
        self.data.write_to(cell)?;
        self.library.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for StateInit {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.fixed_prefix_length.read_from(cell)?;
        self.special.read_from(cell)?;
        // code:(Maybe ^Cell)
        self.code = match cell.get_next_bit()? {
            true => Some(cell.checked_drain_reference()?),
            false => None,
        };

        // data:(Maybe ^Cell)
        self.data = match cell.get_next_bit()? {
            true => Some(cell.checked_drain_reference()?),
            false => None,
        };

        self.library.read_from(cell)?;

        Ok(())
    }
}

#[derive(Debug, Default, Eq, PartialEq, Clone, Copy)]
pub enum MessageProcessingStatus {
    #[default]
    Unknown = 0,
    Queued,
    Processing,
    Preliminary,
    Proposed,
    Finalized,
    Refused,
    Transiting,
}

///////////////////////////////////////////////////////////////////////////////
///
/// Auto-generated code
///
///

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct AnycastInfo {
    pub depth: Number5,
    pub rewrite_pfx: SliceData,
}

impl Deserializable for AnycastInfo {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.depth.read_from(cell)?;
        self.rewrite_pfx = cell.get_next_slice(self.depth.as_usize())?;
        Ok(())
    }
    fn skip(slice: &mut SliceData) -> Result<()> {
        let depth = Number5::construct_from(slice)?;
        slice.move_by(depth.as_usize())?;
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct MsgAddrExt {
    pub len: Number9,
    pub external_address: SliceData,
}

impl Deserializable for MsgAddrExt {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.len.read_from(cell)?;
        self.external_address = cell.get_next_slice(self.len.as_usize())?;
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum MsgAddressExt {
    #[default]
    AddrNone,
    AddrExtern(MsgAddrExt),
}

impl Deserializable for MsgAddressExt {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        let bits = cell.get_next_bits(2)?[0] >> 6;
        if bits == 0 {
            *self = MsgAddressExt::AddrNone;
        }
        if bits == 1 {
            let mut data = MsgAddrExt::default();
            data.read_from(cell)?;
            *self = MsgAddressExt::AddrExtern(data);
        }
        // TODO: add error checking!
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MsgAddrStd {
    pub anycast: Option<AnycastInfo>,
    pub workchain_id: i8,
    pub address: AccountId,
}

impl Deserializable for MsgAddrStd {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.anycast.read_from(cell)?;
        self.workchain_id.read_from(cell)?;
        self.address = cell.get_next_slice(256)?;
        Ok(())
    }
    fn skip(slice: &mut SliceData) -> Result<()> {
        Option::<AnycastInfo>::skip(slice)?;
        slice.move_by(8)?; // workchain_id
        slice.move_by(256)?; // address
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct MsgAddrVar {
    pub anycast: Option<AnycastInfo>,
    pub addr_len: Number9,
    pub workchain_id: i32,
    pub address: SliceData,
}

impl Deserializable for MsgAddrVar {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.anycast.read_from(cell)?;
        self.addr_len.read_from(cell)?;
        self.workchain_id.read_from(cell)?;
        self.address = cell.get_next_slice(self.addr_len.as_usize())?;
        Ok(())
    }
    fn skip(slice: &mut SliceData) -> Result<()> {
        Option::<AnycastInfo>::skip(slice)?;
        let addr_len = Number9::construct_from(slice)?;
        slice.move_by(32)?; // workchain_id
        slice.move_by(addr_len.as_usize())?; // address
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MsgAddressInt {
    AddrStd(MsgAddrStd),
    AddrVar(MsgAddrVar),
}

impl Deserializable for MsgAddressInt {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        *self = match cell.get_next_int(2)? {
            0b10 => MsgAddressInt::AddrStd(MsgAddrStd::construct_from(cell)?),
            0b11 => MsgAddressInt::AddrVar(MsgAddrVar::construct_from(cell)?),
            _ => fail!(BlockError::Other("Wrong type of address".to_string())),
        };
        // TODO: fix autogen for error checking!
        /*
        let bits = cell.get_next_bits(2)?[0] >> 6;
        if bits == 2 {
            let mut data = MsgAddrStd::default();
            data.read_from(cell)?;
            *self = MsgAddressInt::AddrStd(data);
        }
        if bits == 3 {
            let mut data = MsgAddrVar::default();
            data.read_from(cell)?;
            *self = MsgAddressInt::AddrVar(data);
        }
        */
        Ok(())
    }
    fn skip(slice: &mut SliceData) -> Result<()> {
        match slice.get_next_int(2)? {
            0b10 => MsgAddrStd::skip(slice)?,
            0b11 => MsgAddrVar::skip(slice)?,
            _ => fail!(BlockError::Other("Wrong type of address".to_string())),
        };
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum MsgAddress {
    #[default]
    AddrNone,
    AddrExt(MsgAddrExt),
    AddrStd(MsgAddrStd),
    AddrVar(MsgAddrVar),
}

impl MsgAddress {
    pub fn to_msg_addr_int(self) -> Option<MsgAddressInt> {
        match self {
            MsgAddress::AddrStd(addr) => Some(MsgAddressInt::AddrStd(addr)),
            MsgAddress::AddrVar(addr) => Some(MsgAddressInt::AddrVar(addr)),
            MsgAddress::AddrNone | MsgAddress::AddrExt(_) => None,
        }
    }
}

impl Deserializable for MsgAddress {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        let bits = cell.get_next_bits(2)?[0] >> 6;
        if bits == 0 {
            *self = MsgAddress::AddrNone;
        }
        if bits == 1 {
            let mut data = MsgAddrExt::default();
            data.read_from(cell)?;
            *self = MsgAddress::AddrExt(data);
        }
        if bits == 2 {
            let mut data = MsgAddrStd::default();
            data.read_from(cell)?;
            *self = MsgAddress::AddrStd(data);
        }
        if bits == 3 {
            let mut data = MsgAddrVar::default();
            data.read_from(cell)?;
            *self = MsgAddress::AddrVar(data);
        }
        Ok(())
    }
}

// _ value:CurrencyCollection created_lt:uint64 created_at:uint32 = NewBounceOriginalInfo;
/// Information about original bounced message
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NewBounceOriginalInfo {
    pub value: CurrencyCollection,
    pub created_lt: u64,
    pub created_at: u32,
}

impl Serializable for NewBounceOriginalInfo {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.value.write_to(cell)?;
        self.created_lt.write_to(cell)?;
        self.created_at.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for NewBounceOriginalInfo {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.value.read_from(cell)?;
        self.created_lt.read_from(cell)?;
        self.created_at.read_from(cell)?;
        Ok(())
    }
}

// _ gas_used:uint32 vm_steps:uint32 = NewBounceComputePhaseInfo;
/// Information about compute phase of the transaction that caused the bounce
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NewBounceComputePhaseInfo {
    pub gas_used: u32,
    pub vm_steps: u32,
}

impl Serializable for NewBounceComputePhaseInfo {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.gas_used.write_to(cell)?;
        self.vm_steps.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for NewBounceComputePhaseInfo {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.gas_used.read_from(cell)?;
        self.vm_steps.read_from(cell)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Copy, num_derive::FromPrimitive)]
pub enum BouncedByPhase {
    #[default]
    ComputeSkip = 0,
    Compute = 1,
    Action = 2,
}

// new_bounce_body#fffffffe
//     original_body:^Cell
//     original_info:^NewBounceOriginalInfo
//     bounced_by_phase:uint8 exit_code:int32
//     compute_phase:(Maybe NewBounceComputePhaseInfo)
//     = NewBounceBody;
/// Bounced message body layout
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NewBounceBody {
    pub original_body: Cell,
    pub original_info: ChildCell<NewBounceOriginalInfo>,
    pub bounced_by_phase: BouncedByPhase,
    pub exit_code: i32,
    pub compute_phase: Option<NewBounceComputePhaseInfo>,
}

pub const NEW_BOUNCE_BODY_TAG: u32 = 0xfffffffe;

impl Serializable for NewBounceBody {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        NEW_BOUNCE_BODY_TAG.write_to(cell)?;
        cell.checked_append_reference(self.original_body.clone())?;
        cell.checked_append_reference(self.original_info.cell())?;
        (self.bounced_by_phase as u8).write_to(cell)?;
        self.exit_code.write_to(cell)?;
        self.compute_phase.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for NewBounceBody {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        let tag = cell.get_next_u32()?;
        if tag != NEW_BOUNCE_BODY_TAG {
            fail!(Self::invalid_tag(tag))
        }
        self.original_body = cell.checked_drain_reference()?;
        self.original_info = ChildCell::with_cell(cell.checked_drain_reference()?);
        let bounced_by_phase = cell.get_next_byte()?;
        self.bounced_by_phase = BouncedByPhase::from_u8(bounced_by_phase)
            .ok_or_else(|| error!(Self::invalid_tag(bounced_by_phase as u32)))?;
        self.exit_code.read_from(cell)?;
        self.compute_phase.read_from(cell)?;
        Ok(())
    }
}
