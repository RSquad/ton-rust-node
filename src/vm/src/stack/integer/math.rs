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
    behavior::OperationBehavior,
    utils::{
        binary_op, construct_double_nan, construct_single_nan, process_double_result,
        process_single_result, unary_op,
    },
    IntegerData,
};
use num_traits::Zero;
use std::ops::AddAssign;
use ton_block::{Result, Status};

// [x / y] -> (q, r)  :  q*y + r = x  :  |r| < |y|
#[derive(Copy, Clone, Eq, PartialEq)]
pub enum Round {
    Ceil = 0,                    // r and y have opposite sign
    FloorToNegativeInfinity = 1, // r has the same sign as y
    FloorToZero = 3,             // r has the same sign as x
    Nearest = 2,                 // | r |   =<   | y/2 |
}

impl IntegerData {
    /// Creates and returns a copy of the same value with a sign changed to an opposite.
    pub fn neg<T: OperationBehavior>(&self) -> Result<IntegerData> {
        unary_op::<T, _, _, _, _, _>(
            self,
            |x| -x,
            construct_single_nan,
            process_single_result::<T, _>,
        )
    }

    pub fn add<T: OperationBehavior>(&self, other: &IntegerData) -> Result<IntegerData> {
        binary_op::<T, _, _, _, _, _>(
            self,
            other,
            |x, y| x + y,
            construct_single_nan,
            process_single_result::<T, _>,
        )
    }

    pub fn add_assign<T: OperationBehavior>(&mut self, other: &IntegerData) -> Status {
        let lhs = match &mut self.value {
            None => {
                on_nan_parameter!(T)?;
                *self = construct_single_nan();
                return Ok(());
            }
            Some(v) => v,
        };
        let rhs = match &other.value {
            None => {
                on_nan_parameter!(T)?;
                *self = construct_single_nan();
                return Ok(());
            }
            Some(v) => v,
        };
        lhs.add_assign(rhs);
        if !super::utils::check_overflow(lhs) {
            on_integer_overflow!(T)?;
            *self = construct_single_nan();
        }
        Ok(())
    }

    pub fn add_i8<T: OperationBehavior>(&self, other: &i8) -> Result<IntegerData> {
        unary_op::<T, _, _, _, _, _>(
            self,
            |x| x + other,
            construct_single_nan,
            process_single_result::<T, _>,
        )
    }

    pub fn sub<T: OperationBehavior>(&self, other: &IntegerData) -> Result<IntegerData> {
        binary_op::<T, _, _, _, _, _>(
            self,
            other,
            |x, y| x - y,
            construct_single_nan,
            process_single_result::<T, _>,
        )
    }

    pub fn sub_i8<T: OperationBehavior>(&self, other: &i8) -> Result<IntegerData> {
        unary_op::<T, _, _, _, _, _>(
            self,
            |x| x - other,
            construct_single_nan,
            process_single_result::<T, _>,
        )
    }

    pub fn mul<T: OperationBehavior>(&self, other: &IntegerData) -> Result<IntegerData> {
        binary_op::<T, _, _, _, _, _>(
            self,
            other,
            |x, y| x * y,
            construct_single_nan,
            process_single_result::<T, _>,
        )
    }

    pub fn mul_shr256<T: OperationBehavior>(&self, other: &IntegerData) -> Result<IntegerData> {
        binary_op::<T, _, _, _, _, _>(
            self,
            other,
            |x, y| (x * y) >> 256,
            construct_single_nan,
            process_single_result::<T, _>,
        )
    }

    pub fn mul_i8<T: OperationBehavior>(&self, other: &i8) -> Result<IntegerData> {
        unary_op::<T, _, _, _, _, _>(
            self,
            |x| x * other,
            construct_single_nan,
            process_single_result::<T, _>,
        )
    }

    pub fn div<T: OperationBehavior>(
        &self,
        divisor: &IntegerData,
        rounding: Round,
    ) -> Result<(IntegerData, IntegerData)> {
        let divisor = extract_value!(T, divisor, construct_double_nan);
        if divisor.is_zero() {
            on_integer_overflow!(T)?;
            return Ok(construct_double_nan());
        }

        unary_op::<T, _, _, _, _, _>(
            self,
            |dividend| utils::divmod(dividend, divisor, rounding),
            construct_double_nan,
            process_double_result::<T, _>,
        )
    }

    pub fn div_by_shift<T: OperationBehavior>(
        &self,
        shift: usize,
        rounding: Round,
    ) -> Result<(IntegerData, IntegerData)> {
        unary_op::<T, _, _, _, _, _>(
            self,
            |dividend| utils::div_by_shift(dividend, shift, rounding),
            construct_double_nan,
            process_double_result::<T, _>,
        )
    }
}

pub mod utils {

    use crate::stack::integer::{math::Round, Int};
    use num_traits::{One, Signed, Zero};
    use std::cmp::Ordering;

    #[inline]
    pub fn divmod(dividend: &Int, divisor: &Int, rounding: Round) -> (Int, Int) {
        match rounding {
            Round::FloorToNegativeInfinity => num::Integer::div_mod_floor(dividend, divisor),
            Round::FloorToZero => num::Integer::div_rem(dividend, divisor),
            Round::Ceil => {
                let (mut quotient, mut remainder) = num::Integer::div_rem(dividend, divisor);
                round_ceil(&mut quotient, &mut remainder, dividend, divisor);
                (quotient, remainder)
            }
            Round::Nearest => {
                let (mut quotient, mut remainder) = num::Integer::div_rem(dividend, divisor);
                round_nearest(&mut quotient, &mut remainder, dividend, divisor);
                (quotient, remainder)
            }
        }
    }

    #[inline]
    pub fn div_by_shift(dividend: &Int, shift: usize, rounding: Round) -> (Int, Int) {
        let divisor = Int::one() << shift;
        let remainder_mask = divisor.clone() - 1;
        let (mut quotient, mut remainder) = if dividend.is_negative() {
            let dividend = dividend.abs();
            (-(dividend.clone() >> shift), -(dividend & &remainder_mask))
        } else {
            (dividend >> shift, dividend & &remainder_mask)
        };
        match rounding {
            Round::FloorToNegativeInfinity => {
                round_floor_to_negative_infinity(&mut quotient, &mut remainder, dividend, &divisor)
            }
            Round::Ceil => round_ceil(&mut quotient, &mut remainder, dividend, &divisor),
            Round::Nearest => round_nearest(&mut quotient, &mut remainder, dividend, &divisor),
            _ => {}
        }
        (quotient, remainder)
    }

    #[inline]
    fn round_floor_to_negative_infinity(
        quotient: &mut Int,
        remainder: &mut Int,
        dividend: &Int,
        divisor: &Int,
    ) {
        if remainder.is_zero() || remainder.sign() == divisor.sign() {
            // No rounding needed
            return;
        }
        *remainder += divisor;
        if dividend.sign() == divisor.sign() {
            *quotient += 1;
        } else {
            *quotient -= 1;
        }
    }

    #[inline]
    fn round_ceil(quotient: &mut Int, remainder: &mut Int, dividend: &Int, divisor: &Int) {
        if remainder.is_zero() || remainder.sign() != divisor.sign() {
            // No rounding needed
            return;
        }
        *remainder -= divisor;
        if dividend.sign() == divisor.sign() {
            *quotient += 1;
        } else {
            *quotient -= 1;
        }
    }

    #[inline]
    fn round_nearest(quotient: &mut Int, remainder: &mut Int, dividend: &Int, divisor: &Int) {
        if remainder.is_zero() {
            // No rounding needed
            return;
        }
        //  5 / 2  ->   2,  1  ->   3, -1
        // -5 / 2  ->  -2, -1  ->  -2, -1
        //  5 /-2  ->  -2,  1  ->  -2,  1
        // -5 /-2  ->   2, -1  ->   3,  1
        let r_x2: Int = remainder.clone() << 1;
        let cmp_result = r_x2.abs().cmp(&divisor.abs());
        let is_not_negative = dividend.sign() == divisor.sign();
        if cmp_result == Ordering::Greater || (cmp_result == Ordering::Equal && is_not_negative) {
            if divisor.sign() == remainder.sign() {
                *remainder -= divisor;
            } else {
                *remainder += divisor;
            }
            if is_not_negative {
                *quotient += 1;
            } else {
                *quotient -= 1;
            }
        }
    }
}
