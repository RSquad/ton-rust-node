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
use ton_block::{fail, Exception, ExceptionCode, Status};

pub trait OperationBehavior {
    fn quiet() -> bool;
    fn name_prefix() -> Option<&'static str>;
    fn on_nan_parameter(file: &'static str, line: u32) -> Status;
    fn on_integer_overflow(file: &'static str, line: u32) -> Status;
    fn on_range_check_error(file: &'static str, line: u32) -> Status;
}

pub struct Signaling {}
pub struct Quiet {}

#[macro_export]
macro_rules! on_integer_overflow {
    ($T: ident) => {{
        $T::on_integer_overflow(file!(), line!())
    }};
}

#[macro_export]
macro_rules! on_nan_parameter {
    ($T: ident) => {{
        $T::on_nan_parameter(file!(), line!())
    }};
}

impl OperationBehavior for Signaling {
    fn quiet() -> bool {
        false
    }
    fn name_prefix() -> Option<&'static str> {
        None
    }
    fn on_integer_overflow(file: &'static str, line: u32) -> Status {
        fail!(Exception::from_code(ExceptionCode::IntegerOverflow, String::new(), file, line))
    }
    fn on_nan_parameter(file: &'static str, line: u32) -> Status {
        fail!(Exception::from_code(ExceptionCode::IntegerOverflow, String::new(), file, line))
    }
    fn on_range_check_error(file: &'static str, line: u32) -> Status {
        fail!(Exception::from_code(ExceptionCode::RangeCheckError, String::new(), file, line))
    }
}

impl OperationBehavior for Quiet {
    fn quiet() -> bool {
        true
    }
    fn name_prefix() -> Option<&'static str> {
        Some("Q")
    }
    fn on_integer_overflow(_: &'static str, _: u32) -> Status {
        Ok(())
    }
    fn on_nan_parameter(_: &'static str, _: u32) -> Status {
        Ok(())
    }
    fn on_range_check_error(_: &'static str, _: u32) -> Status {
        Ok(())
    }
}
