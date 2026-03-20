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
use super::*;
use ton_block::{error, fail};

#[test]
fn test_tvm_exception_code() {
    let err = error!(ExceptionCode::RangeCheckError);
    assert_eq!(tvm_exception_code(&err).unwrap(), ExceptionCode::RangeCheckError);
    let err = error!(ExceptionCode::RangeCheckError, "just a text");
    assert_eq!(tvm_exception_code(&err).unwrap(), ExceptionCode::RangeCheckError);
    let err = error!(ExceptionCode::RangeCheckError, "text with format {} - {}", 123, 456);
    assert_eq!(tvm_exception_code(&err).unwrap(), ExceptionCode::RangeCheckError);

    let err = || -> Result<()> { fail!(ExceptionCode::RangeCheckError) }().unwrap_err();
    assert_eq!(tvm_exception_code(&err).unwrap(), ExceptionCode::RangeCheckError);
    let err = || -> Result<()> { fail!("just a text") }().unwrap_err();
    assert_eq!(tvm_exception_code(&err), None);
}

#[test]
fn test_update_error() {
    let err = error!(ExceptionCode::RangeCheckError, "description {}", 42);
    println!("{:?}", err);
    let err = update_error_description(err, |d| format!("additional: {}", d));
    println!("{:?}", err);
    assert_eq!(tvm_exception_code(&err).unwrap(), ExceptionCode::RangeCheckError);
    assert!(err.to_string().contains("additional: "));

    // TODO: make fail! more informative
    // let err = || -> Result<()> { fail!(ExceptionCode::RangeCheckError, "lost description {}", 0) }().unwrap_err();
    // println!("{:?}", err);
    // let err = update_error_description(err, |d| format!("additional: {}", d));
    // println!("{:?}", err);
    // assert_eq!(tvm_exception_code(&err).unwrap(), ExceptionCode::RangeCheckError);

    let exception = Exception::from_number(112, "some text".to_string(), file!(), line!());
    let err = error!(TvmError::new(exception, StackItem::int(0)));
    println!("{:?}", err);
    let err = update_error_description(err, |d| format!("additional: {}", d));
    println!("{:?}", err);
    assert_eq!(tvm_exception_code(&err), None);
    assert_eq!(tvm_exception_or_custom_code(&err), 112);
}
