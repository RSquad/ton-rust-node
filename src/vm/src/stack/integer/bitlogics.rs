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
    utils::{binary_op, process_single_result, unary_op},
    IntegerData,
};
use ton_block::Result;

impl IntegerData {
    pub fn and<T>(&self, other: &IntegerData) -> Result<IntegerData>
    where
        T: OperationBehavior,
    {
        binary_op::<T, _, _, _, _, _>(
            self,
            other,
            |x, y| x & y,
            IntegerData::nan,
            process_single_result::<T, _>,
        )
    }

    pub fn or<T>(&self, other: &IntegerData) -> Result<IntegerData>
    where
        T: OperationBehavior,
    {
        binary_op::<T, _, _, _, _, _>(
            self,
            other,
            |x, y| x | y,
            IntegerData::nan,
            process_single_result::<T, _>,
        )
    }

    pub fn xor<T>(&self, other: &IntegerData) -> Result<IntegerData>
    where
        T: OperationBehavior,
    {
        binary_op::<T, _, _, _, _, _>(
            self,
            other,
            |x, y| x ^ y,
            IntegerData::nan,
            process_single_result::<T, _>,
        )
    }

    pub fn not<T>(&self) -> Result<IntegerData>
    where
        T: OperationBehavior,
    {
        unary_op::<T, _, _, _, _, _>(self, |x| !x, IntegerData::nan, process_single_result::<T, _>)
    }

    pub fn shl<T>(&self, shift: usize) -> Result<IntegerData>
    where
        T: OperationBehavior,
    {
        unary_op::<T, _, _, _, _, _>(
            self,
            |x| x << shift,
            IntegerData::nan,
            process_single_result::<T, _>,
        )
    }

    pub fn shr<T>(&self, shift: usize) -> Result<IntegerData>
    where
        T: OperationBehavior,
    {
        unary_op::<T, _, _, _, _, _>(
            self,
            |x| x >> shift,
            IntegerData::nan,
            process_single_result::<T, _>,
        )
    }
}
