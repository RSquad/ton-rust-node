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
use crate::stack::integer::IntegerData;
use ton_block::ExceptionCode;

#[test]
fn test_as_integer_value() {
    let one = IntegerData::one();
    let nan = IntegerData::nan();

    assert_eq!(one.as_integer_value(0..=1).unwrap(), 1i8);
    assert_eq!(
        crate::error::tvm_exception_code(&one.as_integer_value(0..=0).unwrap_err()),
        Some(ExceptionCode::RangeCheckError)
    );
    assert_eq!(
        crate::error::tvm_exception_code(&one.as_integer_value(2..=2).unwrap_err()),
        Some(ExceptionCode::RangeCheckError)
    );
    assert_eq!(
        crate::error::tvm_exception_code(&nan.as_integer_value(0..=0).unwrap_err()),
        Some(ExceptionCode::RangeCheckError)
    );
}
