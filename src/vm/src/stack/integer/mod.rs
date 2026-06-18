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
use crate::stack::integer::behavior::{OperationBehavior, Quiet, Signaling};
use num_traits::{One, Signed, Zero};
use ton_block::{fail, BuilderData, ExceptionCode, Result, SliceData};

#[macro_use]
pub mod behavior;
mod fmt;

type Int = num::BigInt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntegerData {
    value: Option<Int>,
}

impl Default for IntegerData {
    fn default() -> Self {
        IntegerData::zero()
    }
}

impl IntegerData {
    /// Constructs new (set to 0) value. This is just a wrapper for Self::zero().
    #[inline]
    pub const fn new() -> IntegerData {
        Self::zero()
    }

    /// Constructs new (set to 0) value.
    #[inline]
    pub const fn zero() -> IntegerData {
        IntegerData { value: Some(Int::ZERO) }
    }

    /// Constructs new (set to 1) value.
    #[inline]
    pub fn one() -> IntegerData {
        IntegerData { value: Some(Int::one()) }
    }

    /// Constructs new (set to -1) value.
    #[inline]
    pub fn minus_one() -> IntegerData {
        let value = Some(Int::from_biguint(num::bigint::Sign::Minus, num::BigUint::one()));
        IntegerData { value }
    }

    /// Constructs new Not-a-Number (NaN) value.
    #[inline]
    pub const fn nan() -> IntegerData {
        IntegerData { value: None }
    }

    /// Constructs mask for bits
    /// it must be refactored to simplify
    pub fn mask(bits: usize) -> Self {
        IntegerData::one()
            .shl::<Quiet>(bits)
            .unwrap()
            .sub::<Quiet>(&IntegerData::one())
            .unwrap_or_default()
    }

    /// Clears value (sets to 0).
    #[inline]
    pub fn withdraw(&mut self) -> IntegerData {
        std::mem::replace(self, IntegerData::new())
    }

    /// Replaces value to a given one.
    #[inline]
    pub fn replace(&mut self, new_value: IntegerData) {
        *self = new_value;
    }

    /// Checks if value is a Not-a-Number (NaN).
    #[inline]
    pub const fn is_nan(&self) -> bool {
        self.value.is_none()
    }

    /// Checks if value is negative (less than zero).
    #[inline]
    pub fn is_neg(&self) -> bool {
        self.value.as_ref().is_some_and(Int::is_negative)
    }

    pub fn check_neg(&self) -> Result<()> {
        match &self.value {
            Some(value) => {
                if value.is_negative() {
                    fail!(ExceptionCode::RangeCheckError, "{} is negative", value)
                } else {
                    Ok(())
                }
            }
            None => fail!(ExceptionCode::RangeCheckError, "not a number"),
        }
    }

    /// Checks if value is zero.
    #[inline]
    pub fn is_zero(&self) -> bool {
        self.value.as_ref().is_some_and(Int::is_zero)
    }

    /// constuct
    pub fn from_unsigned_bytes_be(data: impl AsRef<[u8]>) -> Self {
        Self { value: Some(Int::from_bytes_be(num::bigint::Sign::Plus, data.as_ref())) }
    }

    /// Compares value with another taking in account behavior of operation.
    #[inline]
    pub(crate) fn compare<T: OperationBehavior>(
        &self,
        other: &IntegerData,
    ) -> Result<Option<std::cmp::Ordering>> {
        match (&self.value, &other.value) {
            (Some(l), Some(r)) => Ok(Some(l.cmp(r))),
            _ => {
                on_nan_parameter!(T)?;
                Ok(None)
            }
        }
    }

    /// Returns true if signed value fits into a given bits size; otherwise false.
    #[inline]
    pub fn fits_in(&self, bits: usize) -> Result<bool> {
        Ok(self.bitsize()? <= bits)
    }

    /// Returns true if unsigned value fits into a given bits size; otherwise false.
    #[inline]
    pub fn ufits_in(&self, bits: usize) -> Result<bool> {
        Ok(!self.is_neg() && self.ubitsize()? <= bits)
    }

    /// Determines a fewest bits necessary to express signed value.
    #[inline]
    pub fn bitsize(&self) -> Result<usize> {
        utils::process_value(self, |value| Ok(utils::bitsize(value)))
    }

    /// Determines a fewest bits necessary to express unsigned value.
    #[inline]
    pub fn ubitsize(&self) -> Result<usize> {
        utils::process_value(self, |value| {
            debug_assert!(!value.is_negative());
            Ok(value.bits() as usize)
        })
    }

    pub fn as_int(&self) -> &Option<Int> {
        &self.value
    }

    pub fn as_slice(&self, bits: usize, signed: bool, big_endian: bool) -> Result<SliceData> {
        SliceData::load_bitstring(self.as_builder(bits, signed, big_endian)?)
    }

    pub fn as_builder(&self, bits: usize, signed: bool, big_endian: bool) -> Result<BuilderData> {
        if self.is_nan() {
            Signaling::on_nan_parameter(file!(), line!())?;
        }
        self.try_serialize(bits, signed, big_endian)
    }
    pub fn as_vec(&self, bits: usize, signed: bool, big_endian: bool) -> Result<Vec<u8>> {
        if !self.is_nan() {
            self.try_serialize_to_vec(bits, signed, big_endian)
        } else if bits == 256 {
            fail!(ExceptionCode::RangeCheckError)
        } else {
            fail!(ExceptionCode::IntegerOverflow)
        }
    }
    pub fn as_u256(&self) -> Result<Vec<u8>> {
        self.as_vec(256, false, true)
    }
    pub fn from_u256(data: impl AsRef<[u8]>) -> Result<IntegerData> {
        IntegerData::from_bytes(data, 256, false, true)
    }
}

impl AsRef<IntegerData> for IntegerData {
    #[inline]
    fn as_ref(&self) -> &IntegerData {
        self
    }
}

#[macro_use]
pub mod utils {
    use super::*;
    use std::ops::Not;

    #[inline]
    pub fn process_value<F, R>(value: &IntegerData, call_on_valid: F) -> Result<R>
    where
        F: Fn(&Int) -> Result<R>,
    {
        match &value.value {
            None => {
                fail!(ExceptionCode::IntegerOverflow)
            }
            Some(value) => call_on_valid(value),
        }
    }

    /// This macro extracts internal Int value from IntegerData using given NaN behavior
    /// and NaN constructor.
    macro_rules! extract_value {
        ($T: ident, $v: ident, $nan_constructor: ident) => {
            match &$v.value {
                None => {
                    on_nan_parameter!($T)?;
                    return Ok($nan_constructor());
                }
                Some($v) => $v,
            }
        };
    }

    /// Unary operation. Checks lhs for NaN, unwraps it, calls closure and returns wrapped result.
    #[inline]
    pub fn unary_op<T, F, FNaN, FRes, RInt, R>(
        lhs: &IntegerData,
        callback: F,
        nan_constructor: FNaN,
        result_processor: FRes,
    ) -> Result<R>
    where
        T: OperationBehavior,
        F: Fn(&Int) -> RInt,
        FNaN: Fn() -> R,
        FRes: Fn(RInt, FNaN) -> Result<R>,
    {
        let lhs = extract_value!(T, lhs, nan_constructor);

        result_processor(callback(lhs), nan_constructor)
    }

    /// Binary operation. Checks lhs & rhs for NaN, unwraps them, calls closure and returns wrapped result.
    #[inline]
    pub fn binary_op<T, F, FNaN, FRes, RInt, R>(
        lhs: &IntegerData,
        rhs: &IntegerData,
        callback: F,
        nan_constructor: FNaN,
        result_processor: FRes,
    ) -> Result<R>
    where
        T: OperationBehavior,
        F: Fn(&Int, &Int) -> RInt,
        FNaN: Fn() -> R,
        FRes: Fn(RInt, FNaN) -> Result<R>,
    {
        let lhs = extract_value!(T, lhs, nan_constructor);
        let rhs = extract_value!(T, rhs, nan_constructor);

        result_processor(callback(lhs, rhs), nan_constructor)
    }

    #[inline]
    pub fn process_single_result<T, FNaN>(result: Int, nan_constructor: FNaN) -> Result<IntegerData>
    where
        T: OperationBehavior,
        FNaN: Fn() -> IntegerData,
    {
        IntegerData::from(result).or_else(|_| {
            on_integer_overflow!(T)?;
            Ok(nan_constructor())
        })
    }

    #[inline]
    pub fn process_double_result<T>((r1, r2): (Int, Int)) -> Result<(IntegerData, IntegerData)>
    where
        T: OperationBehavior,
    {
        match (IntegerData::from(r1), IntegerData::from(r2)) {
            (Ok(r1), Ok(r2)) => Ok((r1, r2)),
            _ => {
                on_integer_overflow!(T)?;
                Ok((IntegerData::nan(), IntegerData::nan()))
            }
        }
    }

    /// Integer overflow checking. Returns true, if value fits into IntegerData; otherwise false.
    #[inline]
    pub fn check_overflow(value: &Int) -> bool {
        bitsize(value) < 258
    }

    #[inline]
    pub fn bitsize(value: &Int) -> usize {
        if value.is_zero()
            || (value == &Int::from_biguint(num::bigint::Sign::Minus, num::BigUint::one()))
        {
            return 1;
        }
        let res = value.bits() as usize;
        if value.is_positive() {
            return res + 1;
        }
        // For negative values value.bits() returns correct result only when value is power of 2.
        let mut modpow2 = value.abs();
        modpow2 &= &modpow2 - 1;
        if modpow2.is_zero() {
            return res;
        }
        res + 1
    }

    /// Perform in-place two's complement of the given digit iterator
    /// starting from the least significant byte.
    #[inline]
    pub fn twos_complement<'a, I>(digits: I)
    where
        I: IntoIterator<Item = &'a mut u32>,
    {
        let mut carry = true;
        for d in digits {
            *d = d.not();
            if carry {
                *d = d.wrapping_add(1);
                carry = d.is_zero();
            }
        }
    }
}

#[macro_use]
pub mod conversion;
pub mod bitlogics;
pub mod math;
pub mod serialization;

#[cfg(test)]
#[path = "tests/test_integer.rs"]
mod test_integer;

#[cfg(test)]
#[path = "tests/test_conversion.rs"]
mod test_conversion;

#[cfg(test)]
#[path = "tests/test_formatting.rs"]
mod test_formatting;
