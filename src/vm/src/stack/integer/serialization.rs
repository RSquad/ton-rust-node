/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::stack::IntegerData;
use num::{bigint::ToBigInt, Signed};
use ton_block::{fail, BuilderData, ExceptionCode, Result};

impl IntegerData {
    pub fn try_serialize_to_vec(
        &self,
        bits: usize,
        signed: bool,
        big_endian: bool,
    ) -> Result<Vec<u8>> {
        if !signed {
            self.check_neg()?
        }
        if signed && !self.fits_in(bits)? || !signed && !self.ufits_in(bits)? {
            // Spec. 3.2.7
            // * If the integer x to be serialized is not in the range
            //   −2^(n−1) <= x < 2^(n−1) (for signed integer serialization)
            //   or 0 <= x < 2^n (for unsigned integer serialization),
            //   a range check exception is usually generated
            fail!(ExceptionCode::RangeCheckError, "{} is not fit in {} bits", self, bits)
        }
        let mut value = self.take_value_of(|x| x.to_bigint())?;
        if big_endian {
            let excess_bits = calc_excess_bits(bits);
            if excess_bits != 0 {
                value <<= 8 - excess_bits;
            }
        }
        let bytes = match (signed, big_endian) {
            (true, true) => {
                let buffer = value.to_signed_bytes_be();
                extend_buffer_be(buffer, bits, value.is_negative())
            }
            (true, false) => {
                let buffer = value.to_signed_bytes_le();
                extend_buffer_le(buffer, bits, value.is_negative())
            }
            (false, true) => {
                let (_, buffer) = value.to_bytes_be();
                extend_buffer_be(buffer, bits, false)
            }
            (false, false) => {
                let (_, mut buffer) = value.to_bytes_le();
                let expected_buffer_size = bits_to_bytes(bits);
                debug_assert!(expected_buffer_size >= buffer.len());
                buffer.resize(expected_buffer_size, 0);
                buffer
            }
        };
        Ok(bytes)
    }

    pub fn try_serialize(
        &self,
        bits: usize,
        signed: bool,
        big_endian: bool,
    ) -> Result<BuilderData> {
        let bytes = self.try_serialize_to_vec(bits, signed, big_endian)?;
        BuilderData::with_raw(bytes, bits)
    }

    pub fn from_bytes(
        data: impl AsRef<[u8]>,
        bits: usize,
        signed: bool,
        big_endian: bool,
    ) -> Result<IntegerData> {
        let data = data.as_ref();
        debug_assert!(data.len() * 8 >= bits);
        let mut value = match (signed, big_endian) {
            (true, true) => num::BigInt::from_signed_bytes_be(data),
            (true, false) => num::BigInt::from_signed_bytes_le(data),
            (false, true) => num::BigInt::from_bytes_be(num::bigint::Sign::Plus, data),
            (false, false) => num::BigInt::from_bytes_le(num::bigint::Sign::Plus, data),
        };
        if big_endian {
            let excess_bits = calc_excess_bits(bits);
            if excess_bits != 0 {
                value >>= 8 - excess_bits;
            }
        }
        IntegerData::from(value)
    }
}

/// Calculates fewest byte count needed to fit a given bit count.
#[inline]
pub fn bits_to_bytes(bits: usize) -> usize {
    (bits + 7) >> 3
}

/// Calculates excess bits. Bit count which overflows octet.
#[inline]
pub(crate) fn calc_excess_bits(bits: usize) -> usize {
    bits & 0b111
}

#[inline]
fn get_fill(is_negative: bool) -> u8 {
    if is_negative {
        0xFF
    } else {
        0
    }
}

/// Extends buffer, if needed (big-endian).
#[inline]
pub(crate) fn extend_buffer_be(mut buffer: Vec<u8>, bits: usize, is_negative: bool) -> Vec<u8> {
    let new_len = bits_to_bytes(bits);
    if new_len > buffer.len() {
        let mut new_buffer = vec![get_fill(is_negative); new_len - buffer.len()];
        new_buffer.append(&mut buffer);
        new_buffer
    } else {
        buffer
    }
}

/// Extends buffer, if needed (little-endian).
#[inline]
fn extend_buffer_le(mut buffer: Vec<u8>, bits: usize, is_negative: bool) -> Vec<u8> {
    let new_len = bits_to_bytes(bits);
    if new_len > buffer.len() {
        buffer.resize(new_len, get_fill(is_negative));
    }
    buffer
}

pub trait Deserializer<T> {
    /// Tries to deserialize a value from a bitstring
    /// Returns deserialized value if any and a remaining bitstring
    fn deserialize(&self, data: &[u8]) -> T;
}

pub trait Serializer<T> {
    fn try_serialize(&self, value: &T) -> Result<BuilderData>;
}

#[cfg(test)]
#[path = "tests/test_integer_encoding.rs"]
mod test_integer_encoding;

#[cfg(test)]
#[path = "tests/test_ser_deser.rs"]
mod test_ser_deser;
