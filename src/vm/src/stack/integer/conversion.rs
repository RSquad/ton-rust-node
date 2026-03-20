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
use crate::stack::integer::{
    utils::{check_overflow, twos_complement},
    Int, IntegerData,
};
use num_traits::Num;
use std::ops::RangeInclusive;
use ton_block::{error, fail, Error, ExceptionCode, Result};

impl IntegerData {
    /// Constructs new IntegerData from u32 in a fastest way.
    #[inline]
    pub fn from_u32(value: u32) -> IntegerData {
        if value == 0 {
            return Self::zero();
        }
        IntegerData { value: Some(Int::new(num::bigint::Sign::Plus, vec![value])) }
    }

    /// Constructs new IntegerData from i32 in a fastest way.
    #[inline]
    pub fn from_i32(value: i32) -> IntegerData {
        if value == 0 {
            return Self::zero();
        }
        IntegerData {
            value: Some(Int::new(
                if value < 0 { num::bigint::Sign::Minus } else { num::bigint::Sign::Plus },
                vec![value.unsigned_abs()],
            )),
        }
    }

    /// Constructs new IntegerData from u64 in a fastest way.
    #[inline]
    pub fn from_u64(value: u64) -> IntegerData {
        IntegerData { value: Some(Int::from(value)) }
    }

    /// Constructs new IntegerData from i64 in a fastest way.
    #[inline]
    pub fn from_i64(value: i64) -> IntegerData {
        IntegerData { value: Some(Int::from(value)) }
    }

    /// Constructs new IntegerData from u128 in a fastest way.
    #[inline]
    pub fn from_u128(value: u128) -> IntegerData {
        IntegerData { value: Some(Int::from(value)) }
    }

    /// Constructs new IntegerData from i128 in a fastest way.
    #[inline]
    pub fn from_i128(value: i128) -> IntegerData {
        IntegerData { value: Some(Int::from(value)) }
    }

    #[inline]
    pub fn from_usize(value: usize) -> IntegerData {
        IntegerData { value: Some(Int::from(value)) }
    }

    /// Constructs new IntegerData value from the given one of another supported type.
    #[inline]
    pub fn from(value: impl Into<Int>) -> Result<IntegerData> {
        let bigint = value.into();
        match check_overflow(&bigint) {
            true => Ok(IntegerData { value: Some(bigint) }),
            false => fail!(ExceptionCode::IntegerOverflow),
        }
    }

    /// Constructs new IntegerData value from the little-endian slice of u32
    /// without overflow checking.
    #[inline]
    pub fn from_vec_le_unchecked(sign: num::bigint::Sign, digits: Vec<u32>) -> IntegerData {
        IntegerData { value: Some(Int::new(sign, digits)) }
    }

    /// Constructs new IntegerData value from the little-endian slice of u32
    /// with overflow checking.
    #[inline]
    pub fn from_vec_le(sign: num::bigint::Sign, digits: Vec<u32>) -> Result<IntegerData> {
        let bigint = Int::new(sign, digits);
        if !check_overflow(&bigint) {
            fail!(ExceptionCode::IntegerOverflow);
        }
        Ok(IntegerData { value: Some(bigint) })
    }

    /// Parses string literal with given radix and constructs new IntegerData.
    pub fn from_str_radix(literal: &str, radix: u32) -> Result<IntegerData> {
        let s = if let Some(tail) = literal.strip_prefix("0x") { tail } else { literal };
        match Int::from_str_radix(s, radix) {
            Ok(value) => Self::from(value),
            Err(_) => fail!(
                ExceptionCode::TypeCheckError,
                "cannot parse integer from {s} and radix {radix}"
            ),
        }
    }

    /// Returns value converted into given type with range checking.
    pub fn as_integer_value<T>(&self, range: RangeInclusive<T>) -> Result<T>
    where
        T: PartialOrd + std::fmt::Display + FromInt,
    {
        match &self.value {
            None => fail!(ExceptionCode::RangeCheckError, "not a number"),
            Some(value) => T::from_int(value).and_then(|ret| {
                if *range.start() > ret || *range.end() < ret {
                    fail!(
                        ExceptionCode::RangeCheckError,
                        "{} is not in the range {}..={}",
                        ret,
                        range.start(),
                        range.end()
                    );
                }
                Ok(ret)
            }),
        }
    }

    /// Extracts internal value using conversion function. Returns IntegerOverflow exception on NaN.
    #[inline]
    pub fn take_value_of<T>(&self, convert: impl Fn(&Int) -> Option<T>) -> Result<T> {
        match &self.value {
            None => fail!(ExceptionCode::IntegerOverflow, "not a number"),
            Some(value) => {
                if let Some(value) = convert(value) {
                    Ok(value)
                } else {
                    fail!(ExceptionCode::RangeCheckError, "cannot convert {}", value)
                }
            }
        }
    }
}

impl std::str::FromStr for IntegerData {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Self::from_str_radix(s, 10)
    }
}

impl IntegerData {
    /// Decodes value from big endian octet string for PUSHINT primitive using the format
    /// from TVM Spec A.3.1:
    ///  "82lxxx — PUSHINT xxx, where 5-bit 0 ≤ l ≤ 30 determines the length n = 8l + 19
    ///  of signed big-endian integer xxx. The total length of this instruction
    ///  is l + 4 bytes or n + 13 = 8l + 32 bits."
    pub fn from_big_endian_octet_stream<F>(mut get_next_byte: F) -> Result<IntegerData>
    where
        F: FnMut() -> Result<u8>,
    {
        let first_byte = get_next_byte()?;
        let byte_len = ((first_byte & 0b11111000u8) as usize >> 3) + 3;
        let greatest3bits = (first_byte & 0b111) as u32;
        let digit_count = (byte_len + 3) >> 2;
        let mut digits: Vec<u32> = vec![0; digit_count];
        let (sign, mut value) = if greatest3bits & 0b100 == 0 {
            (num::bigint::Sign::Plus, greatest3bits)
        } else {
            (num::bigint::Sign::Minus, 0xFFFF_FFF8u32 | greatest3bits)
        };

        let mut upper = byte_len & 0b11;
        if upper == 0 {
            upper = 4;
        }
        for _ in 1..upper {
            value <<= 8;
            value |= get_next_byte()? as u32;
        }
        let last_index = digit_count - 1;
        digits[last_index] = value;

        for i in (0..last_index).rev() {
            let mut value = (get_next_byte()? as u32) << 24;
            value |= (get_next_byte()? as u32) << 16;
            value |= (get_next_byte()? as u32) << 8;
            value |= get_next_byte()? as u32;

            digits[i] = value;
        }

        if sign == num::bigint::Sign::Minus {
            twos_complement(&mut digits);
        }
        Ok(IntegerData::from_vec_le_unchecked(sign, digits))
    }
}

impl From<u16> for IntegerData {
    fn from(value: u16) -> Self {
        IntegerData::from_u32(value as u32)
    }
}

impl From<u32> for IntegerData {
    fn from(value: u32) -> Self {
        IntegerData::from_u32(value)
    }
}

impl From<i32> for IntegerData {
    fn from(value: i32) -> Self {
        IntegerData::from_i32(value)
    }
}

impl From<u64> for IntegerData {
    fn from(value: u64) -> Self {
        IntegerData::from_u64(value)
    }
}

impl From<usize> for IntegerData {
    fn from(value: usize) -> Self {
        IntegerData::from_usize(value)
    }
}

impl From<i64> for IntegerData {
    fn from(value: i64) -> Self {
        IntegerData::from_i64(value)
    }
}

impl From<u128> for IntegerData {
    fn from(value: u128) -> Self {
        IntegerData::from_u128(value)
    }
}

impl From<i128> for IntegerData {
    fn from(value: i128) -> Self {
        IntegerData::from_i128(value)
    }
}

impl From<Int> for IntegerData {
    fn from(value: Int) -> Self {
        debug_assert!(value.bits() <= 256);
        IntegerData { value: Some(value) }
    }
}

pub trait FromInt {
    fn from_int(value: &Int) -> Result<Self>
    where
        Self: std::marker::Sized;
}

macro_rules! auto_from_int {
    ($($to:ty : $f:tt),+) => {
        $(
            impl FromInt for $to {
                fn from_int(value: &Int) -> Result<$to> {
                    <dyn num::ToPrimitive>::$f(value).ok_or_else(|| error!(
                        ExceptionCode::RangeCheckError,
                        "{} cannot be converted to {}", value, std::any::type_name::<$to>()
                    ))
                }
            }
        )*
    }
}

auto_from_int! {
    u8: to_u8,
    i8: to_i8,
    u16: to_u16,
    i16: to_i16,
    u32: to_u32,
    i32: to_i32,
    u64: to_u64,
    i64: to_i64,
    u128: to_u128,
    i128: to_i128,
    usize: to_usize,
    isize: to_isize
}
