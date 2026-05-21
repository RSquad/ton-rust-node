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
use crate::{
    base64_decode_to_slice, base64_encode, base64_encode_url_safe, define_HashmapE,
    error::{Error, Result},
    fail, sha256_digest, Augmentable, BlockError, BuilderData, Cell, CellType, Deserializable,
    HashmapE, HashmapType, IBitstring, Serializable, SliceData, SmallData,
};
use num::{bigint::Sign, BigInt, One, Zero};
#[cfg(feature = "mirrornet")]
use std::sync::atomic::{AtomicI64, Ordering};
use std::{
    any::type_name,
    cmp,
    convert::{TryFrom, TryInto},
    fmt::{self, Display, LowerHex, UpperHex},
    io::Read,
    marker::PhantomData,
    ops::{Deref, DerefMut},
    str::{self, FromStr},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Default, PartialEq, Eq, Hash, Ord, PartialOrd)]
#[repr(transparent)]
pub struct UInt256([u8; 32]);

impl UInt256 {
    pub const fn default() -> Self {
        Self::new()
    }
    pub const fn new() -> Self {
        Self::ZERO
    }
    pub const fn with_array(data: [u8; 32]) -> Self {
        Self(data)
    }

    pub fn is_zero(&self) -> bool {
        for b in &self.0 {
            if b != &0 {
                return false;
            }
        }
        true
    }

    pub const fn as_array(&self) -> &[u8; 32] {
        &self.0
    }

    pub const fn as_slice(&self) -> &[u8; 32] {
        &self.0
    }

    // Returns solid string like this: a80b23bfe4d301497f3ce11e753f23e8dec32368945ee279d044dbc1f91ace2a
    pub fn as_hex_string(&self) -> String {
        hex::encode(self.0)
    }

    // TODO: usage should be changed to as_hex_string
    pub fn to_hex_string(&self) -> String {
        self.as_hex_string()
    }

    pub fn as_base64(&self, url_safe: bool) -> String {
        if url_safe {
            base64_encode_url_safe(self.0)
        } else {
            base64_encode(self.0)
        }
    }

    pub fn calc_file_hash(bytes: &[u8]) -> Self {
        Self::calc_sha256(bytes)
    }

    pub fn calc_sha256(bytes: &[u8]) -> Self {
        Self(sha256_digest(bytes))
    }

    pub fn from_raw(data: Vec<u8>, length: usize) -> Self {
        assert_eq!(length, 256);
        let hash: [u8; 32] = data.try_into().unwrap();
        Self(hash)
    }

    pub fn from_slice(value: &[u8]) -> Self {
        match value.try_into() {
            Ok(hash) => Self(hash),
            Err(_) => Self::from_le_bytes(value),
        }
    }

    pub fn from_be_bytes(value: &[u8]) -> Self {
        let mut data = [0; 32];
        let len = cmp::min(value.len(), 32);
        let offset = 32 - len;
        (0..len).for_each(|i| data[i + offset] = value[i]);
        Self(data)
    }

    pub fn from_le_bytes(value: &[u8]) -> Self {
        let mut data = [0; 32];
        let len = cmp::min(value.len(), 32);
        (0..len).for_each(|i| data[i] = value[i]);
        Self(data)
    }

    pub const fn max() -> Self {
        UInt256::MAX
    }

    pub fn rand() -> Self {
        Self((0..32).map(|_| rand::random::<u8>()).collect::<Vec<u8>>().try_into().unwrap())
    }

    pub fn inner(self) -> [u8; 32] {
        self.0
    }

    pub fn into_vec(self) -> Vec<u8> {
        self.0.to_vec()
    }

    pub fn prefix64(&self) -> u64 {
        u64::from_be_bytes(self.0[0..8].try_into().unwrap())
    }

    pub const ZERO: UInt256 = UInt256([0; 32]);
    pub const MIN: UInt256 = UInt256([0; 32]);
    pub const MAX: UInt256 = UInt256([0xFF; 32]);
}

impl From<[u8; 32]> for UInt256 {
    fn from(data: [u8; 32]) -> Self {
        UInt256(data)
    }
}

impl From<&[u8; 32]> for UInt256 {
    fn from(data: &[u8; 32]) -> Self {
        UInt256(*data)
    }
}

impl From<&[u8]> for UInt256 {
    fn from(value: &[u8]) -> Self {
        Self::from_slice(value)
    }
}

impl From<Vec<u8>> for UInt256 {
    fn from(value: Vec<u8>) -> Self {
        match value.try_into() {
            Ok(hash) => Self(hash),
            Err(value) => UInt256::from_le_bytes(value.as_slice()),
        }
    }
}

impl FromStr for UInt256 {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self> {
        let mut result = Self::default();
        match value.len() {
            64 => hex::decode_to_slice(value, &mut result.0)?,
            66 => hex::decode_to_slice(&value[2..], &mut result.0)?,
            44 => base64_decode_to_slice(value, &mut result.0)?,
            _ => fail!("invalid account ID string (32 bytes expected), but got string {}", value),
        }
        Ok(result)
    }
}

impl fmt::Debug for UInt256 {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        LowerHex::fmt(self, f)
    }
}

impl fmt::Display for UInt256 {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "UInt256[{:X?}]", self.as_slice())
    }
}

impl LowerHex for UInt256 {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if f.alternate() {
            write!(f, "0x{}", hex::encode(self.0))
        } else {
            write!(f, "{}", hex::encode(self.0))
            // write!(f, "{}...{}", hex::encode(&self.0[..2]), hex::encode(&self.0[30..32]))
        }
    }
}

impl UpperHex for UInt256 {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if f.alternate() {
            write!(f, "0x")?;
        }
        write!(f, "{}", hex::encode_upper(self.0))
    }
}

impl AsRef<[u8; 32]> for UInt256 {
    fn as_ref(&self) -> &[u8; 32] {
        &self.0
    }
}

impl AsRef<[u8]> for UInt256 {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

pub type AccountId = SliceData;

impl From<&[u8; 32]> for AccountId {
    fn from(data: &[u8; 32]) -> AccountId {
        SliceData::from_raw(SmallData::from_slice(data), 256)
    }
}

impl From<[u8; 32]> for AccountId {
    fn from(data: [u8; 32]) -> AccountId {
        SliceData::from_raw(SmallData::from_slice(&data), 256)
    }
}

impl From<UInt256> for AccountId {
    fn from(data: UInt256) -> AccountId {
        SliceData::from_raw(SmallData::from_slice(data.as_array()), 256)
    }
}

impl From<&UInt256> for AccountId {
    fn from(data: &UInt256) -> AccountId {
        SliceData::from_raw(SmallData::from_slice(data.as_array()), 256)
    }
}

impl FromStr for AccountId {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        let uint256: UInt256 = s.parse()?;
        Ok(SliceData::from_raw(SmallData::from_slice(uint256.as_slice()), 256))
    }
}

pub trait ByteOrderRead {
    fn read_be_uint(&mut self, bytes: usize) -> std::io::Result<u64>;
    fn read_le_uint(&mut self, bytes: usize) -> std::io::Result<u64>;
    fn read_byte(&mut self) -> std::io::Result<u8>;
    fn read_be_u16(&mut self) -> std::io::Result<u16>;
    fn read_be_u32(&mut self) -> std::io::Result<u32>;
    fn read_be_u64(&mut self) -> std::io::Result<u64>;
    fn read_le_u16(&mut self) -> std::io::Result<u16>;
    fn read_le_u32(&mut self) -> std::io::Result<u32>;
    fn read_le_u64(&mut self) -> std::io::Result<u64>;
    fn read_u256(&mut self) -> std::io::Result<[u8; 32]>;
}

impl<T: Read> ByteOrderRead for T {
    fn read_be_uint(&mut self, bytes: usize) -> std::io::Result<u64> {
        read_uint(self, bytes, false)
    }

    fn read_le_uint(&mut self, bytes: usize) -> std::io::Result<u64> {
        read_uint(self, bytes, true)
    }

    fn read_byte(&mut self) -> std::io::Result<u8> {
        self.read_be_uint(1).map(|value| value as u8)
    }

    fn read_be_u16(&mut self) -> std::io::Result<u16> {
        self.read_be_uint(2).map(|value| value as u16)
    }

    fn read_be_u32(&mut self) -> std::io::Result<u32> {
        self.read_be_uint(4).map(|value| value as u32)
    }

    fn read_be_u64(&mut self) -> std::io::Result<u64> {
        self.read_be_uint(8)
    }

    fn read_le_u16(&mut self) -> std::io::Result<u16> {
        let mut buf = [0; 2];
        self.read_exact(&mut buf)?;
        Ok(u16::from_le_bytes(buf))
    }

    fn read_le_u32(&mut self) -> std::io::Result<u32> {
        let mut buf = [0; 4];
        self.read_exact(&mut buf)?;
        Ok(u32::from_le_bytes(buf))
    }

    fn read_le_u64(&mut self) -> std::io::Result<u64> {
        let mut buf = [0; 8];
        self.read_exact(&mut buf)?;
        Ok(u64::from_le_bytes(buf))
    }

    fn read_u256(&mut self) -> std::io::Result<[u8; 32]> {
        let mut buf = [0; 32];
        self.read_exact(&mut buf)?;
        Ok(buf)
    }
}

fn read_uint<T: Read>(src: &mut T, bytes: usize, le: bool) -> std::io::Result<u64> {
    match bytes {
        1 => {
            let mut buf = [0];
            src.read_exact(&mut buf)?;
            Ok(buf[0] as u64)
        }
        2 => {
            let mut buf = [0; 2];
            src.read_exact(&mut buf)?;
            if le {
                Ok(u16::from_le_bytes(buf) as u64)
            } else {
                Ok(u16::from_be_bytes(buf) as u64)
            }
        }
        3..=4 => {
            let mut buf = [0; 4];
            if le {
                src.read_exact(&mut buf[0..bytes])?;
                Ok(u32::from_le_bytes(buf) as u64)
            } else {
                src.read_exact(&mut buf[4 - bytes..])?;
                Ok(u32::from_be_bytes(buf) as u64)
            }
        }
        5..=8 => {
            let mut buf = [0; 8];
            if le {
                src.read_exact(&mut buf[0..bytes])?;
                Ok(u64::from_le_bytes(buf))
            } else {
                src.read_exact(&mut buf[8 - bytes..])?;
                Ok(u64::from_be_bytes(buf))
            }
        }
        n => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("too many bytes ({}) to read in u64", n),
        )),
    }
}

pub type Bitmask = u8;

#[inline]
pub fn bits_to_bytes(bits: usize) -> usize {
    bits.div_ceil(8)
}

///
/// var_uint$_ {n:#} len:(#< n) value:(uint (len * 8)) = VarUInteger n;
///
/// var_int$_ {n:#} len:(#< n) value:(int (len * 8)) = VarInteger n;
/// nanocoins$_ amount:(VarUInteger 16) = Coins;
///
/// If one wants to represent x nanocoins, one selects an integer l < 16 such
/// that x < 2^8*l, and serializes first l as an unsigned 4-bit integer, then x itself
/// as an unsigned 8`-bit integer. Notice that four zero bits represent a zero
/// amount of Coins.
macro_rules! define_VarIntegerN {
    ( $varname:ident, $N:expr, BigInt ) => {
        #[derive(Eq, Clone, Debug)]
        pub struct $varname(BigInt);

        #[allow(dead_code)]
        impl $varname {
            fn get_len(value: &BigInt) -> usize {
                bits_to_bytes(value.bits() as usize)
            }

            pub fn inner(self) -> BigInt {
                self.0
            }

            pub fn value(&self) -> &BigInt {
                &self.0
            }

            pub fn value_mut(&mut self) -> &mut BigInt {
                &mut self.0
            }

            pub fn zero() -> Self {
                $varname(Zero::zero())
            }

            pub fn one() -> Self {
                $varname(One::one())
            }

            pub fn sgn(&self) -> bool {
                self.0.sign() != Sign::NoSign
            }

            pub fn from_two_u128(hi: u128, lo: u128) -> Result<Self> {
                let val = (BigInt::from(hi) << 128) | BigInt::from(lo);
                Self::check_overflow(&val)?;
                Ok($varname(val))
            }

            pub fn is_zero(&self) -> bool {
                self.0.is_zero()
            }

            fn check_overflow(value: &BigInt) -> Result<()> {
                match Self::get_len(&value) > $N {
                    true => fail!("value {} is bigger than {} bytes", value, $N),
                    false => Ok(()),
                }
            }

            // determine the size of the len field, using the formula from 3.3.4 VM
            fn get_len_len() -> usize {
                let max_bits = ($N - 1) as f64;
                max_bits.log2() as usize + 1
            }

            // Interface to write value with type rule
            fn write_to_cell(value: &BigInt) -> Result<BuilderData> {
                let len = Self::get_len(value);
                if len >= $N {
                    fail!("serialization of {} error {} >= {}", stringify!($varname), len, $N)
                }

                let mut cell = BuilderData::default();
                cell.append_bits(len, Self::get_len_len())?;
                let value = value.to_bytes_be().1;
                cell.append_raw(&value, len * 8)?;
                Ok(cell)
            }

            fn read_from_cell(cell: &mut SliceData) -> Result<BigInt> {
                let len = cell.get_next_int(Self::get_len_len())? as usize;
                if len >= $N {
                    fail!("deserialization of {} error {} >= {}", stringify!($varname), len, $N)
                }
                Ok(BigInt::from_bytes_be(Sign::Plus, &cell.get_next_bytes(len)?))
            }
        }

        impl<T: Into<BigInt>> From<T> for $varname {
            fn from(value: T) -> Self {
                let val = BigInt::from(value.into());
                Self::check_overflow(&val).expect("Integer overflow");
                $varname(val)
            }
        }

        impl FromStr for $varname {
            type Err = crate::Error;

            fn from_str(string: &str) -> Result<Self> {
                let result = if let Some(stripped) = string.strip_prefix("0x") {
                    BigInt::parse_bytes(stripped.as_bytes(), 16)
                } else {
                    BigInt::parse_bytes(string.as_bytes(), 10)
                };
                match result {
                    Some(val) => {
                        Self::check_overflow(&val)?;
                        Ok(Self(val))
                    }
                    None => fail!("cannot parse {} for {}", stringify!($varname), string),
                }
            }
        }

        impl AddSub for $varname {
            fn add(&mut self, other: &Self) -> Result<bool> {
                if let Some(result) = self.0.checked_add(&other.0) {
                    if let Err(err) = Self::check_overflow(&result) {
                        log::warn!("{} + {} overflow: {:?}", self, other, err);
                        Ok(false)
                    } else {
                        self.0 = result;
                        Ok(true)
                    }
                } else {
                    Ok(false)
                }
            }
            fn sub(&mut self, other: &Self) -> Result<bool> {
                if let Some(result) = self.0.checked_sub(&other.0) {
                    self.0 = result;
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
        }

        impl Ord for $varname {
            fn cmp(&self, other: &$varname) -> std::cmp::Ordering {
                Ord::cmp(&self.0, &other.0)
            }
        }

        impl PartialOrd for $varname {
            fn partial_cmp(&self, other: &$varname) -> Option<std::cmp::Ordering> {
                Some(self.cmp(other))
            }
        }

        impl PartialEq for $varname {
            fn eq(&self, other: &$varname) -> bool {
                self.cmp(other) == std::cmp::Ordering::Equal
            }
        }

        impl Default for $varname {
            fn default() -> Self {
                $varname(BigInt::default())
            }
        }

        impl Serializable for $varname {
            fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
                let data = Self::write_to_cell(&self.0)?;
                cell.append_builder(&data)?;
                Ok(())
            }
        }

        impl Deserializable for $varname {
            fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
                self.0 = Self::read_from_cell(cell)?;
                Ok(())
            }
            fn skip(slice: &mut SliceData) -> Result<()> {
                let len = slice.get_next_int(Self::get_len_len())? as usize;
                if len >= $N {
                    fail!("deserialization of {} error {} >= {}", stringify!($varname), len, $N)
                }
                slice.move_by(len * 8)?;
                Ok(())
            }
        }

        impl fmt::Display for $varname {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "{}", &self.0)
            }
        }
    };
    ( $varname:ident, $N:expr, $tt:ty ) => {
        #[derive(Eq, Copy, Clone, Debug, Default, Ord, PartialEq, PartialOrd)]
        pub struct $varname($tt);

        impl $varname {
            pub fn clear(&mut self) {
                self.0 = 0;
            }
            pub const fn zero() -> Self {
                Self(0)
            }
            pub const fn one() -> Self {
                Self(1)
            }
            pub const fn sgn(&self) -> bool {
                false
            }
            pub const fn is_zero(&self) -> bool {
                self.0 == 0
            }
            pub const MAX: Self = Self(((1 as $tt) << (8 * $N)) - 1);
            pub fn add_checked(&mut self, other: &Self) -> bool {
                if let Some(result) = self.0.checked_add(other.0) {
                    if let Err(err) = Self::check_overflow(&result) {
                        log::warn!("{} + {} overflow: {:?}", self, other, err);
                        false
                    } else {
                        self.0 = result;
                        true
                    }
                } else {
                    false
                }
            }
            pub fn sub_checked(&mut self, other: &Self) -> bool {
                if let Some(result) = self.0.checked_sub(other.0) {
                    self.0 = result;
                    true
                } else {
                    false
                }
            }
            fn check_overflow(value: &$tt) -> Result<()> {
                let bytes = ((0 as $tt).leading_zeros() / 8 - value.leading_zeros() / 8) as usize;
                match bytes > $N {
                    true => fail!("value {} is bigger than {} bytes", value, $N),
                    false => Ok(()),
                }
            }
            pub fn get_len(&self) -> usize {
                let bits = 8 - ($N as u8).leading_zeros();
                let bytes = ((0 as $tt).leading_zeros() / 8 - self.0.leading_zeros() / 8) as usize;
                bits as usize + bytes * 8
            }
            pub const fn inner(&self) -> $tt {
                self.0
            }
        }

        impl Serializable for $varname {
            fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
                let bits = 8 - ($N as u8).leading_zeros();
                let bytes = ((0 as $tt).leading_zeros() / 8 - self.0.leading_zeros() / 8) as usize;
                if bytes > $N {
                    fail!(
                        "cannot store {} {}, required {} bytes",
                        self,
                        stringify!($varname),
                        bytes
                    )
                }
                cell.append_bits(bytes, bits as usize)?;
                let be_bytes = self.0.to_be_bytes();
                cell.append_raw(&be_bytes[be_bytes.len() - bytes..], bytes * 8)?;
                Ok(())
            }
        }

        impl Deserializable for $varname {
            fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
                let bits = 8 - ($N as u8).leading_zeros();
                let bytes = slice.get_next_int(bits as usize)? as usize;
                let max = std::mem::size_of::<$tt>();
                let mut buffer = [0; std::mem::size_of::<$tt>()];
                if bytes > 0 {
                    let first = slice.get_next_byte()?;
                    if first == 0 {
                        fail!("non-canonical {} encoding: leading zero byte", stringify!($varname))
                    }
                    buffer[max - bytes] = first;
                    if bytes > 1 {
                        slice.get_next_bytes_to_slice(&mut buffer[max - bytes + 1..])?;
                    }
                }
                self.0 = <$tt>::from_be_bytes(buffer);
                Ok(())
            }
            fn skip(slice: &mut SliceData) -> Result<()> {
                let bits = 8 - ($N as u8).leading_zeros();
                let bytes = slice.get_next_int(bits as usize)?;
                slice.move_by(8 * bytes as usize)?;
                Ok(())
            }
        }

        impl AddSub for $varname {
            fn add(&mut self, other: &Self) -> Result<bool> {
                Ok(self.add_checked(other))
            }
            fn sub(&mut self, other: &Self) -> Result<bool> {
                Ok(self.sub_checked(other))
            }
        }

        impl From<$varname> for $tt {
            fn from(value: $varname) -> Self {
                value.0
            }
        }

        impl std::convert::TryFrom<$tt> for $varname {
            type Error = crate::Error;
            fn try_from(value: $tt) -> Result<Self> {
                Self::check_overflow(&value)?;
                Ok(Self(value))
            }
        }

        impl fmt::Display for $varname {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "{}", &self.0)
            }
        }

        impl std::ops::Mul<$tt> for $varname {
            type Output = Self;
            fn mul(mut self, rhs: $tt) -> Self::Output {
                self.0 *= rhs;
                self
            }
        }

        impl std::ops::MulAssign<$tt> for $varname {
            fn mul_assign(&mut self, rhs: $tt) {
                self.0 *= rhs;
            }
        }

        impl std::ops::Mul for $varname {
            type Output = Self;
            fn mul(mut self, rhs: Self) -> Self::Output {
                self.0 *= rhs.0;
                self
            }
        }

        impl std::ops::MulAssign for $varname {
            fn mul_assign(&mut self, rhs: Self) {
                self.0 *= rhs.0;
            }
        }

        impl std::ops::Div<$tt> for $varname {
            type Output = Self;
            fn div(mut self, rhs: $tt) -> Self::Output {
                self.0 /= rhs;
                self
            }
        }

        impl std::ops::DivAssign<$tt> for $varname {
            fn div_assign(&mut self, rhs: $tt) {
                self.0 /= rhs;
            }
        }

        impl std::ops::Div for $varname {
            type Output = Self;
            fn div(mut self, rhs: Self) -> Self::Output {
                self.0 /= rhs.0;
                self
            }
        }

        impl std::ops::DivAssign for $varname {
            fn div_assign(&mut self, rhs: Self) {
                self.0 /= rhs.0;
            }
        }

        impl std::ops::Shr<u8> for $varname {
            type Output = Self;
            fn shr(mut self, rhs: u8) -> Self::Output {
                self.0 >>= rhs;
                self
            }
        }

        impl std::ops::ShrAssign<u8> for $varname {
            fn shr_assign(&mut self, rhs: u8) {
                self.0 >>= rhs;
            }
        }

        impl std::ops::Shl<u8> for $varname {
            type Output = Self;
            fn shl(mut self, rhs: u8) -> Self {
                self.0 <<= rhs;
                self
            }
        }

        impl std::ops::ShlAssign<u8> for $varname {
            fn shl_assign(&mut self, rhs: u8) {
                self.0 <<= rhs;
            }
        }

        impl num::CheckedAdd for $varname {
            fn checked_add(&self, rhs: &Self) -> Option<Self> {
                if let Some(result) = self.0.checked_add(rhs.0) {
                    if Self::check_overflow(&result).is_ok() {
                        return Some(Self(result));
                    }
                }
                None
            }
        }

        impl std::ops::Add<$tt> for $varname {
            type Output = Self;
            fn add(mut self, rhs: $tt) -> Self {
                self.0 += rhs;
                self
            }
        }

        impl std::ops::AddAssign<$tt> for $varname {
            fn add_assign(&mut self, rhs: $tt) {
                self.0 += rhs;
            }
        }

        impl std::ops::Add for $varname {
            type Output = Self;
            fn add(mut self, rhs: Self) -> Self {
                self.0 += rhs.0;
                self
            }
        }

        impl std::ops::AddAssign for $varname {
            fn add_assign(&mut self, rhs: Self) {
                self.0 += rhs.0;
            }
        }

        impl num::CheckedSub for $varname {
            fn checked_sub(&self, rhs: &Self) -> Option<Self> {
                Some(Self(self.0.checked_sub(rhs.0)?))
            }
        }

        impl std::ops::Sub<$tt> for $varname {
            type Output = Self;
            fn sub(mut self, rhs: $tt) -> Self {
                self.0 -= rhs;
                self
            }
        }

        impl std::ops::SubAssign<$tt> for $varname {
            fn sub_assign(&mut self, rhs: $tt) {
                self.0 -= rhs;
            }
        }

        impl std::ops::Sub for $varname {
            type Output = Self;
            fn sub(mut self, rhs: Self) -> Self {
                self.0 -= rhs.0;
                self
            }
        }

        impl std::ops::SubAssign for $varname {
            fn sub_assign(&mut self, rhs: Self) {
                self.0 -= rhs.0;
            }
        }

        impl PartialEq<$tt> for $varname {
            fn eq(&self, other: &$tt) -> bool {
                self.0.cmp(other) == std::cmp::Ordering::Equal
            }
        }

        impl PartialOrd<$tt> for $varname {
            fn partial_cmp(&self, other: &$tt) -> Option<std::cmp::Ordering> {
                Some(self.0.cmp(other))
            }
        }
    };
}

// Coins is `VarUInteger 16`, but macro is defined in the way,
// that it uses `$N.leading_zeros` to define type length, so length 15 is used here
define_VarIntegerN!(Coins, 15, u128);
// Macro which defines the type using BigInt as underlying type,
// calculates length with `($N - 1).log2 + 1`, so proper length is used here
define_VarIntegerN!(VarUInteger32, 32, BigInt);
define_VarIntegerN!(VarUInteger3, 3, u32);
define_VarIntegerN!(VarUInteger7, 7, u64);

pub type VarUInteger16 = Coins;

impl Augmentable for Coins {
    fn calc(&mut self, other: &Self) -> Result<bool> {
        self.add(other)
    }
}

// it cannot produce problem
impl From<u64> for Coins {
    fn from(value: u64) -> Self {
        Self(value as u128)
    }
}

impl PartialEq<u64> for Coins {
    fn eq(&self, other: &u64) -> bool {
        self.0 == (*other as u128)
    }
}
impl PartialOrd<u64> for Coins {
    fn partial_cmp(&self, other: &u64) -> Option<std::cmp::Ordering> {
        Some(self.0.cmp(&(*other as u128)))
    }
}

// it cannot produce problem
impl From<u16> for VarUInteger3 {
    fn from(value: u16) -> Self {
        Self(value as u32)
    }
}

// it cannot produce problem
impl From<u32> for VarUInteger7 {
    fn from(value: u32) -> Self {
        Self(value as u64)
    }
}

impl VarUInteger7 {
    pub const fn new(value: u32) -> Self {
        Self(value as u64)
    }
    pub const fn as_u64(&self) -> u64 {
        self.0
    }
}

impl VarUInteger3 {
    pub const fn new(value: u16) -> Self {
        Self(value as u32)
    }
    pub const fn as_u32(&self) -> u32 {
        self.0
    }
    pub const fn as_u64(&self) -> u64 {
        self.0 as u64
    }
}

impl Coins {
    pub const fn new(value: u64) -> Self {
        Self(value as u128)
    }
    pub const fn as_u128(&self) -> u128 {
        self.0
    }
    pub const fn as_u64(&self) -> Option<u64> {
        if self.0 <= u64::MAX as u128 {
            Some(self.0 as u64)
        } else {
            None
        }
    }
}

impl FromStr for Coins {
    type Err = crate::Error;

    fn from_str(string: &str) -> Result<Self> {
        if let Some(stripped) = string.strip_prefix("0x") {
            Ok(Self(u128::from_str_radix(stripped, 16)?))
        } else {
            Ok(Self(string.parse::<u128>()?))
        }
    }
}

///////////////////////////////////////////////////////////////////////////////
///
/// number ## N
/// n<=X
///
macro_rules! define_NumberN_up32bit {
    ( $varname:ident, $N:expr ) => {
        #[derive(PartialEq, Eq, Hash, Clone, Copy, Debug, Default, PartialOrd, Ord)]
        pub struct $varname(u32);

        #[allow(dead_code)]
        impl $varname {
            pub fn new_checked(value: u32, max_value: u32) -> Result<Self> {
                if value > max_value {
                    fail!($crate::BlockError::InvalidArg(format!(
                        "value: {} must be <= {}",
                        value, max_value
                    )))
                }
                Ok($varname(value))
            }

            pub fn new(value: u32) -> Result<Self> {
                let max_value = Self::get_max_value();
                Self::new_checked(value, max_value)
            }

            pub const fn as_u8(&self) -> u8 {
                self.0 as u8
            }

            pub const fn as_u16(&self) -> u16 {
                self.0 as u16
            }

            pub const fn as_u32(&self) -> u32 {
                self.0
            }

            pub const fn as_usize(&self) -> usize {
                self.0 as usize
            }

            pub const fn get_max_len() -> usize {
                (((1 as u64) << $N) - 1) as usize
            }

            pub const fn get_max_value() -> u32 {
                (((1 as u64) << $N) - 1) as u32
            }
        }

        impl Serializable for $varname {
            fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
                cell.append_bits(self.0 as usize, $N)?;
                Ok(())
            }
        }

        impl Deserializable for $varname {
            fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
                self.0 = cell.get_next_int($N)? as u32;
                Ok(())
            }
            fn skip(slice: &mut SliceData) -> Result<()> {
                slice.move_by($N)?;
                Ok(())
            }
        }

        impl fmt::Display for $varname {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "vui{}[value = {}]", $N, self.0)
            }
        }

        impl PartialEq<u32> for $varname {
            fn eq(&self, other: &u32) -> bool {
                &self.0 == other
            }
        }

        impl PartialOrd<u32> for $varname {
            fn partial_cmp(&self, other: &u32) -> Option<std::cmp::Ordering> {
                Some(self.0.cmp(other))
            }
        }

        impl Deref for $varname {
            type Target = u32;
            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }
    };
}
define_NumberN_up32bit!(Number5, 5);
define_NumberN_up32bit!(Number8, 8);
define_NumberN_up32bit!(Number9, 9);
define_NumberN_up32bit!(Number12, 12);
define_NumberN_up32bit!(Number13, 13);
define_NumberN_up32bit!(Number16, 16);
define_NumberN_up32bit!(Number32, 32);

define_HashmapE! {ExtraCurrencyCollection, 32, VarUInteger32}

impl From<HashmapE> for ExtraCurrencyCollection {
    fn from(other: HashmapE) -> Self {
        Self::with_hashmap(other.data().cloned())
    }
}

impl From<u8> for Number8 {
    fn from(value: u8) -> Self {
        Self(value as u32)
    }
}

impl From<u8> for Number9 {
    fn from(value: u8) -> Self {
        Self(value as u32)
    }
}

impl From<u8> for Number12 {
    fn from(value: u8) -> Self {
        Self(value as u32)
    }
}

impl From<u8> for Number13 {
    fn from(value: u8) -> Self {
        Self(value as u32)
    }
}

impl From<u16> for Number16 {
    fn from(value: u16) -> Self {
        Self(value as u32)
    }
}

impl From<u32> for Number32 {
    fn from(value: u32) -> Self {
        Self(value)
    }
}

impl TryFrom<u32> for Number5 {
    type Error = Error;
    fn try_from(value: u32) -> Result<Self> {
        Self::new(value)
    }
}

impl TryFrom<u32> for Number8 {
    type Error = Error;
    fn try_from(value: u32) -> Result<Self> {
        Self::new(value)
    }
}

impl TryFrom<u32> for Number9 {
    type Error = Error;
    fn try_from(value: u32) -> Result<Self> {
        Self::new(value)
    }
}

impl TryFrom<u32> for Number12 {
    type Error = Error;
    fn try_from(value: u32) -> Result<Self> {
        Self::new(value)
    }
}

impl TryFrom<u32> for Number13 {
    type Error = Error;
    fn try_from(value: u32) -> Result<Self> {
        Self::new(value)
    }
}

impl TryFrom<u32> for Number16 {
    type Error = Error;
    fn try_from(value: u32) -> Result<Self> {
        Self::new(value)
    }
}

/*
extra_currencies$_
    dict:(HashMapE 32 (VarUInteger 32))
= ExtraCurrencyCollection;

currencies$_
    coins: Coins
    other:ExtraCurrencyCollection
= CurrencyCollection;
*/
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CurrencyCollection {
    pub coins: Coins,
    pub other: ExtraCurrencyCollection,
}

impl Augmentable for CurrencyCollection {
    fn calc(&mut self, other: &Self) -> Result<bool> {
        self.add(other)
    }
}

impl CurrencyCollection {
    pub fn get_other(&self, key: u32) -> Result<Option<VarUInteger32>> {
        self.other.get(&key)
    }

    pub fn set_other(&mut self, key: u32, other: u128) -> Result<()> {
        self.set_other_ex(key, &VarUInteger32::from_two_u128(0, other)?)?;
        Ok(())
    }

    pub fn set_other_ex(&mut self, key: u32, other: &VarUInteger32) -> Result<()> {
        self.other.set(&key, other)?;
        Ok(())
    }

    pub const fn new() -> Self {
        CurrencyCollection { coins: Coins::zero(), other: ExtraCurrencyCollection::new() }
    }

    pub const fn with_coins(coins: u64) -> Self {
        Self::from_coins(Coins::new(coins))
    }

    pub const fn from_coins(coins: Coins) -> Self {
        CurrencyCollection { coins, other: ExtraCurrencyCollection::new() }
    }

    pub fn is_zero(&self) -> Result<bool> {
        if !self.coins.is_zero() {
            return Ok(false);
        }
        self.other.iterate(|value| Ok(value.is_zero()))
    }
    pub fn remove_zero_currencies(&mut self) -> Result<()> {
        self.other.retire(|value| !value.is_zero())?;
        Ok(())
    }
}

impl Serializable for CurrencyCollection {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.coins.write_to(cell)?;
        self.other.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for CurrencyCollection {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.coins.read_from(cell)?;
        self.other.read_from(cell)?;
        Ok(())
    }
    fn skip(slice: &mut SliceData) -> Result<()> {
        Coins::skip(slice)?;
        ExtraCurrencyCollection::skip(slice)?;
        Ok(())
    }
}

pub trait AddSub {
    fn sub(&mut self, other: &Self) -> Result<bool>;
    fn add(&mut self, other: &Self) -> Result<bool>;
}

impl AddSub for CurrencyCollection {
    fn sub(&mut self, other: &Self) -> Result<bool> {
        if !self.coins.sub(&other.coins)? {
            return Ok(false);
        }
        other.other.iterate_with_keys(|key: u32, b| -> Result<bool> {
            if let Some(mut a) = self.other.get(&key)? {
                if a >= b {
                    a.sub(&b)?;
                    self.other.set(&key, &a)?;
                    return Ok(true);
                }
            }
            Ok(false) // coin not found in mine or amount is smaller - cannot subtract
        })
    }
    fn add(&mut self, other: &Self) -> Result<bool> {
        self.coins.add(&other.coins)?;
        let mut result = self.other.clone();
        other.other.iterate_with_keys(|key: u32, b| -> Result<bool> {
            match self.other.get(&key)? {
                Some(mut a) => {
                    a.add(&b)?;
                    result.set(&key, &a)?;
                }
                None => {
                    result.set(&key, &b)?;
                }
            }
            Ok(true)
        })?;
        self.other = result;
        Ok(true)
    }
}

impl fmt::Display for CurrencyCollection {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.coins)?;
        if !self.other.is_empty() {
            let mut len = 0;
            write!(f, ", other: {{")?;
            self.other
                .iterate_with_keys(|key: u32, value| {
                    len += 1;
                    write!(f, " {} => {},", key, value.0)?;
                    Ok(true)
                })
                .ok();
            write!(f, " count: {} }}", len)?;
        }
        Ok(())
    }
}

impl From<u64> for CurrencyCollection {
    fn from(value: u64) -> Self {
        Self::with_coins(value)
    }
}

impl Serializable for u64 {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_u64(*self)?;
        Ok(())
    }
}

impl Deserializable for u64 {
    fn construct_from(slice: &mut SliceData) -> Result<Self> {
        slice.get_next_u64()
    }
    fn skip(slice: &mut SliceData) -> Result<()> {
        slice.move_by(Self::BITS as usize)
    }
}

impl Serializable for u8 {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_u8(*self)?;
        Ok(())
    }
}

impl Deserializable for u8 {
    fn construct_from(slice: &mut SliceData) -> Result<Self> {
        slice.get_next_byte()
    }
    fn skip(slice: &mut SliceData) -> Result<()> {
        slice.move_by(Self::BITS as usize)
    }
}

impl Serializable for i32 {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_i32(*self)?;
        Ok(())
    }
}

impl Deserializable for u32 {
    fn construct_from(slice: &mut SliceData) -> Result<Self> {
        slice.get_next_u32()
    }
    fn skip(slice: &mut SliceData) -> Result<()> {
        slice.move_by(Self::BITS as usize)
    }
}

impl Serializable for u32 {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_u32(*self)?;
        Ok(())
    }
}

impl Serializable for u128 {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_u128(*self)?;
        Ok(())
    }
}

impl Deserializable for i32 {
    fn construct_from(slice: &mut SliceData) -> Result<Self> {
        slice.get_next_i32()
    }
    fn skip(slice: &mut SliceData) -> Result<()> {
        slice.move_by(Self::BITS as usize)
    }
}

impl Serializable for i8 {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_i8(*self)?;
        Ok(())
    }
}

impl Deserializable for i8 {
    fn construct_from(slice: &mut SliceData) -> Result<Self> {
        slice.get_next_byte().map(|v| v as i8)
    }
    fn skip(slice: &mut SliceData) -> Result<()> {
        slice.move_by(Self::BITS as usize)
    }
}

impl Serializable for i16 {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_i16(*self)?;
        Ok(())
    }
}

impl Deserializable for i16 {
    fn construct_from(slice: &mut SliceData) -> Result<Self> {
        slice.get_next_i16()
    }
    fn skip(slice: &mut SliceData) -> Result<()> {
        slice.move_by(Self::BITS as usize)
    }
}

impl Serializable for u16 {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_u16(*self)?;
        Ok(())
    }
}

impl Deserializable for u16 {
    fn construct_from(slice: &mut SliceData) -> Result<Self> {
        slice.get_next_u16()
    }
    fn skip(slice: &mut SliceData) -> Result<()> {
        slice.move_by(Self::BITS as usize)
    }
}

impl Serializable for bool {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_bit_bool(*self)?;
        Ok(())
    }
}

impl Deserializable for bool {
    fn construct_from(slice: &mut SliceData) -> Result<Self> {
        slice.get_next_bit()
    }
    fn skip(slice: &mut SliceData) -> Result<()> {
        slice.move_by(1)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InRefValue<X: Deserializable + Serializable>(pub X);

impl<X: Deserializable + Serializable> InRefValue<X> {
    pub fn new(inner: X) -> InRefValue<X> {
        InRefValue(inner)
    }
    pub fn inner(self) -> X {
        self.0
    }
}

impl<X: Deserializable + Serializable> AsRef<X> for InRefValue<X> {
    fn as_ref(&self) -> &X {
        &self.0
    }
}

impl<X: Deserializable + Serializable> Deref for InRefValue<X> {
    type Target = X;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<X: Deserializable + Serializable> DerefMut for InRefValue<X> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<X: Deserializable + Serializable> Deserializable for InRefValue<X> {
    fn construct_from(slice: &mut SliceData) -> Result<Self> {
        Ok(Self(X::construct_from_reference(slice)?))
    }
}

impl<X: Deserializable + Serializable> Serializable for InRefValue<X> {
    fn write_to(&self, builder: &mut BuilderData) -> Result<()> {
        builder.checked_append_reference(self.0.serialize()?)?;
        Ok(())
    }
}

// Global timeshift in seconds, used for testing purposes
#[cfg(feature = "mirrornet")]
static TIMESHIFT_SEC: AtomicI64 = AtomicI64::new(0);

#[derive(PartialEq, Copy, Clone, Debug, Eq, Default, Hash)]
pub struct UnixTime;

impl UnixTime {
    pub fn now() -> u64 {
        #[cfg(feature = "mirrornet")]
        let timeshift = Self::timeshift_sec();
        #[cfg(not(feature = "mirrornet"))]
        let timeshift = 0;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|_| Duration::from_secs(0))
            .as_secs() as i64;
        (now + timeshift) as u64
    }

    pub fn now_f64() -> f64 {
        #[cfg(feature = "mirrornet")]
        let timeshift = Self::timeshift_sec() as f64;
        #[cfg(not(feature = "mirrornet"))]
        let timeshift = 0.0;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|_| Duration::from_secs(0))
            .as_secs_f64();
        now + timeshift
    }

    pub fn now_ms() -> u64 {
        #[cfg(feature = "mirrornet")]
        let timeshift = Self::timeshift_sec();
        #[cfg(not(feature = "mirrornet"))]
        let timeshift = 0;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|_| Duration::from_secs(0))
            .as_millis() as i64;
        (now + timeshift * 1000) as u64
    }

    #[cfg(feature = "mirrornet")]
    pub fn set_timeshift(timeshift_sec: i64) -> Result<()> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|_| Duration::from_secs(0))
            .as_secs() as i64;
        if timeshift_sec.abs() > now {
            fail!("timeshift {} is too big, current time is {}", timeshift_sec, now);
        }
        TIMESHIFT_SEC
            .compare_exchange(0, timeshift_sec, Ordering::Relaxed, Ordering::Relaxed)
            .map_err(|_| BlockError::InvalidArg("timeshift can be set only once".into()))?;
        Ok(())
    }

    #[cfg(feature = "mirrornet")]
    pub fn timeshift_sec() -> i64 {
        TIMESHIFT_SEC.load(Ordering::Relaxed)
    }
}

pub struct TimeChecker<F, D>
where
    F: Fn() -> D,
    D: Display,
{
    operation: F,
    target: &'static str,
    threshold: Duration,
    start: Instant,
}

impl<F, D> TimeChecker<F, D>
where
    F: Fn() -> D,
    D: Display,
{
    pub fn new(target: &'static str, operation: F, threshold_ms: u64) -> Self {
        let start = Instant::now();
        log::trace!(target: target, "{} - started", operation());
        Self { operation, target, threshold: Duration::from_millis(threshold_ms), start }
    }
}

impl<F, D> Drop for TimeChecker<F, D>
where
    F: Fn() -> D,
    D: Display,
{
    fn drop(&mut self) {
        let time = self.start.elapsed();
        if time < self.threshold {
            log::trace!(
                target: self.target,
                "{} - finished, TIME: {}",
                (self.operation)(),
                time.as_millis()
            );
        } else {
            log::warn!(
                target: self.target,
                "{} - finished too slow, TIME: {}ms, expected: {}ms",
                (self.operation)(),
                time.as_millis(),
                self.threshold.as_millis()
            );
        }
    }
}

#[macro_export]
macro_rules! time_checker {
    ($op:expr, $threshold:expr) => {
        $crate::TimeChecker::new(module_path!(), $op, $threshold)
    };
}

#[derive(Debug, Default, Clone, Eq)]
pub struct ChildCell<T: Serializable + Deserializable> {
    cell: Option<Cell>,
    phantom: PhantomData<T>,
}

impl<T: Serializable + Deserializable> ChildCell<T> {
    pub fn with_cell(cell: Cell) -> Self {
        Self { cell: Some(cell), phantom: PhantomData }
    }
    pub fn with_struct(s: &T) -> Result<Self> {
        Ok(Self::with_cell(s.serialize()?))
    }

    pub fn write_struct(&mut self, s: &T) -> Result<()> {
        self.cell = Some(s.serialize()?);
        Ok(())
    }

    pub fn read_struct(&self) -> Result<T> {
        match self.cell.clone() {
            Some(cell) => {
                if cell.cell_type() == CellType::PrunedBranch {
                    fail!(BlockError::PrunedCellAccess(type_name::<T>().into()))
                }
                T::construct_from_cell(cell)
            }
            None => Ok(T::default()),
        }
    }

    pub fn cell(&self) -> Cell {
        match self.cell.as_ref() {
            Some(cell) => cell.clone(),
            None => T::default().serialize().unwrap_or_default(),
        }
    }

    pub fn set_cell(&mut self, cell: Cell) {
        self.cell = Some(cell);
    }

    pub fn hash(&self) -> UInt256 {
        match self.cell.as_ref() {
            Some(cell) => cell.repr_hash().clone(),
            None => T::default().serialize().unwrap_or_default().repr_hash().clone(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.cell.is_none()
    }
}

impl<T: Default + Serializable + Deserializable> PartialEq for ChildCell<T> {
    fn eq(&self, other: &Self) -> bool {
        if self.cell == other.cell {
            return true;
        }
        match (self.cell.as_ref(), other.cell.as_ref()) {
            (Some(cell), Some(other)) => cell.eq(other),
            (None, Some(cell)) | (Some(cell), None) => {
                cell.eq(&T::default().serialize().unwrap_or_default())
            }
            (None, None) => true,
        }
    }
}

impl<T: Serializable + Deserializable> Serializable for ChildCell<T> {
    fn write_to(&self, builder: &mut BuilderData) -> Result<()> {
        if let Some(child_cell) = &self.cell {
            builder.checked_append_reference(child_cell.clone())?;
        } else {
            builder.checked_append_reference(T::default().serialize()?)?;
        }
        Ok(())
    }
}

impl<T: Serializable + Deserializable> Deserializable for ChildCell<T> {
    fn construct_from(slice: &mut SliceData) -> Result<Self> {
        Ok(Self::with_cell(slice.checked_drain_reference()?))
    }
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        *self = Self::construct_from(slice)?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/test_types.rs"]
mod tests;
