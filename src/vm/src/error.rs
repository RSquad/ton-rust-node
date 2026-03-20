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
use crate::stack::{integer::IntegerData, StackItem};
use ton_block::{error, Error, Exception, ExceptionCode, Result};

#[derive(Debug, thiserror::Error)]
pub struct TvmError {
    pub exception: Exception,
    pub value: StackItem,
}

impl std::fmt::Display for TvmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TvmError: {} - {}", self.exception, self.value)
    }
}

impl TvmError {
    pub fn new(exception: Exception, value: StackItem) -> Self {
        Self { exception, value }
    }
    pub fn exception(
        code: ExceptionCode,
        comment: impl ToString,
        value: impl Into<IntegerData>,
        file: &'static str,
        line: u32,
    ) -> Self {
        Self {
            exception: Exception::from_code(code, comment.to_string(), file, line),
            value: StackItem::int(value),
        }
    }
}

// pub fn is_out_of_gas(err: &Error) -> bool {
//     let err = match err.downcast_ref() {
//         Some(TvmError { exception, value }) => exception.exception_code(),
//         Some(_) => return false,
//         None => err.downcast_ref::<ExceptionCode>().cloned()
//     };
//     err.map_or(false, |code| code == ExceptionCode::OutOfGas)
// }

// pub fn is_normal_termination(err: &Error) -> Option<i32> {
//     match tvm_exception_code(err)? {
//         ExceptionCode::NormalTermination => Some(0),
//         ExceptionCode::AlternativeTermination => Some(1),
//         _ => None,
//     }
// }

// pub fn tvm_exception_value(err: &Error) -> StackItem {
//     if let Some(TvmError::TvmExceptionFull(_err, value)) = err.downcast_ref() {
//         return value.clone();
//     }
//     StackItem::default()
// }

pub fn tvm_exception(err: &Error) -> Option<&Exception> {
    if let Some(error) = err.downcast_ref::<TvmError>() {
        Some(&error.exception)
    } else if let Some(exception) = err.downcast_ref::<Exception>() {
        Some(exception)
    } else {
        None
    }
}

pub fn tvm_exception_full(err: Error) -> Result<(Exception, StackItem)> {
    let err = match err.downcast() {
        Ok(TvmError { exception, value }) => return Ok((exception, value)),
        Err(err) => err,
    };
    let err = match err.downcast::<Exception>() {
        Ok(exception) => return Ok((exception, StackItem::int(0))),
        Err(err) => err,
    };
    match err.downcast::<ExceptionCode>() {
        Ok(code) => {
            Ok((Exception::from_code(code, String::new(), file!(), line!()), StackItem::int(0)))
        }
        Err(err) => Err(err),
    }
}

pub fn tvm_exception_code(err: &Error) -> Option<ExceptionCode> {
    if let Some(error) = err.downcast_ref::<TvmError>() {
        error.exception.exception_code()
    } else if let Some(exception) = err.downcast_ref::<Exception>() {
        exception.exception_code()
    } else {
        err.downcast_ref::<ExceptionCode>().cloned()
    }
}

pub fn tvm_exception_or_custom_code(err: &Error) -> i32 {
    if let Some(err) = err.downcast_ref::<TvmError>() {
        err.exception.exception_or_custom_code()
    } else if let Some(err) = err.downcast_ref::<Exception>() {
        err.exception_or_custom_code()
    } else {
        err.downcast_ref::<ExceptionCode>()
            .map_or(ExceptionCode::UnknownError as i32, |code| *code as i32)
    }
}

pub fn update_error_description(mut err: Error, f: impl FnOnce(&str) -> String) -> Error {
    if let Some(error) = err.downcast_mut::<TvmError>() {
        error.exception.comment = f(error.exception.comment.as_str());
    } else if let Some(exception) = err.downcast_mut::<Exception>() {
        exception.comment = f(exception.comment.as_str());
    } else if let Some(code) = err.downcast_ref::<ExceptionCode>() {
        let exception = Exception::from_code(*code, f(&format!("{:?}", err)), file!(), line!());
        err = error!(exception)
    }
    err
}

#[cfg(test)]
#[path = "tests/test_error.rs"]
mod tests;
